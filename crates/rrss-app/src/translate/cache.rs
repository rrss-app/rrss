//! 翻译输入 / 配置身份与缓存编解码。仅 app 层解释缓存封套，通用 IR 不认识它。

use super::types::UnitState;
use super::*;
use rrss_core::{ArticleRow, Error, Result as CoreResult, ai_store::ResolvedTask, db};
use rrss_render::{Document, IR_VERSION, extract_units};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

// 修改抽取或翻译输出约定时提升；不和共享 articles.hash 绑定。
const TRANSLATION_VERSION: u32 = 1;
const CACHE_VERSION: u32 = 1;
const PROGRESS_VERSION: u32 = 2;

#[derive(Clone)]
pub(super) struct Config {
    pub backend: TranslateBackend,
    pub lang: String,
    pub complete: Option<ResolvedTask>,
    pub hash: String,
    // 只用于在途请求的提交检查，不持久化凭据摘要，不暴露到 IPC。
    pub(super) execution_hash: String,
    pub(super) revision: u64,
}

fn endpoint(url: &str) -> CoreResult<String> {
    let mut url = reqwest::Url::parse(url.trim())
        .map_err(|_| Error::InvalidSetting("翻译服务地址无效".into()))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(Error::InvalidSetting(
            "翻译服务地址必须使用 HTTP 或 HTTPS".into(),
        ));
    }
    url.set_fragment(None);
    Ok(url.to_string())
}

impl Config {
    pub fn read(conn: &Connection) -> CoreResult<Self> {
        let backend = selected_backend(conn)?;
        let lang = translations::target_lang(conn)?;
        let complete = if backend == TranslateBackend::Complete {
            Some(ai_store::resolve_task(conn, TASK_ARTICLE_TRANSLATE)?)
        } else {
            None
        };
        let (identity, secret) = match &backend {
            TranslateBackend::Complete => {
                let task = complete.as_ref().expect("resolved complete config");
                let url = endpoint(task.provider.base_url.trim().trim_end_matches('/'))?;
                // 协议只决定请求形状，不改变译文；Auto 探测回写 chat / responses 不能使译文或在途作业失效。
                (
                    serde_json::json!([
                        url,
                        task.provider.id,
                        task.model,
                        task.template.version,
                        task.template.body,
                    ]),
                    task.provider.api_key.as_deref(),
                )
            }
            TranslateBackend::Http { base_url, api_key }
            | TranslateBackend::DeepLx { base_url, api_key } => {
                let url = http_translate_url(base_url)
                    .map_err(|_| Error::InvalidSetting("翻译服务地址无效".into()))?;
                (serde_json::json!(endpoint(&url)?), api_key.as_deref())
            }
            TranslateBackend::DeepL { base_url, api_key } => {
                // Free / Pro 只改变鉴权和服务入口，不改变已完成译文的内容身份。
                // 自定义代理仍参与身份；清除 Key 后离线缓存继续可读。
                let url = match base_url.as_deref().filter(|url| !url.trim().is_empty()) {
                    Some(url) => endpoint(&resolve_deepl_official_url(Some(url), ""))?,
                    None => "deepl:official".into(),
                };
                (
                    serde_json::json!([url, DEEPL_XML_PROMPT_VERSION]),
                    api_key.as_deref(),
                )
            }
        };
        let hash = ai_store::hash_input(
            &serde_json::json!([TRANSLATION_VERSION, lang, backend.id(), identity]).to_string(),
        );
        let execution_hash = ai_store::hash_input(
            &serde_json::json!([hash, secret.filter(|value| !value.is_empty())]).to_string(),
        );
        Ok(Self {
            backend,
            lang,
            complete,
            hash,
            execution_hash,
            revision: crate::configuration::translation_revision(conn)?,
        })
    }

    pub(super) fn optional(conn: &Connection) -> CoreResult<Option<Self>> {
        match Self::read(conn) {
            Ok(config) => Ok(Some(config)),
            Err(
                Error::ProviderNotFound
                | Error::TaskModelUnset(_)
                | Error::TemplateNotFound(_)
                | Error::InvalidSetting(_),
            ) => Ok(None),
            Err(err) => Err(err),
        }
    }

    pub fn model(&self) -> Option<String> {
        self.complete.as_ref().map(|task| task.model.clone())
    }

    pub fn prompt_version(&self) -> i64 {
        self.complete.as_ref().map_or_else(
            || {
                if matches!(self.backend, TranslateBackend::DeepL { .. }) {
                    DEEPL_XML_PROMPT_VERSION
                } else {
                    1
                }
            },
            |task| task.template.version,
        )
    }
}

/// 配置写入边界只需要执行身份，不暴露翻译工作副本或凭据。
pub(crate) fn configuration_fingerprint(conn: &Connection) -> CoreResult<Option<String>> {
    let config = Config::optional(conn)?;
    let enabled = translations::is_enabled(conn)?;
    Ok(config.map(|config| format!("{}:{enabled}", config.execution_hash)))
}

#[derive(Serialize, Deserialize)]
pub(super) struct Cached {
    version: u32,
    config_hash: String,
    pub document: Document,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    states: Option<Vec<UnitState>>,
}

#[cfg(test)]
pub(super) fn encode(config: &Config, document: Document) -> CoreResult<String> {
    serde_json::to_string(&Cached {
        version: CACHE_VERSION,
        config_hash: config.hash.clone(),
        document,
        states: None,
    })
    .map_err(|_| Error::Parse("无法编码翻译缓存".into()))
}

pub(super) fn read(
    conn: &Connection,
    article: &ArticleRow,
    source: &Document,
    config: &Config,
) -> CoreResult<Option<(String, Document)>> {
    if article.pruned_at.is_some() {
        return Ok(None);
    }
    let Some(row) = translations::get(conn, article.id, &config.lang, config.backend.id())? else {
        return Ok(None);
    };
    if row.source_hash != crate::document::source_hash(&article.title, source) {
        return Ok(None);
    }
    let Ok(cached) = serde_json::from_str::<Cached>(&row.blocks_json) else {
        return Ok(None);
    };
    if !(cached.version == CACHE_VERSION
        || (cached.version == PROGRESS_VERSION
            && cached.states.as_ref().is_some_and(|states| {
                states.len() == extract_units(&article.title, source).len()
                    && !states.is_empty()
                    && states.iter().all(|state| *state == UnitState::Done)
            })))
        || cached.config_hash != config.hash
        || cached.document.v != IR_VERSION
        || cached.document.blocks.len() != source.blocks.len()
        || cached
            .document
            .blocks
            .iter()
            .zip(&source.blocks)
            .any(|(translated, original)| {
                std::mem::discriminant(translated) != std::mem::discriminant(original)
            })
    {
        return Ok(None);
    }
    Ok(Some((row.title, cached.document)))
}

/// 配置缺失不阻止阅读；真实数据库错误仍上报，不伪装成缓存未命中。
pub(crate) fn for_reader(
    conn: &Connection,
    article: &ArticleRow,
    source: &Document,
) -> CoreResult<Option<(String, Document)>> {
    let Some(config) = Config::optional(conn)? else {
        return Ok(None);
    };
    read(conn, article, source, &config)
}

/// 取得 SQLite 写锁后才核对输入和配置，核对与 upsert 之间无其它写者。
/// 网络等待不持有事务；清理 / 抓全文先提交则旧结果拒绝，后提交则旧缓存不再有效。
#[cfg(test)]
pub(super) fn commit(
    conn: &Connection,
    id: i64,
    source_hash: &str,
    config: &Config,
    title: String,
    document: Document,
) -> CoreResult<()> {
    let blocks_json = encode(config, document)?;
    commit_encoded(conn, id, source_hash, config, title, blocks_json, false).map(|_| ())
}

pub(super) fn matches_current(
    conn: &Connection,
    id: i64,
    source_hash: &str,
    config: &Config,
) -> CoreResult<bool> {
    let current = articles::get_article(conn, id)?;
    let doc = crate::document::load_document(&current);
    Ok(current.pruned_at.is_none()
        && translations::is_enabled(conn)?
        && crate::document::source_hash(&current.title, &doc) == source_hash
        && Config::optional(conn)?.is_some_and(|value| {
            value.execution_hash == config.execution_hash && value.revision == config.revision
        }))
}

fn commit_encoded(
    conn: &Connection,
    id: i64,
    source_hash: &str,
    config: &Config,
    title: String,
    blocks_json: String,
    keep_complete: bool,
) -> CoreResult<Commit> {
    let tx = db::write_transaction(conn)?;
    if !matches_current(&tx, id, source_hash, config)? {
        return Err(Error::InvalidSetting(
            "正文或翻译配置已更新，请重新翻译".into(),
        ));
    }
    // 与 upsert 同一写事务核对，避免另一作业刚写入的完整译文被部分结果覆盖。
    if keep_complete {
        let article = articles::get_article(&tx, id)?;
        let source = crate::document::load_document(&article);
        if read(&tx, &article, &source, config)?.is_some() {
            return Ok(Commit::Kept);
        }
    }
    translations::upsert(
        &tx,
        &ArticleTranslation {
            article_id: id,
            target_lang: config.lang.clone(),
            source_hash: source_hash.into(),
            engine: config.backend.id().into(),
            model: config.model(),
            prompt_version: config.prompt_version(),
            title,
            blocks_json,
            created_at: translations::now(),
        },
    )?;
    tx.commit()?;
    Ok(Commit::Saved)
}

pub(super) enum Commit {
    Saved,
    /// 本次未全部译完，库里已有同一输入的完整译文：不覆盖，旧结果继续有效。
    Kept,
}

pub(super) struct ProgressCache {
    pub title: String,
    pub document: Document,
    pub states: Vec<UnitState>,
}

pub(super) fn read_progress(
    conn: &Connection,
    article: &ArticleRow,
    source: &Document,
    config: &Config,
) -> CoreResult<Option<ProgressCache>> {
    if article.pruned_at.is_some() {
        return Ok(None);
    }
    let Some(row) = translations::get(conn, article.id, &config.lang, config.backend.id())? else {
        return Ok(None);
    };
    if row.source_hash != crate::document::source_hash(&article.title, source) {
        return Ok(None);
    }
    let Ok(cached) = serde_json::from_str::<Cached>(&row.blocks_json) else {
        return Ok(None);
    };
    let Some(states) = cached.states else {
        return Ok(None);
    };
    if cached.version != PROGRESS_VERSION
        || cached.config_hash != config.hash
        || cached.document.v != IR_VERSION
        || cached.document.blocks.len() != source.blocks.len()
        || cached
            .document
            .blocks
            .iter()
            .zip(&source.blocks)
            .any(|(a, b)| std::mem::discriminant(a) != std::mem::discriminant(b))
        || states.len() != extract_units(&article.title, source).len()
        || !states.contains(&UnitState::Done)
        || states
            .iter()
            .any(|state| matches!(state, UnitState::Pending | UnitState::Inflight))
    {
        return Ok(None);
    }
    Ok(Some(ProgressCache {
        title: row.title,
        document: cached.document,
        states,
    }))
}

pub(super) fn commit_progress(
    conn: &Connection,
    id: i64,
    source_hash: &str,
    config: &Config,
    cache: ProgressCache,
) -> CoreResult<Commit> {
    if !cache.states.contains(&UnitState::Done)
        || cache
            .states
            .iter()
            .any(|state| matches!(state, UnitState::Pending | UnitState::Inflight))
    {
        return Err(Error::InvalidSetting(
            "翻译结果尚未结束或没有有效译文".into(),
        ));
    }
    let complete = cache.states.iter().all(|state| *state == UnitState::Done);
    let json = serde_json::to_string(&Cached {
        version: PROGRESS_VERSION,
        config_hash: config.hash.clone(),
        document: cache.document,
        states: Some(cache.states),
    })
    .map_err(|_| Error::Parse("无法编码翻译缓存".into()))?;
    commit_encoded(conn, id, source_hash, config, cache.title, json, !complete)
}
