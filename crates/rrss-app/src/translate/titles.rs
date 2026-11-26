//! 自动翻译：列表标题的按需批量翻译、文章卡片标注与阅读页判定。约定见翻译规格「自动翻译」。
//! 标题译文只用于显示，不改规则、兴趣、嵌入与全文索引；日志不记标题原文和译文。

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use rrss_core::articles::ROW_CARD;
use rrss_core::translations::{self, FeedAuto};
use rrss_render::TranslateUnit;
use rusqlite::Connection;

use super::{Config, TranslateBackend, spawn_db, translate_units, validate_units, window};
use crate::views::{
    ArticleView, AutoBodyState, AutoTranslateView, TitleTranslationView, TitleTranslations,
    TranslationCommandError, TranslationFailure,
};

/// 一次最多翻的标题数；前端按这个分批。由 export_ts 导出前端常量。
pub const MAX_TITLES: usize = 20;
/// 一批标题的总期限；DeepLX 逐条请求，比单窗宽。
const BATCH_TIMEOUT: Duration = Duration::from_secs(90);
/// 存下的译文标题上限（字符）；超出说明服务答非所问，丢掉。
const MAX_TITLE_CHARS: usize = 400;

pub const PAUSED_DISABLED: &str = "disabled";
pub const PAUSED_UNCONFIGURED: &str = "unconfigured";
pub const PAUSED_BUDGET: &str = "budget";

/// 当前设置下哪些文章自动翻译。每次查询现读，不缓存。
pub(crate) struct Policy {
    enabled: bool,
    config: Option<Config>,
    lang: String,
    titles: bool,
    body: bool,
    view: String,
    max_words: i64,
    english_only: bool,
    feeds: HashMap<i64, FeedAuto>,
}

impl Policy {
    pub(crate) fn read(conn: &Connection) -> rrss_core::Result<Self> {
        use crate::settings_schema::current;
        let enabled = translations::is_enabled(conn)?;
        let config = if enabled {
            Config::optional(conn)?.filter(usable)
        } else {
            None
        };
        Ok(Self {
            enabled,
            config,
            lang: crate::ui_language::translate_target(conn)?,
            titles: current(conn, translations::AUTO_TITLES_KEY)? == "1",
            body: current(conn, translations::AUTO_BODY_KEY)? == "1",
            view: current(conn, translations::AUTO_VIEW_KEY)?,
            max_words: current(conn, translations::AUTO_MAX_WORDS_KEY)?
                .parse()
                .unwrap_or(6000),
            english_only: current(conn, translations::AUTO_SCOPE_KEY)? == "en",
            feeds: translations::feed_autos(conn)?,
        })
    }

    fn feed(&self, feed_id: i64) -> FeedAuto {
        self.feeds
            .get(&feed_id)
            .copied()
            .unwrap_or(FeedAuto::Follow)
    }

    /// 标题自动翻译对这个源生效：总开关开着、服务可用、全局或按源打开。
    fn titles_for(&self, feed_id: i64) -> bool {
        self.config.is_some() && self.feed(feed_id).applies(self.titles)
    }

    /// 正文自动翻译对这个源生效（服务不可用时阅读页仍要说明原因）。
    fn body_for(&self, feed_id: i64) -> bool {
        self.enabled && self.feed(feed_id).applies(self.body)
    }

    /// 没有任何源会翻标题时跳过整页的逐条判断。
    fn any_titles(&self) -> bool {
        self.config.is_some()
            && (self.titles || self.feeds.values().any(|auto| *auto == FeedAuto::Always))
    }

    fn uses_budget(&self) -> bool {
        self.config
            .as_ref()
            .is_some_and(|config| config.backend == TranslateBackend::Complete)
    }
}

/// DeepL 官方没有 Key 时等同未配置；其余后端在读配置时已校验地址。
fn usable(config: &Config) -> bool {
    !matches!(&config.backend, TranslateBackend::DeepL { api_key, .. }
        if api_key.as_deref().is_none_or(|key| key.trim().is_empty()))
}

/// 生成预算用完时不自动发请求；手动翻译不经过这里。
fn budget_exhausted(conn: &Connection) -> rrss_core::Result<bool> {
    match rrss_core::ai_store::ensure_auto_job_budget(
        conn,
        rrss_core::ai_store::BudgetKind::Generation,
    ) {
        Ok(()) => Ok(false),
        Err(rrss_core::Error::DailyBudgetExhausted) => Ok(true),
        Err(error) => Err(error),
    }
}

struct ResolvedArticle {
    title: String,
    foreign: Option<String>,
}

/// Shared by annotation and request preparation; safe inside a read snapshot.
fn resolve_article_langs(
    conn: &Connection,
    ids: &[i64],
    policy: &Policy,
) -> rrss_core::Result<HashMap<i64, ResolvedArticle>> {
    let rows = translations::article_languages(conn, ids)?;
    let feeds: Vec<_> = rows
        .iter()
        .filter(|row| row.lang.is_none() && policy.titles_for(row.feed_id))
        .map(|row| row.feed_id)
        .collect();
    let priors = translations::feed_language_priors(conn, &feeds)?;
    let mut resolved = HashMap::new();
    for row in rows {
        if !policy.titles_for(row.feed_id) {
            continue;
        }
        let detected = row.lang.or_else(|| {
            let prose = if let Some(text) = row.fulltext_preview.filter(|s| !s.is_empty()) {
                rrss_render::markdown_prose_preview(&text, 2000)
            } else {
                rrss_render::html_prose_preview(
                    row.html_preview.as_deref().unwrap_or_default(),
                    2000,
                )
            };
            rrss_render::detect_article_lang(
                &row.title,
                &prose,
                priors.get(&row.feed_id).map(String::as_str),
            )
        });
        let foreign = detected
            .as_deref()
            .and_then(|lang| {
                rrss_render::foreign_lang_code(lang, &policy.lang, policy.english_only)
            })
            .map(str::to_owned);
        resolved.insert(
            row.id,
            ResolvedArticle {
                title: row.title,
                foreign,
            },
        );
    }
    Ok(resolved)
}

/// 给列表的文章卡片标上外文语言码、已有的标题译文和正文译文状态。
pub fn annotate(conn: &Connection, articles: &mut [ArticleView]) -> rrss_core::Result<()> {
    let policy = Policy::read(conn)?;
    if !policy.any_titles() {
        return Ok(());
    }
    let ids: Vec<_> = articles
        .iter()
        .filter(|a| a.row_kind == ROW_CARD && policy.titles_for(a.feed_id))
        .map(|a| a.id)
        .collect();
    if ids.is_empty() {
        return Ok(());
    }
    let resolved = resolve_article_langs(conn, &ids, &policy)?;
    let mut foreign = Vec::new();
    for article in articles.iter_mut() {
        if article.row_kind != ROW_CARD {
            continue;
        }
        if let Some(lang) = resolved
            .get(&article.id)
            .and_then(|row| row.foreign.as_ref())
        {
            article.title_lang = Some(lang.clone());
            foreign.push(article.id);
        }
    }
    if foreign.is_empty() {
        return Ok(());
    }
    let titles = translations::title_translations(conn, &foreign, &policy.lang)?;
    let engine = policy.config.as_ref().map(|config| config.backend.id());
    let bodies = match engine {
        Some(engine) => translations::body_translated(conn, &foreign, &policy.lang, engine)?,
        None => Default::default(),
    };
    for article in articles.iter_mut().filter(|a| a.title_lang.is_some()) {
        article.title_translation = titles.get(&article.id).cloned();
        article.body_translated = Some(bodies.contains(&article.id));
    }
    Ok(())
}

enum Prepared {
    /// 不发请求；已存的译文照样带回。
    Paused(&'static str, Vec<(i64, String)>),
    Ready {
        config: Config,
        lang: String,
        items: Vec<(i64, String)>,
        stored: Vec<(i64, String)>,
    },
}

fn prepare(conn: &Connection, ids: &[i64]) -> rrss_core::Result<Prepared> {
    let policy = Policy::read(conn)?;
    if !policy.enabled {
        return Ok(Prepared::Paused(PAUSED_DISABLED, Vec::new()));
    }
    let Some(config) = policy.config.clone() else {
        return Ok(Prepared::Paused(PAUSED_UNCONFIGURED, Vec::new()));
    };
    let mut existing = translations::title_translations(conn, ids, &policy.lang)?;
    let missing: Vec<_> = ids
        .iter()
        .copied()
        .filter(|id| !existing.contains_key(id))
        .collect();
    let mut resolved = if missing.is_empty() {
        HashMap::new()
    } else {
        resolve_article_langs(conn, &missing, &policy)?
    };
    let mut items = Vec::new();
    let mut stored = Vec::new();
    for &id in ids {
        if let Some(title) = existing.remove(&id) {
            stored.push((id, title));
            continue;
        }
        if let Some(row) = resolved.remove(&id)
            && row.foreign.is_some()
        {
            items.push((id, row.title));
        }
    }
    if !items.is_empty() && policy.uses_budget() && budget_exhausted(conn)? {
        return Ok(Prepared::Paused(PAUSED_BUDGET, stored));
    }
    Ok(Prepared::Ready {
        config,
        lang: policy.lang,
        items,
        stored,
    })
}

fn clean_title(raw: &str) -> Option<String> {
    let title = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    (!title.is_empty() && title.chars().count() <= MAX_TITLE_CHARS).then_some(title)
}

/// 翻译一批列表标题并存下，连同已存的译文一起返回。只处理当前确实生效的外文标题，其余忽略。
pub async fn translate_titles(
    path: &Path,
    client: &reqwest::Client,
    ids: Vec<i64>,
) -> Result<TitleTranslations, TranslationCommandError> {
    let mut ids = ids;
    ids.sort_unstable();
    ids.dedup();
    ids.truncate(MAX_TITLES);
    let prepared = spawn_db(path.to_owned(), move |conn| prepare(conn, &ids))
        .await
        .map_err(|err| TranslationCommandError::from(window::classify(&err)))?;
    let views = |titles: Vec<(i64, String)>| -> Vec<TitleTranslationView> {
        titles
            .into_iter()
            .map(|(id, title)| TitleTranslationView { id, title })
            .collect()
    };
    let (config, lang, items, stored) = match prepared {
        Prepared::Paused(reason, stored) => {
            return Ok(TitleTranslations {
                titles: views(stored),
                paused: Some(reason.into()),
            });
        }
        Prepared::Ready {
            config,
            lang,
            items,
            stored,
        } => (config, lang, items, stored),
    };
    if items.is_empty() {
        return Ok(TitleTranslations {
            titles: views(stored),
            paused: None,
        });
    }
    let units: Vec<TranslateUnit> = items
        .iter()
        .enumerate()
        .map(|(i, (_, title))| TranslateUnit {
            i: i as u32,
            t: title.clone(),
        })
        .collect();
    let out = tokio::time::timeout(
        BATCH_TIMEOUT,
        translate_units(&config, path, client, &units, false),
    )
    .await
    .map_err(|_| TranslationCommandError::from(TranslationFailure::Timeout))?
    .map_err(|err| {
        let failure = window::classify(&err);
        tracing::debug!(
            backend = config.backend.id(),
            count = units.len(),
            "title translation failed"
        );
        TranslationCommandError::from(failure)
    })?;
    validate_units(&units, &out)
        .map_err(|_| TranslationCommandError::from(TranslationFailure::InvalidOutput))?;
    let translated: HashMap<u32, String> = out
        .into_iter()
        .filter_map(|unit| Some((unit.i, clean_title(&unit.t)?)))
        .collect();
    let titles: Vec<(i64, String)> = items
        .iter()
        .enumerate()
        .filter_map(|(i, (id, _))| Some((*id, translated.get(&(i as u32))?.clone())))
        .collect();
    let engine = config.backend.id();
    let saved = titles.clone();
    spawn_db(path.to_owned(), move |conn| {
        translations::put_title_translations(conn, &lang, engine, &saved)
    })
    .await
    .map_err(|err| TranslationCommandError::from(window::classify(&err)))?;
    tracing::debug!(
        backend = engine,
        requested = items.len(),
        translated = titles.len(),
        "translated list titles"
    );
    Ok(TitleTranslations {
        titles: views(stored.into_iter().chain(titles).collect()),
        paused: None,
    })
}

/// 阅读页的自动翻译判定。标题或正文自动翻译对这篇生效、且是外文时才有值。
pub(crate) fn reader_auto(
    conn: &Connection,
    article: &rrss_core::ArticleRow,
    doc: &rrss_render::Document,
    cached: bool,
) -> rrss_core::Result<Option<AutoTranslateView>> {
    let policy = Policy::read(conn)?;
    let titles = policy.titles_for(article.feed_id);
    let body = policy.body_for(article.feed_id);
    if !policy.enabled || !(titles || body) {
        return Ok(None);
    }
    // The stored article language retains entry-specific evidence when prose is
    // short; new, stronger body evidence still overrides it in the detector.
    let feed_prior = if article.lang.is_none() {
        translations::feed_language_priors(conn, &[article.feed_id])?.remove(&article.feed_id)
    } else {
        None
    };
    let prior = article.lang.as_deref().or(feed_prior.as_deref());
    let prose = rrss_render::document_prose_preview(doc, 2000);
    let detected = if prose.trim().is_empty() && article.lang.is_some() {
        article.lang.clone()
    } else {
        rrss_render::detect_article_lang(&article.title, &prose, prior)
    };
    let Some(lang) = detected
        .as_deref()
        .and_then(|lang| rrss_render::foreign_lang_code(lang, &policy.lang, policy.english_only))
    else {
        return Ok(None);
    };
    let plain = rrss_render::units_plain(&rrss_render::extract_units(&article.title, doc));
    let words = i64::try_from(rrss_render::word_count(&plain)).unwrap_or(i64::MAX);
    let state = if !body {
        AutoBodyState::Off
    } else if policy.config.is_none() {
        AutoBodyState::Unconfigured
    } else if cached {
        AutoBodyState::Cached
    } else if words > policy.max_words {
        AutoBodyState::TooLong
    } else if policy.uses_budget() && budget_exhausted(conn)? {
        AutoBodyState::Budget
    } else {
        AutoBodyState::Start
    };
    let title =
        translations::title_translations(conn, &[article.id], &policy.lang)?.remove(&article.id);
    Ok(Some(AutoTranslateView {
        lang: lang.into(),
        title,
        body: state,
        words,
        max_words: policy.max_words,
        view: policy.view,
    }))
}

#[cfg(test)]
mod tests;
