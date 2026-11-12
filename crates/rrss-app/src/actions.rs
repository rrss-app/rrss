//! 写 SQLite 的动作。无 UI 类型。

use rrss_core::ai_store::{AiProvider, NewProvider};
use rrss_core::articles;
use rrss_core::backup::{self, BackupKind};
use rrss_core::feed_scope::{self, FeedScope};
use rrss_core::feedback::{self, FeedbackKind};
use rrss_core::feeds::{self, SubscribeRequest};
use rrss_core::interests;
use rrss_core::memories;
use rrss_core::opml;
use rrss_core::pi_store;
use rrss_core::prefs;
use rrss_core::presets;
use rrss_core::rag;
use rrss_core::rules::{
    self, Dimension, NewRule, Polarity, RuleAction, RuleSource, ScopeKind, validate_pattern,
};
use rrss_core::storage::{self, Cleanup};
use rrss_core::threads::{self, MessageRole};
use rrss_core::timeline;
use rrss_core::translations;
use rrss_core::{ai_store, db, rescore_all};
use rusqlite::Connection;
use std::path::Path;

use crate::views::{
    BackupDoneView, CitationView, NewRuleSpec, PrefsImportView, RestoreCheckView,
    SaveAiSettingsArgs, SaveFeedArgs, SavePiSettingsArgs,
};

pub fn record_exposures(
    conn: &Connection,
    records: Vec<crate::views::ExposureInput>,
) -> rrss_core::Result<crate::views::ExposureIds> {
    rrss_core::signals::record_exposures(
        conn,
        &records.into_iter().map(Into::into).collect::<Vec<_>>(),
    )
}

pub fn record_article_events(
    conn: &Connection,
    records: Vec<crate::views::ArticleEventInput>,
) -> rrss_core::Result<usize> {
    let count = rrss_core::signals::record_events(
        conn,
        &records.into_iter().map(Into::into).collect::<Vec<_>>(),
    )?;
    if count > 0 {
        crate::jobs::rank::request(conn)?;
    }
    Ok(count)
}

pub fn record_read_sessions(
    conn: &Connection,
    records: Vec<crate::views::ReadSessionInput>,
) -> rrss_core::Result<usize> {
    let count = rrss_core::signals::record_read_sessions(
        conn,
        &records.into_iter().map(Into::into).collect::<Vec<_>>(),
    )?;
    if count > 0 {
        crate::jobs::rank::request(conn)?;
    }
    Ok(count)
}

pub fn add_feed(conn: &Connection, url: &str) -> rrss_core::Result<i64> {
    feeds::subscribe(
        conn,
        SubscribeRequest {
            url: url.trim().to_string(),
            folder_id: None,
            as_rsshub: false,
            rsshub_instance_id: None,
        },
    )
}

/// 订阅行内编辑的复合保存：名称 / 地址 / 文件夹同一事务，任一失败整体回滚，
/// 成功后在同一事务内读取订阅树返回。
pub fn save_feed(
    conn: &Connection,
    args: &SaveFeedArgs,
) -> rrss_core::Result<crate::views::FeedTree> {
    let tx = db::write_transaction(conn)?;
    feeds::get_feed(&tx, args.id)?;
    if let Some(title) = &args.title {
        feeds::rename_feed(&tx, args.id, title.trim())?;
    }
    if let Some(url) = &args.url {
        feeds::update_feed_url(&tx, args.id, url.trim())?;
    }
    if let Some(folder_id) = args.folder_id {
        feeds::set_folder(&tx, args.id, Some(folder_id).filter(|id| *id > 0))?;
    }
    if let Some(retention) = &args.retention {
        let days = match retention.trim() {
            "follow" => None,
            "keep" => Some(0),
            n => Some(n.parse::<i64>().ok().filter(|d| *d > 0).ok_or_else(|| {
                rrss_core::Error::InvalidRule(format!("保留天数需要是正整数：{n}"))
            })?),
        };
        rrss_core::retention::set_feed_days(&tx, args.id, days)?;
    }
    let tree = crate::queries::feed_tree(&tx)?;
    tx.commit()?;
    Ok(tree)
}

pub fn unsubscribe(conn: &Connection, id: i64) -> rrss_core::Result<()> {
    feeds::unsubscribe(conn, id)?;
    tracing::info!(feed_id = id, "feed unsubscribed");
    Ok(())
}

/// 批量退订（源健康里的「退订长期失败的源」），一个写事务。
pub fn unsubscribe_many(
    conn: &Connection,
    ids: &[i64],
) -> rrss_core::Result<crate::views::FeedTree> {
    let tx = db::write_transaction(conn)?;
    for id in ids {
        feeds::unsubscribe(&tx, *id)?;
    }
    let tree = crate::queries::feed_tree(&tx)?;
    tx.commit()?;
    tracing::info!(count = ids.len(), "feeds unsubscribed");
    Ok(tree)
}

/// 忽略源健康提示：同一种状态 30 天内不再提示。
pub fn dismiss_feed_health(
    conn: &Connection,
    id: i64,
) -> rrss_core::Result<crate::views::FeedTree> {
    rrss_core::feed_health::dismiss(conn, id, feeds::now_ms())?;
    crate::queries::feed_tree(conn)
}

pub fn create_folder(
    conn: &Connection,
    name: &str,
    parent_folder_id: Option<i64>,
) -> rrss_core::Result<i64> {
    feeds::create_folder_first(conn, name.trim(), parent_folder_id)
}

pub fn move_tree_items(
    conn: &Connection,
    items: Vec<String>,
    parent_folder_id: Option<i64>,
    before: Option<String>,
) -> rrss_core::Result<Vec<crate::views::TreeLayout>> {
    let items = items
        .iter()
        .map(|item| rrss_core::tree_order::TreeItem::parse(item))
        .collect::<rrss_core::Result<Vec<_>>>()?;
    let before = before
        .as_deref()
        .map(rrss_core::tree_order::TreeItem::parse)
        .transpose()?;
    let layout = rrss_core::tree_order::move_tree_items(conn, &items, parent_folder_id, before)?;
    Ok(layout
        .entries
        .into_iter()
        .map(|slot| crate::views::TreeLayout {
            item: slot.item.key(),
            parent_folder_id: slot.parent,
            position: slot.position,
        })
        .collect())
}

pub fn restore_tree_layout(
    conn: &Connection,
    layout: Vec<crate::views::TreeLayout>,
) -> rrss_core::Result<()> {
    let entries = layout
        .iter()
        .map(|row| {
            Ok(rrss_core::tree_order::TreePlacement {
                item: rrss_core::tree_order::TreeItem::parse(&row.item)?,
                parent: row.parent_folder_id,
                position: row.position,
            })
        })
        .collect::<rrss_core::Result<Vec<_>>>()?;
    rrss_core::tree_order::restore_tree_layout(conn, &rrss_core::tree_order::TreeLayout { entries })
}

pub fn rename_folder(conn: &Connection, id: i64, name: &str) -> rrss_core::Result<()> {
    feeds::rename_folder(conn, id, name.trim())
}

pub fn delete_folder(conn: &Connection, id: i64) -> rrss_core::Result<()> {
    feeds::delete_folder(conn, id)?;
    tracing::info!(folder_id = id, "folder deleted");
    Ok(())
}

pub fn mark_read(conn: &Connection, id: i64, read: bool) -> rrss_core::Result<()> {
    articles::set_read(conn, id, read)
}

pub fn open_article(conn: &Connection, id: i64) -> rrss_core::Result<crate::views::ReaderView> {
    let view = crate::queries::reader_view(conn, id)?;
    articles::set_read(conn, id, true)?;
    Ok(view)
}

pub fn star_article(conn: &Connection, id: i64, starred: bool) -> rrss_core::Result<()> {
    articles::set_starred(conn, id, starred)
}

pub fn mark_scope_read(conn: &Connection, scope: &str) -> rrss_core::Result<Vec<i64>> {
    let scope = feed_scope::resolve(conn, FeedScope::parse_str(scope))?;
    let ids = timeline::mark_scope_read(conn, scope)?;
    tracing::info!(count = ids.len(), "articles marked read");
    Ok(ids)
}

/// 相近报道组尾「全部标为已读」：范围与焦点页相同（`all` / `folder:<id>`），返回改动的 id。
pub fn mark_cluster_read(
    conn: &Connection,
    cluster_id: i64,
    scope: Option<&str>,
) -> rrss_core::Result<Vec<i64>> {
    let scope = feed_scope::resolve(conn, FeedScope::parse_str(scope.unwrap_or("all")))?;
    let ids = articles::mark_cluster_read(conn, cluster_id, scope)?;
    tracing::info!(cluster_id, count = ids.len(), "cluster marked read");
    Ok(ids)
}

pub fn unmark_read(conn: &Connection, ids: &[i64]) -> rrss_core::Result<()> {
    for id in ids {
        articles::set_read(conn, *id, false)?;
    }
    Ok(())
}

pub fn set_interest_enabled(conn: &Connection, id: i64, enabled: bool) -> rrss_core::Result<()> {
    interests::set_enabled(conn, id, enabled)
}

pub fn delete_interest(conn: &Connection, id: i64) -> rrss_core::Result<()> {
    interests::delete_interest(conn, id)
}

pub fn add_interest(conn: &Connection, label: &str) -> rrss_core::Result<i64> {
    interests::insert_interest(conn, label.trim(), "manual")
}

pub fn rate_article(
    conn: &Connection,
    id: i64,
    up: bool,
) -> rrss_core::Result<feedback::RateResult> {
    let kind = if up {
        FeedbackKind::Up
    } else {
        FeedbackKind::Down
    };
    let result = feedback::rate_article(conn, id, kind)?;
    crate::jobs::rank::request(conn)?;
    Ok(result)
}

fn spec_to_new_rule(spec: &NewRuleSpec) -> rrss_core::Result<NewRule> {
    let polarity = Polarity::parse_str(&spec.polarity)?;
    let dimension = Dimension::parse_str(&spec.dimension)?;
    validate_pattern(dimension, &spec.pattern)?;
    let scope_kind = ScopeKind::parse_str(&spec.scope_kind)?;
    Ok(NewRule {
        polarity,
        dimension,
        pattern: spec.pattern.trim().to_string(),
        scope_kind,
        scope_id: if scope_kind == ScopeKind::Global {
            None
        } else {
            Some(spec.scope_id)
        },
        weight: spec.weight,
        source: RuleSource::Manual,
        enabled: true,
        pack_id: None,
    })
}

/// Persist a rule change and its article scores atomically.
/// `rescore_all` reuses this transaction instead of starting a nested BEGIN.
fn persist_then_rescore<T>(
    conn: &Connection,
    persist: impl FnOnce(&Connection) -> rrss_core::Result<T>,
) -> rrss_core::Result<T> {
    let tx = db::write_transaction(conn)?;
    let out = persist(&tx)?;
    rescore_all(&tx)?;
    tx.commit()?;
    Ok(out)
}

pub fn create_rule(conn: &Connection, spec: &NewRuleSpec) -> rrss_core::Result<i64> {
    let spec = spec_to_new_rule(spec)?;
    persist_then_rescore(conn, |c| rules::insert_rule(c, spec.clone()))
}

pub fn release_article(conn: &Connection, id: i64) -> rrss_core::Result<()> {
    set_article_release(conn, id, Some(feeds::now_ms()))
}

pub fn unrelease_article(conn: &Connection, id: i64) -> rrss_core::Result<()> {
    set_article_release(conn, id, None)
}

fn set_article_release(conn: &Connection, id: i64, at: Option<i64>) -> rrss_core::Result<()> {
    let changed = conn.execute(
        "UPDATE articles SET released_at = ?1 WHERE id = ?2",
        rusqlite::params![at, id],
    )?;
    if changed == 0 {
        return Err(rrss_core::Error::ArticleNotFound);
    }
    Ok(())
}

pub fn allow_rule_for_feed(
    conn: &Connection,
    rule_id: i64,
    feed_id: i64,
) -> rrss_core::Result<i64> {
    persist_then_rescore(conn, |c| {
        let rule = rules::get_rule(c, rule_id)?;
        rules::insert_rule(
            c,
            NewRule {
                polarity: match rule.polarity {
                    Polarity::Like => Polarity::Dislike,
                    Polarity::Dislike => Polarity::Like,
                },
                dimension: rule.dimension,
                pattern: rule.pattern,
                scope_kind: ScopeKind::Feed,
                scope_id: Some(feed_id),
                weight: rule.weight.abs(),
                source: RuleSource::Manual,
                enabled: true,
                pack_id: None,
            },
        )
    })
}

pub fn update_rule(conn: &Connection, id: i64, spec: &NewRuleSpec) -> rrss_core::Result<()> {
    persist_then_rescore(conn, |c| {
        let mut rule = rules::get_rule(c, id)?;
        let n = spec_to_new_rule(spec)?;
        rule.polarity = n.polarity;
        rule.dimension = n.dimension;
        rule.pattern = n.pattern;
        rule.weight = n.weight;
        rule.scope_kind = n.scope_kind;
        rule.scope_id = n.scope_id;
        rules::update_rule(c, &rule)
    })
}

pub fn set_rule_action(conn: &Connection, id: i64, action: i32) -> rrss_core::Result<()> {
    let mapped = match action {
        0 => RuleAction::Like,
        1 => RuleAction::Dislike,
        _ => RuleAction::Neutral,
    };
    persist_then_rescore(conn, |c| rules::set_rule_action(c, id, mapped))
}

pub fn confirm_suggested_rule(conn: &Connection, id: i64) -> rrss_core::Result<()> {
    persist_then_rescore(conn, |c| rules::confirm_rule(c, id))
}

pub fn discard_rule(conn: &Connection, id: i64) -> rrss_core::Result<()> {
    persist_then_rescore(conn, |c| rules::delete_rule(c, id))
}

pub fn apply_preset(conn: &Connection, id: &str) -> rrss_core::Result<u32> {
    persist_then_rescore(conn, |c| presets::apply_pack(c, id))
}

pub fn remove_preset(conn: &Connection, id: &str) -> rrss_core::Result<u32> {
    persist_then_rescore(conn, |c| presets::remove_pack(c, id))
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct SaveTranslateArgs {
    pub enabled: bool,
    pub backend: String,
    pub target_lang: String,
    pub deepl_key: Option<String>,
    pub deepl_url: Option<String>,
    pub deeplx_url: Option<String>,
    pub deeplx_token: Option<String>,
    pub http_url: Option<String>,
    pub http_key: Option<String>,
}

pub fn save_translate_settings(
    conn: &Connection,
    args: &SaveTranslateArgs,
) -> rrss_core::Result<()> {
    crate::configuration::write(conn, |tx| {
        translations::set_enabled(tx, args.enabled)?;
        translations::set_backend(tx, &args.backend)?;
        translations::set_target_lang(tx, &args.target_lang)?;
        translations::set_deepl_config(tx, args.deepl_url.as_deref(), args.deepl_key.as_deref())?;
        translations::set_deeplx_config(
            tx,
            args.deeplx_url.as_deref(),
            args.deeplx_token.as_deref(),
        )?;
        translations::set_http_config(tx, args.http_url.as_deref(), args.http_key.as_deref())?;
        Ok(())
    })
}

pub fn confirm_feature(
    conn: &Connection,
    _article_id: i64,
    feed_id: i64,
    dislike: bool,
    feed_scope: bool,
    dimension: &str,
    pattern: &str,
) -> rrss_core::Result<i64> {
    let polarity = if dislike {
        Polarity::Dislike
    } else {
        Polarity::Like
    };
    let dimension = Dimension::parse_str(dimension)?;
    let (scope_kind, scope_id) = if feed_scope {
        (ScopeKind::Feed, Some(feed_id))
    } else {
        (ScopeKind::Global, None)
    };
    persist_then_rescore(conn, |c| {
        rules::insert_rule(
            c,
            NewRule {
                polarity,
                dimension,
                pattern: pattern.to_string(),
                scope_kind,
                scope_id,
                weight: rules::DEFAULT_WEIGHT,
                source: RuleSource::Manual,
                enabled: true,
                pack_id: None,
            },
        )
    })
}

pub fn rescore(conn: &Connection) -> rrss_core::Result<u32> {
    rescore_all(conn)
}

pub fn confirm_memory(conn: &Connection, id: i64) -> rrss_core::Result<()> {
    memories::confirm_memory(conn, id)
}

pub fn discard_memory(conn: &Connection, id: i64) -> rrss_core::Result<()> {
    memories::delete_memory(conn, id)
}

pub fn pin_memory(conn: &Connection, id: i64, pinned: bool) -> rrss_core::Result<()> {
    memories::set_pinned(conn, id, pinned)
}

pub fn clear_memories(conn: &Connection) -> rrss_core::Result<usize> {
    let count = memories::clear_memories(conn)?;
    tracing::info!(count, "memories cleared");
    Ok(count)
}

pub fn export_memories(conn: &Connection) -> rrss_core::Result<String> {
    memories::export_memories_json(conn)
}

pub fn edit_memory(conn: &Connection, id: i64, content: &str) -> rrss_core::Result<()> {
    memories::update_memory_content(conn, id, content)
}

pub fn memory_as_interest(conn: &Connection, id: i64) -> rrss_core::Result<i64> {
    db::atomic_write(conn, |conn| {
        let memory = memories::get_memory(conn, id)?;
        if !memory.pending() {
            return Err(rrss_core::Error::InvalidMemory(
                "这条记忆已确认，请刷新列表后查看，不能重复添加为兴趣".into(),
            ));
        }
        memories::confirm_memory(conn, id)?;
        interests::insert_interest(conn, &memory.content, "from_chat")
    })
}

/// 模式与模型、覆盖可一起提交；只写提供的字段，失败整体回滚。
/// 不启动 / 重启 Pi；现有 Profile 指纹机制在下次使用时应用。
pub fn save_pi_config(
    conn: &Connection,
    args: &crate::views::SavePiConfigArgs,
) -> rrss_core::Result<crate::views::PiConfigView> {
    let tx = db::write_transaction(conn)?;
    for (key, raw) in [
        (pi_store::PI_SETTINGS_OVERRIDE_KEY, &args.settings_override),
        (pi_store::PI_MODELS_OVERRIDE_KEY, &args.models_override),
    ] {
        if let Some(raw) = raw {
            pi_store::set_pi_override(&tx, key, raw)?;
        }
    }
    if let Some(enabled) = args.use_own_config {
        pi_store::set_use_own_config(&tx, enabled)?;
    }
    if let Some(model) = &args.own_model {
        pi_store::set_own_model(&tx, model)?;
    }
    let view = crate::queries::pi_config(&tx)?;
    tx.commit()?;
    Ok(view)
}

pub fn delete_provider(conn: &Connection, id: i64) -> rrss_core::Result<()> {
    crate::configuration::write(conn, |tx| ai_store::delete_provider_in_transaction(tx, id))
}

pub fn save_model_bindings(
    conn: &Connection,
    args: &crate::views::SaveModelBindingsArgs,
) -> rrss_core::Result<crate::views::ModelBindingsView> {
    crate::configuration::write(conn, |tx| {
        for binding in &args.bindings {
            bind_task(tx, &binding.task, binding.provider_id, &binding.model)?;
        }
        let view = crate::queries::model_bindings(tx)?;
        Ok(view)
    })
}

/// 连接配置在同一事务保存，任一步失败整体回滚。
/// 密钥只在输入非空时替换；成功后在同一事务内读取权威快照返回。
pub fn save_ai_settings(
    conn: &Connection,
    args: &SaveAiSettingsArgs,
) -> rrss_core::Result<crate::views::AiPage> {
    use rrss_core::error::Error;
    let input = &args.provider;
    if input.name.trim().is_empty() {
        return Err(Error::InvalidProvider("连接名称不能为空".into()));
    }
    crate::capabilities::validate_base_url(&input.base_url)?;
    if !matches!(input.protocol.trim(), "chat" | "responses" | "auto") {
        return Err(Error::InvalidProvider("不支持的连接协议".into()));
    }
    let preset = input
        .kind
        .as_deref()
        .or((input.id == 0).then_some("openai_compat"))
        .map(|kind| {
            crate::provider_presets::provider_presets()
                .into_iter()
                .find(|preset| preset.id == kind)
                .ok_or_else(|| Error::InvalidProvider("未知的连接预设".into()))
        })
        .transpose()?;
    let compat = input
        .compat_json
        .as_deref()
        .map(|raw| {
            let raw = if raw.trim().is_empty() { "{}" } else { raw };
            let value: serde_json::Value = serde_json::from_str(raw)
                .map_err(|_| Error::InvalidCompat("兼容配置必须是 JSON 对象".into()))?;
            if !value.is_object() {
                return Err(Error::InvalidCompat("兼容配置必须是 JSON 对象".into()));
            }
            Ok(value)
        })
        .transpose()?;
    crate::configuration::write(conn, |tx| {
        // 弹层草稿在这次事务之前不落库；后续任一步失败连新连接一起回滚。
        let provider_id = if input.id == 0 {
            ai_store::insert_provider(
                tx,
                &NewProvider {
                    name: input.name.trim().into(),
                    base_url: input.base_url.trim().into(),
                    api_key: None,
                    protocol: input.protocol.clone(),
                },
            )?
        } else {
            input.id
        };
        let existing = ai_store::get_provider(tx, provider_id)?;
        let provider = AiProvider {
            id: provider_id,
            name: args.provider.name.trim().to_string(),
            base_url: args.provider.base_url.trim().to_string(),
            // 新填的密钥替换；「清除」删掉；否则保留（前端拿不到完整密钥，阶段 19-9）。
            api_key: match args
                .provider
                .api_key
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                Some(key) => Some(key.to_string()),
                None if args.provider.clear_key == Some(true) => None,
                None => existing.api_key,
            },
            protocol: args.provider.protocol.clone(),
            kind: preset
                .as_ref()
                .map(|p| p.id.clone())
                .unwrap_or(existing.kind),
            last_test: existing.last_test,
        };
        ai_store::update_provider(tx, &provider)?;
        if input.id == 0 {
            if let Some(preset) = preset {
                let compat = serde_json::from_str(&preset.compat_json)
                    .map_err(|_| Error::InvalidCompat("预设兼容配置无效".into()))?;
                ai_store::set_provider_compat(tx, provider_id, &compat)?;
            }
        }
        if let Some(compat) = compat {
            ai_store::set_provider_compat(tx, provider_id, &compat)?;
        }
        let page = crate::queries::ai_page(tx, provider_id)?;
        Ok(page)
    })
}

/// 用途只能选加进来的模型，对话默认还须用于对话；`provider_id` 为 0 表示不选（用途暂停）。
fn bind_task(
    conn: &Connection,
    task: &str,
    provider_id: i64,
    model: &str,
) -> rrss_core::Result<()> {
    if task == ai_store::TASK_CHAT && pi_store::use_own_config(conn)? {
        return Err(rrss_core::Error::InvalidProvider(
            "对话模型由自有 Pi 配置决定".into(),
        ));
    }
    let model = model.trim();
    if provider_id == 0 && model.is_empty() {
        return ai_store::set_task_binding(conn, task, None, None);
    }
    if provider_id <= 0 || model.is_empty() {
        return Err(rrss_core::Error::InvalidModelChoice(
            "请选一个模型，或清空这个用途".into(),
        ));
    }
    rrss_core::model_pool::ensure_selectable(
        conn,
        &rrss_core::model_pool::ModelRef::new(provider_id, model),
        task == ai_store::TASK_CHAT,
    )?;
    ai_store::set_task_binding(conn, task, Some(provider_id), Some(model))
}

pub use crate::web::{collect_article, open_web_link};

pub fn cite_article(conn: &Connection, id: i64) -> rrss_core::Result<CitationView> {
    articles::collect_article(conn, id)?;
    let article = articles::get_article(conn, id)?;
    Ok(CitationView {
        kind: "article".into(),
        article_id: article.id,
        title: article.title,
        feed_title: article.feed_title,
        url: article.canonical_url.unwrap_or_default(),
        excerpt: String::new(),
        block_id: -1,
    })
}

pub fn cite_selection(
    conn: &Connection,
    article_id: i64,
    excerpt: &str,
    block_id: i64,
) -> rrss_core::Result<CitationView> {
    articles::collect_article(conn, article_id)?;
    let article = articles::get_article(conn, article_id)?;
    Ok(CitationView {
        kind: "span".into(),
        article_id: article.id,
        title: article.title,
        feed_title: article.feed_title,
        url: article.canonical_url.unwrap_or_default(),
        excerpt: excerpt.chars().take(500).collect(),
        block_id,
    })
}

pub fn import_opml(conn: &Connection, xml: &str) -> rrss_core::Result<opml::ImportReport> {
    opml::import_opml(conn, xml)
}

pub fn export_opml(conn: &Connection) -> rrss_core::Result<String> {
    opml::export_opml(conn)
}

fn ordinary_thread(conn: &Connection, thread_id: i64) -> anyhow::Result<threads::Thread> {
    let thread = threads::get_thread(conn, thread_id)?;
    anyhow::ensure!(thread.anchor_kind.is_none(), "简报等锚定会话不支持此操作");
    Ok(thread)
}

fn update_thread(
    conn: &Connection,
    thread_id: i64,
    sql: &str,
    value: impl rusqlite::ToSql,
) -> anyhow::Result<()> {
    let tx = db::write_transaction(conn)?;
    ordinary_thread(&tx, thread_id)?;
    tx.execute(sql, rusqlite::params![value, thread_id])?;
    tx.commit()?;
    Ok(())
}

pub fn rename_thread(conn: &Connection, thread_id: i64, title: &str) -> anyhow::Result<()> {
    let title = title.trim();
    anyhow::ensure!(title.chars().count() <= 80, "会话标题最长 80 字");
    update_thread(
        conn,
        thread_id,
        "UPDATE threads SET title=?1 WHERE id=?2",
        (!title.is_empty()).then_some(title),
    )
}

pub fn set_thread_folder(
    conn: &Connection,
    thread_id: i64,
    folder_id: Option<i64>,
) -> anyhow::Result<()> {
    let tx = db::write_transaction(conn)?;
    ordinary_thread(&tx, thread_id)?;
    if let Some(id) = folder_id {
        tx.query_row(
            "SELECT id FROM thread_folders WHERE id=?1",
            [id],
            |_| Ok(()),
        )?;
    }
    tx.execute(
        "UPDATE threads SET folder_id=?1 WHERE id=?2",
        rusqlite::params![folder_id, thread_id],
    )?;
    tx.commit()?;
    Ok(())
}

pub fn set_thread_pinned(conn: &Connection, thread_id: i64, pinned: bool) -> anyhow::Result<()> {
    update_thread(
        conn,
        thread_id,
        "UPDATE threads SET pinned=?1 WHERE id=?2",
        pinned,
    )
}

pub fn archive_thread(conn: &Connection, thread_id: i64) -> anyhow::Result<()> {
    update_thread(
        conn,
        thread_id,
        "UPDATE threads SET archived_at=?1 WHERE id=?2",
        feeds::now_ms(),
    )
}

pub fn unarchive_thread(conn: &Connection, thread_id: i64) -> anyhow::Result<()> {
    update_thread(
        conn,
        thread_id,
        "UPDATE threads SET archived_at=?1 WHERE id=?2",
        None::<i64>,
    )
}

/// 调用方在整个删除期间持有 ChatRuntime 的删除守卫，防止同时发送。
pub fn delete_thread(conn: &Connection, thread_id: i64, data_dir: &Path) -> anyhow::Result<()> {
    let tx = db::write_transaction(conn)?;
    let thread = ordinary_thread(&tx, thread_id)?;
    // 所有应用连接均开启 foreign_keys，messages 通过 ON DELETE CASCADE 删除。
    tx.execute("DELETE FROM threads WHERE id=?1", [thread_id])?;
    if let Some(file) = thread.pi_session_file {
        let sessions = crate::pi::workspace::WorkspacePaths::new(data_dir).sessions;
        // canonicalize 同时挡住 .. 和指向托管目录外的符号链接；文件不存在视为已清理。
        let canonical = |path: &Path| -> std::io::Result<Option<std::path::PathBuf>> {
            match path.canonicalize() {
                Ok(path) => Ok(Some(path)),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(err) => Err(err),
            }
        };
        let remove = || -> std::io::Result<()> {
            let file = Path::new(&file);
            if let (Some(root), Some(path), Some(parent)) = (
                canonical(&sessions)?,
                canonical(file)?,
                file.parent().map(canonical).transpose()?.flatten(),
            ) && path.starts_with(&root)
                && path != root
                && parent.starts_with(&root)
            {
                match std::fs::remove_file(file) {
                    Err(err) if err.kind() != std::io::ErrorKind::NotFound => return Err(err),
                    _ => {}
                }
            }
            Ok(())
        };
        remove().map_err(|err| {
            anyhow::anyhow!(
                "删除会话文件失败：{}",
                rrss_core::logsafe::text(&err.to_string(), 200)
            )
        })?;
    }
    tx.commit()?;
    Ok(())
}

fn thread_folder_name(name: &str) -> anyhow::Result<&str> {
    let name = name.trim();
    anyhow::ensure!(
        !name.is_empty() && name.chars().count() <= 40,
        "文件夹名称须为 1–40 字"
    );
    Ok(name)
}

pub fn create_thread_folder(
    conn: &Connection,
    name: &str,
) -> anyhow::Result<crate::views::ThreadFolderView> {
    let name = thread_folder_name(name)?;
    let tx = db::write_transaction(conn)?;
    tx.execute(
        "INSERT INTO thread_folders(name, position, created_at)
        SELECT ?1, COALESCE(MAX(position), -1)+1, ?2 FROM thread_folders",
        rusqlite::params![name, feeds::now_ms()],
    )?;
    let id = tx.last_insert_rowid();
    let position = tx.query_row(
        "SELECT position FROM thread_folders WHERE id=?1",
        [id],
        |r| r.get(0),
    )?;
    tx.commit()?;
    Ok(crate::views::ThreadFolderView {
        id,
        name: name.into(),
        position,
        collapsed: false,
    })
}

pub fn rename_thread_folder(conn: &Connection, folder_id: i64, name: &str) -> anyhow::Result<()> {
    let name = thread_folder_name(name)?;
    let n = conn.execute(
        "UPDATE thread_folders SET name=?1 WHERE id=?2",
        rusqlite::params![name, folder_id],
    )?;
    anyhow::ensure!(n > 0, "会话文件夹不存在");
    Ok(())
}

pub fn delete_thread_folder(conn: &Connection, folder_id: i64) -> anyhow::Result<()> {
    let tx = db::write_transaction(conn)?;
    // 显式置空，即使调用方的连接未开启外键，也不会留下悬空归类。
    tx.execute(
        "UPDATE threads SET folder_id=NULL WHERE folder_id=?1",
        [folder_id],
    )?;
    let n = tx.execute("DELETE FROM thread_folders WHERE id=?1", [folder_id])?;
    anyhow::ensure!(n > 0, "会话文件夹不存在");
    tx.commit()?;
    Ok(())
}

pub fn reorder_thread_folders(conn: &Connection, ids: &[i64]) -> anyhow::Result<()> {
    let tx = db::write_transaction(conn)?;
    let existing: std::collections::HashSet<_> = crate::queries::thread_folders(&tx)?
        .into_iter()
        .map(|f| f.id)
        .collect();
    let requested: std::collections::HashSet<_> = ids.iter().copied().collect();
    anyhow::ensure!(
        ids.len() == existing.len() && requested == existing,
        "排序必须包含全部会话文件夹且不能重复"
    );
    for (position, id) in ids.iter().enumerate() {
        tx.execute(
            "UPDATE thread_folders SET position=?1 WHERE id=?2",
            rusqlite::params![position as i64, id],
        )?;
    }
    tx.commit()?;
    Ok(())
}

pub fn set_thread_folder_collapsed(
    conn: &Connection,
    folder_id: i64,
    collapsed: bool,
) -> anyhow::Result<()> {
    let n = conn.execute(
        "UPDATE thread_folders SET collapsed=?1 WHERE id=?2",
        rusqlite::params![collapsed, folder_id],
    )?;
    anyhow::ensure!(n > 0, "会话文件夹不存在");
    Ok(())
}

pub fn new_thread(conn: &Connection, title: Option<&str>) -> rrss_core::Result<i64> {
    Ok(threads::create_thread(conn, title)?.id)
}

pub fn insert_user_message(
    conn: &Connection,
    thread_id: i64,
    text: &str,
    citations: &[CitationView],
) -> rrss_core::Result<i64> {
    let snaps: Vec<rrss_core::CitationSnap> = citations
        .iter()
        .map(|c| rrss_core::CitationSnap {
            kind: c.kind.clone(),
            article_id: c.article_id,
            title: c.title.clone(),
            feed_title: c.feed_title.clone(),
            url: c.url.clone(),
            excerpt: c.excerpt.clone(),
            block_id: c.block_id,
        })
        .collect();
    let json = if snaps.is_empty() {
        None
    } else {
        serde_json::to_string(&snaps).ok()
    };
    Ok(threads::insert_message(conn, thread_id, MessageRole::User, text, json.as_deref())?.id)
}

pub fn toggle_pi_tool(conn: &Connection, name: &str, enabled: bool) -> rrss_core::Result<()> {
    pi_store::set_tool_enabled(conn, name, enabled)
}

/// 改写工具描述；空白恢复随应用提供的默认描述。描述进入工具清单，下次发送时生效。
pub fn set_pi_tool_description(
    conn: &Connection,
    name: &str,
    description: &str,
) -> rrss_core::Result<()> {
    let description = description.trim();
    if description.is_empty() {
        pi_store::restore_tool_description(conn, name)
    } else {
        pi_store::set_tool_description(conn, name, description)
    }
}

/// 新建或覆盖一个用户技能；新建的技能默认启用。名称与描述按 Pi 的技能规则校验。
pub fn save_skill(
    conn: &Connection,
    data_dir: &std::path::Path,
    draft: &crate::views::SkillDraft,
) -> anyhow::Result<()> {
    let name = draft.name.trim();
    let existed = crate::pi::workspace::WorkspacePaths::new(data_dir)
        .user_skill(name)
        .join("SKILL.md")
        .is_file();
    crate::pi::launch::write_user_skill(data_dir, name, draft.description.trim(), &draft.body)?;
    if !existed {
        toggle_skill(conn, name, true)?;
    }
    Ok(())
}

/// 删除用户技能并移出启用列表；随应用提供的技能不能删除。
pub fn delete_skill(
    conn: &Connection,
    data_dir: &std::path::Path,
    name: &str,
) -> anyhow::Result<()> {
    // 名称来自 IPC，先按技能规则校验，避免拼出技能目录以外的路径。
    crate::pi::launch::validate_skill_name(name)?;
    crate::pi::launch::delete_user_skill(data_dir, name)?;
    toggle_skill(conn, name, false)?;
    Ok(())
}

/// Pi 状态区的复合保存：路径 / 空闲超时 / 思考等级同一事务，思考等级非法时整体不写。
pub fn save_pi_settings(
    conn: &Connection,
    data_dir: &std::path::Path,
    args: &SavePiSettingsArgs,
) -> rrss_core::Result<crate::views::PiPage> {
    let tx = db::write_transaction(conn)?;
    db::set_setting(&tx, ai_store::PI_BINARY_KEY, args.pi_path.trim())?;
    let page = crate::queries::pi_page(&tx, data_dir);
    tx.commit()?;
    Ok(page)
}

pub fn toggle_skill(conn: &Connection, name: &str, enabled: bool) -> rrss_core::Result<()> {
    let mut names = pi_store::enabled_skill_names(conn)?;
    if enabled {
        if !names.iter().any(|n| n == name) {
            names.push(name.to_string());
        }
    } else {
        names.retain(|n| n != name);
    }
    pi_store::set_enabled_skill_names(conn, &names)
}

pub fn reset_failed_embeds(conn: &Connection) -> rrss_core::Result<u32> {
    let reset = rag::reset_failed_chunks(conn)?;
    if let Some(path) = conn.path() {
        let capacity =
            crate::scheduler::model_capacity::Capacity::for_database(std::path::Path::new(path));
        capacity.retries.lock().unwrap().reset_embeddings();
        capacity
            .retry_revision
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
    Ok(reset)
}

/// 存储页的清理与压缩。`action` 取自 [`crate::views::StorageItemView::action`]；返回给用户看的结果。
pub fn storage_clean(
    conn: &Connection,
    db_path: &std::path::Path,
    action: &str,
) -> rrss_core::Result<String> {
    if action == "vacuum" {
        storage::vacuum(conn)?;
        return Ok("数据库已压缩。".into());
    }
    if action == "logs" {
        crate::log::current()
            .ok_or_else(|| std::io::Error::other("logging is not initialized"))?
            .clear()?;
        return Ok("运行日志已清空，之后继续记录。".into());
    }
    let what =
        Cleanup::parse(action).ok_or_else(|| rrss_core::Error::UnknownCleanup(action.into()))?;
    let data_dir = db_path.parent().unwrap_or(std::path::Path::new("."));
    let model = ai_store::resolve_embed(conn)?.map(|e| e.model);
    let n = storage::clean(conn, data_dir, what, model.as_deref())?;
    let n = crate::queries::storage::fmt_count(n);
    Ok(match what {
        Cleanup::AiCache => format!("已清空 AI 缓存（{n} 条）。"),
        Cleanup::RecommendationLogs => format!("已清空推荐日志（{n} 条）。"),
        Cleanup::Translations => format!("已清空译文缓存（{n} 篇）。"),
        Cleanup::Images => format!("已清空配图缓存（{n} 个文件）。"),
        Cleanup::Videos => format!("已清空音视频缓存（{n} 个文件）。"),
        Cleanup::UntrackedMedia => format!("已清理 {n} 个未登记文件。"),
        Cleanup::OldVectors => format!("已清除 {n} 段旧模型向量，之后会按当前模型重新生成。"),
        Cleanup::LegacyTables => "已删除旧版遗留数据。".into(),
    })
}

/// 写进备份清单的程序版本。
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// 手动完整备份到用户选的位置。
pub fn backup_create(
    conn: &Connection,
    data_dir: &Path,
    dest: &Path,
    include_secrets: bool,
) -> rrss_core::Result<BackupDoneView> {
    let manifest = backup::create(
        conn,
        data_dir,
        dest,
        BackupKind::Manual,
        APP_VERSION,
        include_secrets,
    )?;
    db::set_setting(
        conn,
        crate::queries::LAST_MANUAL_KEY,
        &manifest.created_at.to_string(),
    )?;
    Ok(BackupDoneView {
        path: dest.display().to_string(),
        size: crate::queries::fmt_bytes(storage::file_bytes(dest) as i64),
        summary: format!(
            "{} · {}",
            crate::queries::counts_summary(&manifest.counts),
            if manifest.secrets_included {
                "包含密钥"
            } else {
                "不含密钥，恢复后需重新填写"
            }
        ),
    })
}

/// 恢复第一步：解压到暂存目录并检查，给出备份内容请用户确认。
pub fn backup_inspect(data_dir: &Path, path: &Path) -> rrss_core::Result<RestoreCheckView> {
    let check = backup::inspect(path, data_dir)?;
    Ok(RestoreCheckView {
        path: path.display().to_string(),
        file_name: path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        taken: format!(
            "备份于 {} · 版本 v{}",
            crate::queries::fmt_when(check.manifest.created_at),
            check.schema_version
        ),
        summary: format!(
            "{} · {}",
            crate::queries::counts_summary(&check.counts),
            if check.manifest.secrets_included {
                "包含密钥"
            } else {
                "不含密钥，恢复后需重新填写 API Key 与外部工具变量"
            }
        ),
    })
}

/// 恢复第二步：先把当前数据备份到自动备份里，再登记待办；调用方随后重启应用。
pub fn backup_restore(conn: &Connection, data_dir: &Path, path: &Path) -> rrss_core::Result<()> {
    backup::create_auto(conn, data_dir, BackupKind::BeforeRestore, APP_VERSION)?;
    backup::schedule_restore(data_dir, path)
}

pub fn backup_cancel_restore(data_dir: &Path) -> rrss_core::Result<()> {
    backup::cancel_restore(data_dir)
}

pub fn dismiss_restore_notice(data_dir: &Path) -> rrss_core::Result<()> {
    backup::clear_restore_result(data_dir)
}

pub fn dismiss_auto_summary_notice(conn: &Connection) -> rrss_core::Result<()> {
    db::set_setting(conn, ai_store::FOCUS_AUTO_SUMMARY_NOTICE_KEY, "done")
}

/// 设置选项注册表的通用写入：按描述符校验，返回规范化后的值。
pub fn set_setting(
    conn: &Connection,
    id: &str,
    value: &str,
) -> rrss_core::Result<crate::views::SettingValue> {
    crate::settings_schema::set(conn, id, value)
}

pub fn set_backup_schedule(conn: &Connection, value: &str) -> rrss_core::Result<()> {
    if !matches!(value, "off" | "daily" | "weekly") {
        return Err(rrss_core::Error::Backup(format!(
            "unknown backup schedule: {value}"
        )));
    }
    db::set_setting(conn, crate::queries::SCHEDULE_KEY, value)
}

/// 定时备份到期就生成一份；未开启或未到期时返回 `None`。刷新后的整理作业调用。
pub fn run_scheduled_backup(
    conn: &Connection,
    data_dir: &Path,
    now_ms: i64,
) -> rrss_core::Result<Option<std::path::PathBuf>> {
    const DAY: i64 = 24 * 3600 * 1000;
    // 刷新时间有抖动，提前一小时也算到期，免得每天的备份一天天往后推。
    const SLACK: i64 = 3600 * 1000;
    let every = match crate::queries::backup_schedule(conn)?.as_str() {
        "daily" => DAY,
        "weekly" => 7 * DAY,
        _ => return Ok(None),
    };
    if let Some(last) = backup::latest_auto(data_dir, BackupKind::Scheduled)
        && now_ms - last < every - SLACK
    {
        return Ok(None);
    }
    backup::create_auto(conn, data_dir, BackupKind::Scheduled, APP_VERSION).map(Some)
}

pub fn set_retention_days(conn: &Connection, days: i64) -> rrss_core::Result<()> {
    rrss_core::retention::set_global_days(conn, days)
}

const RETENTION_LAST_RUN: &str = "retention_last_run";
const SIGNALS_LAST_RUN: &str = "signals_retention_last_run";

/// 执行一轮保留清理：第一次清理前先自动备份，之后每批一个写事务。
/// `force` 为假时一天最多一次（刷新后的整理作业调用）；返回清理篇数与是否做了备份。
pub fn run_retention(
    conn: &Connection,
    data_dir: &Path,
    now_ms: i64,
    force: bool,
) -> rrss_core::Result<(i64, bool)> {
    use rrss_core::retention;
    const DAY: i64 = 24 * 3600 * 1000;
    let last_signals_run =
        db::get_setting(conn, SIGNALS_LAST_RUN)?.and_then(|v| v.parse::<i64>().ok());
    if force || last_signals_run.is_none_or(|last| now_ms.saturating_sub(last) >= DAY - 3600 * 1000)
    {
        rrss_core::signals::prune(conn, now_ms)?;
        db::set_setting(conn, SIGNALS_LAST_RUN, &now_ms.to_string())?;
    }
    let expired = retention::delete_expired(conn, now_ms)?;
    if !retention::active(conn)? {
        return Ok((expired, false));
    }
    if !force {
        let last = db::get_setting(conn, RETENTION_LAST_RUN)?
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(0);
        if now_ms - last < DAY - 3600 * 1000 {
            return Ok((expired, false));
        }
    }
    let mut backed_up = false;
    if !retention::ever_pruned(conn)? && !retention::due_batch(conn, now_ms, 1)?.is_empty() {
        backup::create_auto(conn, data_dir, BackupKind::BeforePrune, APP_VERSION)?;
        backed_up = true;
    }
    let mut total = expired;
    loop {
        let tx = db::write_transaction(conn)?;
        let ids = retention::due_batch(&tx, now_ms, retention::BATCH)?;
        if ids.is_empty() {
            break;
        }
        total += retention::prune_articles(&tx, &ids, now_ms)?;
        tx.commit()?;
        // 批与批之间让一下写锁：等锁的刷新按退避间隔重试，紧接着开下一个事务它很难插进来。
        std::thread::sleep(std::time::Duration::from_millis(120));
    }
    db::set_setting(conn, RETENTION_LAST_RUN, &now_ms.to_string())?;
    Ok((total, backed_up))
}

/// 存储页「立即清理」。
pub fn retention_clean_now(conn: &Connection, data_dir: &Path) -> rrss_core::Result<String> {
    let (n, backed_up) = run_retention(conn, data_dir, rrss_core::feeds::now_ms(), true)?;
    Ok(format!(
        "{}已清理 {} 篇旧文章的正文。可以压缩数据库把空间还给磁盘。",
        if backed_up {
            "已先自动备份。"
        } else {
            ""
        },
        crate::queries::fmt_count(n)
    ))
}

/// 只导出偏好（规则、兴趣、记忆），不含密钥。
pub fn prefs_export(conn: &Connection, parts: prefs::Parts) -> rrss_core::Result<String> {
    let export = prefs::export(conn, parts)?;
    serde_json::to_string_pretty(&export).map_err(|e| rrss_core::Error::Backup(e.to_string()))
}

/// 合并导入偏好；规则变化后重算可见列表。
pub fn prefs_import(conn: &Connection, text: &str) -> rrss_core::Result<PrefsImportView> {
    let export = prefs::parse(text)?;
    let r = persist_then_rescore(conn, |c| prefs::import(c, &export))?;
    let mut parts = Vec::new();
    let dup = |n: u32, unit: &str| {
        if n > 0 {
            format!("，跳过重复 {n} {unit}")
        } else {
            String::new()
        }
    };
    if export.rules.is_some() {
        parts.push(format!(
            "规则 新增 {} 条{}",
            r.rules_added,
            dup(r.rules_duplicate, "条")
        ));
    }
    if r.packs_added > 0 {
        parts.push(format!("预设包 新增 {} 个", r.packs_added));
    }
    if export.interests.is_some() {
        parts.push(format!(
            "兴趣 新增 {} 个{}",
            r.interests_added,
            dup(r.interests_duplicate, "个")
        ));
    }
    if export.memories.is_some() {
        parts.push(format!(
            "记忆 新增 {} 条{}",
            r.memories_added,
            dup(r.memories_duplicate, "条")
        ));
    }
    Ok(PrefsImportView {
        summary: if parts.is_empty() {
            "文件里没有可导入的内容。".into()
        } else {
            parts.join(" · ")
        },
        skipped: r.skipped,
    })
}

pub fn set_vector_dims(
    conn: &Connection,
    value: &str,
) -> rrss_core::Result<crate::views::VectorSpaceStatusView> {
    use rrss_core::vector_space as spaces;
    let value = value.trim();
    spaces::resolve_dims(value, 0)?;
    db::atomic_write(conn, |conn| {
        let active = spaces::active(conn)?;
        if active.as_ref().is_some_and(|s| s.state == "stale") {
            return Err(rrss_core::Error::InvalidSetting(
                "向量库已停写，请先恢复原配置或重建，再改维度。".into(),
            ));
        }
        if spaces::building(conn)?.is_some() {
            return Err(rrss_core::Error::VectorSpaceRebuildInProgress);
        }
        db::set_setting(conn, spaces::DIMS_SETTING, value)?;
        if let Some(active) = active {
            let dims = active.target_dims(value)?;
            if dims as i64 != active.dims {
                let id = spaces::prepare_dims_rebuild(conn, &active, dims)?;
                spaces::finish_rebuild(conn, id)?;
            }
        }
        crate::queries::vector_space_status(conn)
    })
}

pub fn cancel_vector_rebuild(
    conn: &Connection,
) -> rrss_core::Result<crate::views::VectorSpaceStatusView> {
    crate::configuration::write(conn, |conn| {
        rrss_core::vector_space::cancel_rebuild(conn)?;
        crate::queries::vector_space_status(conn)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rrss_core::db;

    fn open_temp() -> (tempfile::TempDir, rrss_core::db::PooledConnection) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        let conn = db::open_at(&path).unwrap();
        (dir, conn)
    }

    #[test]
    fn thread_folder_deletion_unfiles_threads_even_without_foreign_keys() {
        let dir = tempfile::tempdir().unwrap();
        let conn = db::open_at(&dir.path().join("t.db")).unwrap();
        let id = new_thread(&conn, None).unwrap();
        let folder = create_thread_folder(&conn, "  工作  ").unwrap();
        assert_eq!(folder.name, "工作");
        assert!(create_thread_folder(&conn, "  ").is_err());
        assert!(rename_thread_folder(&conn, folder.id, &"字".repeat(41)).is_err());
        rename_thread_folder(&conn, folder.id, "  资料  ").unwrap();
        set_thread_folder_collapsed(&conn, folder.id, true).unwrap();
        let folders = crate::queries::thread_folders(&conn).unwrap();
        assert_eq!(folders[0].name, "资料");
        assert!(folders[0].collapsed);
        set_thread_folder(&conn, id, Some(folder.id)).unwrap();
        assert_eq!(
            threads::get_thread(&conn, id).unwrap().folder_id,
            Some(folder.id)
        );
        assert!(set_thread_folder(&conn, id, Some(folder.id + 1)).is_err());
        conn.pragma_update(None, "foreign_keys", "OFF").unwrap();
        delete_thread_folder(&conn, folder.id).unwrap();
        assert_eq!(threads::get_thread(&conn, id).unwrap().folder_id, None);
        assert!(crate::queries::thread_folders(&conn).unwrap().is_empty());
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
    }

    #[test]
    fn archived_threads_are_separate_and_sorted_by_archive_time() {
        let dir = tempfile::tempdir().unwrap();
        let conn = db::open_at(&dir.path().join("t.db")).unwrap();
        let a = new_thread(&conn, None).unwrap();
        let b = new_thread(&conn, None).unwrap();
        for id in [a, b] {
            insert_user_message(&conn, id, "first", &[]).unwrap();
            archive_thread(&conn, id).unwrap();
        }
        conn.execute("UPDATE threads SET archived_at=1 WHERE id=?1", [b])
            .unwrap();
        let list = crate::queries::thread_list(&conn).unwrap();
        assert!(list.threads.is_empty());
        assert_eq!(list.archived_count, 2);
        assert_eq!(
            crate::queries::archived_threads(&conn)
                .unwrap()
                .iter()
                .map(|t| t.id)
                .collect::<Vec<_>>(),
            [a, b]
        );
        unarchive_thread(&conn, a).unwrap();
        assert_eq!(crate::queries::threads(&conn).unwrap()[0].id, a);
        assert_eq!(
            crate::queries::thread_list(&conn).unwrap().archived_count,
            1
        );
    }

    #[test]
    fn thread_rename_trims_counts_characters_and_empty_restores_automatic_title() {
        let dir = tempfile::tempdir().unwrap();
        let conn = db::open_at(&dir.path().join("t.db")).unwrap();
        let id = new_thread(&conn, None).unwrap();
        insert_user_message(&conn, id, "自动标题", &[]).unwrap();
        let title = "字".repeat(80);
        rename_thread(&conn, id, &format!("  {title}  ")).unwrap();
        assert_eq!(crate::queries::threads(&conn).unwrap()[0].title, title);
        assert!(rename_thread(&conn, id, &"字".repeat(81)).is_err());
        rename_thread(&conn, id, "新对话").unwrap();
        assert_eq!(crate::queries::threads(&conn).unwrap()[0].title, "新对话");
        rename_thread(&conn, id, "  ").unwrap();
        assert_eq!(threads::get_thread(&conn, id).unwrap().title, None);
        assert_eq!(crate::queries::threads(&conn).unwrap()[0].title, "自动标题");
    }

    #[test]
    fn thread_folder_order_requires_every_id_once() {
        let dir = tempfile::tempdir().unwrap();
        let conn = db::open_at(&dir.path().join("t.db")).unwrap();
        let a = create_thread_folder(&conn, "A").unwrap().id;
        let b = create_thread_folder(&conn, "B").unwrap().id;
        for ids in [vec![a], vec![a, a], vec![a, b + 1]] {
            assert!(reorder_thread_folders(&conn, &ids).is_err());
            assert_eq!(crate::queries::thread_folders(&conn).unwrap()[0].id, a);
        }
        reorder_thread_folders(&conn, &[b, a]).unwrap();
        let folders = crate::queries::thread_folders(&conn).unwrap();
        assert_eq!(
            folders
                .iter()
                .map(|f| (f.id, f.position))
                .collect::<Vec<_>>(),
            [(b, 0), (a, 1)]
        );
    }

    #[test]
    fn anchored_threads_reject_all_ordinary_thread_actions() {
        let dir = tempfile::tempdir().unwrap();
        let conn = db::open_at(&dir.path().join("t.db")).unwrap();
        let id = threads::get_or_create_anchored_thread(&conn, "追问", "digest", 1)
            .unwrap()
            .id;
        assert!(rename_thread(&conn, id, "标题").is_err());
        assert!(set_thread_folder(&conn, id, None).is_err());
        assert!(set_thread_pinned(&conn, id, true).is_err());
        assert!(archive_thread(&conn, id).is_err());
        assert!(unarchive_thread(&conn, id).is_err());
        assert!(delete_thread(&conn, id, dir.path()).is_err());
        assert!(threads::get_thread(&conn, id).is_ok());
    }

    #[test]
    fn thread_deletion_only_removes_managed_session_files() {
        let dir = tempfile::tempdir().unwrap();
        let conn = db::open_at(&dir.path().join("t.db")).unwrap();
        let sessions = crate::pi::workspace::WorkspacePaths::new(dir.path()).sessions;
        std::fs::create_dir_all(&sessions).unwrap();
        let outside = dir.path().join("outside.jsonl");
        std::fs::write(&outside, "fixture").unwrap();
        // 只有 Unix 会追加符号链接一项。
        #[cfg_attr(not(unix), allow(unused_mut))]
        let mut files = vec![
            outside.clone(),
            sessions.join("../outside.jsonl"),
            sessions.join("missing.jsonl"),
        ];
        std::fs::write(&files[1], "fixture").unwrap();
        #[cfg(unix)]
        {
            let symlink = sessions.join("linked.jsonl");
            std::os::unix::fs::symlink(&outside, &symlink).unwrap();
            files.push(symlink);
        }
        for file in files {
            let id = new_thread(&conn, None).unwrap();
            threads::set_pi_session_file(&conn, id, Some(file.to_str().unwrap())).unwrap();
            delete_thread(&conn, id, dir.path()).unwrap();
        }
        assert!(outside.exists());
        assert!(sessions.join("../outside.jsonl").exists());
        let blocked = sessions.join("not-a-file");
        std::fs::create_dir(&blocked).unwrap();
        let id = new_thread(&conn, None).unwrap();
        threads::set_pi_session_file(&conn, id, Some(blocked.to_str().unwrap())).unwrap();
        assert!(delete_thread(&conn, id, dir.path()).is_err());
        assert!(
            threads::get_thread(&conn, id).is_ok(),
            "file failure rolls back the deletion"
        );
    }

    #[test]
    fn memory_conversion_rejects_repeats_but_keeps_distinct_sources() {
        let (_dir, conn) = open_temp();
        let first = memories::insert_pending_fact(&conn, "local first").unwrap();
        let second = memories::insert_pending_fact(&conn, "local first").unwrap();
        let interest = memory_as_interest(&conn, first).unwrap();
        assert!(!memories::get_memory(&conn, first).unwrap().pending());
        assert!(memory_as_interest(&conn, first).is_err());
        let another = memory_as_interest(&conn, second).unwrap();
        assert_ne!(interest, another);
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM interests WHERE source='from_chat'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn memory_conversion_rolls_back_both_writes_on_failure() {
        for trigger in [
            "CREATE TRIGGER fail_conversion BEFORE INSERT ON interests BEGIN SELECT RAISE(ABORT, 'fixture insert failure'); END",
            "CREATE TRIGGER fail_conversion BEFORE UPDATE ON memories BEGIN SELECT RAISE(ABORT, 'fixture confirm failure'); END",
        ] {
            let (_dir, conn) = open_temp();
            let id = memories::insert_pending_fact(&conn, "local first").unwrap();
            conn.execute_batch(trigger).unwrap();
            assert!(memory_as_interest(&conn, id).is_err());
            assert!(memories::get_memory(&conn, id).unwrap().pending());
            let count: i64 = conn
                .query_row("SELECT COUNT(*) FROM interests", [], |r| r.get(0))
                .unwrap();
            assert_eq!(count, 0);
            conn.execute_batch("DROP TRIGGER fail_conversion").unwrap();
            assert!(memory_as_interest(&conn, id).is_ok());
        }
    }

    #[test]
    fn memory_conversion_serializes_competing_requests() {
        let (dir, conn) = open_temp();
        let id = memories::insert_pending_fact(&conn, "local first").unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let handles: Vec<_> = (0..2)
            .map(|_| {
                let path = dir.path().join("t.db");
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    let conn = db::open_at(&path).unwrap();
                    barrier.wait();
                    memory_as_interest(&conn, id).is_ok()
                })
            })
            .collect();
        let succeeded = handles
            .into_iter()
            .filter_map(|h| h.join().unwrap().then_some(()))
            .count();
        assert_eq!(succeeded, 1);
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM interests", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    fn add_provider(conn: &Connection) -> rrss_core::Result<i64> {
        ai_store::insert_provider(
            conn,
            &NewProvider {
                name: "新端点".into(),
                base_url: "http://127.0.0.1:11434/v1".into(),
                api_key: None,
                protocol: "chat".into(),
            },
        )
    }

    #[test]
    fn successful_feedback_events_and_sessions_request_rank_refresh() {
        let (_dir, conn) = open_temp();
        conn.execute_batch(
            "INSERT INTO feeds(id,url,created_at) VALUES(1,'https://rank.invalid/feed',0);
            INSERT INTO articles(id,feed_id,title,created_at) VALUES(1,1,'one',0);",
        )
        .unwrap();
        let pending = || db::get_setting(&conn, "rank_dirty_at").unwrap().is_some();
        rate_article(&conn, 1, true).unwrap();
        assert!(pending());
        conn.execute("DELETE FROM settings WHERE key='rank_dirty_at'", [])
            .unwrap();
        record_article_events(
            &conn,
            vec![crate::views::ArticleEventInput {
                article_id: 1,
                kind: crate::views::SignalEventKind::Ask,
                exposure_id: None,
                at: 1,
            }],
        )
        .unwrap();
        assert!(pending());
        conn.execute("DELETE FROM settings WHERE key='rank_dirty_at'", [])
            .unwrap();
        record_read_sessions(
            &conn,
            vec![crate::views::ReadSessionInput {
                article_id: 1,
                exposure_id: None,
                opened_at: 1,
                active_ms: 1,
                idle_ms: 0,
                read_chars: 0,
                skimmed_chars: 0,
                total_chars: 0,
                max_block: 0,
                reached_end: false,
                translated: false,
                ended_by: crate::views::ReadSessionEndedBy::Back,
                via: crate::views::ReadSessionVia::Focus,
            }],
        )
        .unwrap();
        assert!(pending());
        conn.execute("DELETE FROM settings WHERE key='rank_dirty_at'", [])
            .unwrap();
        assert!(rate_article(&conn, 999, true).is_err());
        assert_eq!(record_article_events(&conn, vec![]).unwrap(), 0);
        assert!(!pending());
    }

    #[test]
    fn translation_secrets_are_write_only_and_restore_markers_clear_on_replacement() {
        use crate::views::KeyState;
        let dir = tempfile::tempdir().unwrap();
        let conn = db::open_at(&dir.path().join("secrets.db")).unwrap();
        let mut input = SaveTranslateArgs {
            enabled: true,
            backend: "http".into(),
            target_lang: "zh".into(),
            deepl_key: Some("deepl-private-3456".into()),
            deepl_url: None,
            deeplx_url: None,
            deeplx_token: Some("deeplx-private-3456".into()),
            http_url: Some("http://127.0.0.1:5000".into()),
            http_key: Some("http-private-3456".into()),
        };
        save_translate_settings(&conn, &input).unwrap();
        let view = crate::queries::translate_page(&conn).unwrap();
        assert_eq!(view.http_key.state, KeyState::Set);
        assert_eq!(view.http_key.hint, "http…3456");
        assert!(!serde_json::to_string(&view).unwrap().contains("private"));
        input.http_key = None;
        input.deepl_key = None;
        input.deeplx_token = None;
        input.target_lang = "en".into();
        save_translate_settings(&conn, &input).unwrap();
        assert_eq!(
            translations::http_config(&conn).unwrap().1.as_deref(),
            Some("http-private-3456")
        );
        db::set_setting(&conn, translations::TRANSLATE_HTTP_KEY_KEY, "").unwrap();
        db::set_setting(
            &conn,
            &translations::missing_key(translations::TRANSLATE_HTTP_KEY_KEY),
            "1",
        )
        .unwrap();
        assert_eq!(
            crate::queries::translate_page(&conn)
                .unwrap()
                .http_key
                .state,
            KeyState::Missing
        );
        save_translate_settings(&conn, &input).unwrap();
        assert_eq!(
            crate::queries::translate_page(&conn)
                .unwrap()
                .http_key
                .state,
            KeyState::Missing
        );
        input.http_key = Some("replacement-7890".into());
        save_translate_settings(&conn, &input).unwrap();
        assert_eq!(
            crate::queries::translate_page(&conn).unwrap().http_key.hint,
            "repl…7890"
        );
        input.http_key = Some(String::new());
        save_translate_settings(&conn, &input).unwrap();
        assert_eq!(
            crate::queries::translate_page(&conn)
                .unwrap()
                .http_key
                .state,
            KeyState::None
        );
    }

    #[test]
    fn manual_backup_records_time_and_restore_is_scheduled_after_safety_backup() {
        let (dir, conn) = open_temp();
        let dest = dir.path().join("out/manual.zip");
        let done = backup_create(&conn, dir.path(), &dest, false).unwrap();
        assert!(done.summary.starts_with("文章 0 篇"));
        assert!(
            crate::queries::backup_page(&conn, dir.path())
                .unwrap()
                .last_manual
                .starts_with("上次备份：今天")
        );

        let check = backup_inspect(dir.path(), &dest).unwrap();
        assert_eq!(check.file_name, "manual.zip");
        assert!(
            check
                .taken
                .contains(&format!("版本 v{}", db::SCHEMA_VERSION))
        );
        backup_restore(&conn, dir.path(), &dest).unwrap();
        let entries = backup::list_auto(dir.path());
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].manifest.as_ref().unwrap().kind,
            BackupKind::BeforeRestore
        );
        assert!(dir.path().join("restore-pending.json").is_file());
        backup_cancel_restore(dir.path()).unwrap();
        assert!(!dir.path().join("restore-pending.json").exists());
    }

    #[test]
    fn scheduled_backup_runs_only_when_due() {
        let (dir, conn) = open_temp();
        let now = rrss_core::feeds::now_ms();
        assert!(
            run_scheduled_backup(&conn, dir.path(), now)
                .unwrap()
                .is_none()
        );
        assert!(set_backup_schedule(&conn, "hourly").is_err());
        set_backup_schedule(&conn, "daily").unwrap();
        assert!(
            run_scheduled_backup(&conn, dir.path(), now)
                .unwrap()
                .is_some()
        );
        assert!(
            run_scheduled_backup(&conn, dir.path(), now + 3600 * 1000)
                .unwrap()
                .is_none()
        );
        assert!(
            run_scheduled_backup(&conn, dir.path(), now + 47 * 1800 * 1000)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn signals_retention_is_fixed_and_independent_of_body_retention() {
        let (dir, conn) = open_temp();
        let now = 1_800_000_000_000;
        let cutoff = now - rrss_core::signals::RETENTION_MS;
        conn.execute_batch("INSERT INTO feeds(id,url,created_at) VALUES(1,'https://test.invalid/rss',0);
            INSERT INTO articles(id,feed_id,title,content_html,created_at) VALUES(1,1,'t','<p>body</p>',0);").unwrap();
        let insert = |at: i64| {
            conn.execute("INSERT INTO exposures(article_id,surface,position,score,interest_boost,rank_score,variant,session_id,shown_at)
                VALUES(1,'focus',0,0,0,0,'baseline',?1,?2)", rusqlite::params![at.to_string(),at]).unwrap();
            conn.execute(
                "INSERT INTO article_events(article_id,kind,at) VALUES(1,'open',?1)",
                [at],
            )
            .unwrap();
            conn.execute("INSERT INTO read_sessions(article_id,opened_at,active_ms,idle_ms,read_chars,skimmed_chars,total_chars,max_block,reached_end,translated,ended_by,via)
                VALUES(1,?1,0,0,0,0,0,0,0,0,'back','focus')", [at]).unwrap();
        };
        let counts = || {
            ["exposures", "article_events", "read_sessions"].map(|t| {
                conn.query_row(&format!("SELECT COUNT(*) FROM {t}"), [], |r| {
                    r.get::<_, i64>(0)
                })
                .unwrap()
            })
        };
        insert(cutoff - 1);
        insert(cutoff);
        insert(now - 100 * 24 * 3600 * 1000);
        assert_eq!(
            run_retention(&conn, dir.path(), now, false).unwrap(),
            (0, false)
        );
        assert_eq!(counts(), [2; 3]);
        assert!(backup::list_auto(dir.path()).is_empty());
        assert_eq!(
            conn.query_row("SELECT content_html FROM articles", [], |r| r
                .get::<_, String>(0))
                .unwrap(),
            "<p>body</p>"
        );
        insert(cutoff - 2);
        run_retention(&conn, dir.path(), now + 1000, false).unwrap();
        assert_eq!(
            counts(),
            [3; 3],
            "daily throttle applies while body retention is off"
        );
        run_retention(&conn, dir.path(), now, true).unwrap();
        assert_eq!(counts(), [2; 3], "force bypasses log throttle");
        set_retention_days(&conn, 90).unwrap();
        run_retention(&conn, dir.path(), now, true).unwrap();
        assert_eq!(
            counts(),
            [2; 3],
            "body retention does not shorten log retention"
        );
        set_retention_days(&conn, 0).unwrap();
        run_retention(&conn, dir.path(), now + 24 * 3600 * 1000, false).unwrap();
        assert_eq!(
            counts(),
            [1; 3],
            "logs keep pruning after body retention is disabled"
        );
    }

    #[test]
    fn retention_backs_up_once_prunes_in_batches_and_runs_daily() {
        let (dir, conn) = open_temp();
        let now = rrss_core::feeds::now_ms();
        let old = now - 200 * 24 * 3600 * 1000;
        conn.execute(
            "INSERT INTO feeds (id, url, created_at) VALUES (1, 'https://a/rss', 0)",
            [],
        )
        .unwrap();
        for i in 0..(rrss_core::retention::BATCH + 5) {
            conn.execute(
                "INSERT INTO articles (feed_id, guid, title, content_html, created_at) VALUES (1, ?1, 't', '<p>x</p>', ?2)",
                rusqlite::params![format!("g{i}"), old],
            )
            .unwrap();
        }
        assert_eq!(
            run_retention(&conn, dir.path(), now, false).unwrap(),
            (0, false),
            "off by default"
        );
        set_retention_days(&conn, 90).unwrap();
        let page = crate::queries::retention_page(&conn, now).unwrap();
        assert_eq!(page.preview, "将清理 205 篇");
        assert_eq!(page.confirm_ok, "备份并清理");
        assert!(page.confirm[2].contains("第一次清理"));

        let (n, backed_up) = run_retention(&conn, dir.path(), now, false).unwrap();
        assert_eq!((n, backed_up), (rrss_core::retention::BATCH + 5, true));
        assert_eq!(
            backup::list_auto(dir.path())[0]
                .manifest
                .as_ref()
                .unwrap()
                .kind,
            BackupKind::BeforePrune
        );
        conn.execute(
            "INSERT INTO articles (feed_id, guid, title, content_html, created_at) VALUES (1, 'late', 't', '<p>x</p>', ?1)",
            [old],
        )
        .unwrap();
        assert_eq!(
            run_retention(&conn, dir.path(), now + 3600 * 1000, false).unwrap(),
            (0, false),
            "once a day"
        );
        let msg = retention_clean_now(&conn, dir.path()).unwrap();
        assert_eq!(
            msg,
            "已清理 1 篇旧文章的正文。可以压缩数据库把空间还给磁盘。"
        );
        assert_eq!(
            backup::list_auto(dir.path()).len(),
            1,
            "no second pre-prune backup"
        );
        let page = crate::queries::retention_page(&conn, now).unwrap();
        assert!(!page.can_run);
        assert_eq!(page.confirm_ok, "清理");
    }

    #[test]
    fn feed_tree_carries_retention_and_health_and_dismiss_hides_it() {
        let (_dir, conn) = open_temp();
        let day = 86_400_000;
        let now = rrss_core::feeds::now_ms();
        conn.execute(
            "INSERT INTO feeds (id, url, created_at) VALUES (1, 'https://a/rss', ?1)",
            [now - 100 * day],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO feeds (id, url, created_at) VALUES (2, 'https://b/rss', ?1)",
            [now - 100 * day],
        )
        .unwrap();
        conn.execute(
            "UPDATE feeds SET fail_count = 12, failing_since = ?1, error = 'HTTP 404' WHERE id = 1",
            [now - 9 * day],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO articles (feed_id, title, created_at) VALUES (2, 't', ?1)",
            [now - day],
        )
        .unwrap();
        let tree = save_feed(
            &conn,
            &SaveFeedArgs {
                id: 2,
                title: None,
                url: None,
                folder_id: None,
                retention: Some("keep".into()),
            },
        )
        .unwrap();
        let a = find_feed(&tree, 1);
        assert_eq!(a.health, "failing");
        assert_eq!(a.health_label, "连续失败 12 次 · 9 天");
        assert!(
            a.health_detail.contains("还没有成功抓取过"),
            "{}",
            a.health_detail
        );
        let b = find_feed(&tree, 2);
        assert_eq!((b.health.as_str(), b.retention_days), ("", Some(0)));
        assert!(
            save_feed(
                &conn,
                &SaveFeedArgs {
                    id: 2,
                    title: None,
                    url: None,
                    folder_id: None,
                    retention: Some("abc".into())
                }
            )
            .is_err()
        );
        let tree = save_feed(
            &conn,
            &SaveFeedArgs {
                id: 2,
                title: None,
                url: None,
                folder_id: None,
                retention: Some("30".into()),
            },
        )
        .unwrap();
        assert_eq!(find_feed(&tree, 2).retention_days, Some(30));

        let tree = dismiss_feed_health(&conn, 1).unwrap();
        assert_eq!(find_feed(&tree, 1).health, "");
        let tree = unsubscribe_many(&conn, &[1, 2]).unwrap();
        assert!(tree.uncategorized.is_empty());
    }

    #[test]
    fn prefs_import_summarises_and_rescopes() {
        let (_dir, conn) = open_temp();
        conn.execute_batch(
            "INSERT INTO rules (polarity, dimension, pattern, scope_kind, weight, source, enabled, created_at)
             VALUES ('dislike', 'title_kw', '广告', 'global', 10, 'manual', 1, 0);
             INSERT INTO interests (label, weight, source, enabled, created_at) VALUES ('Rust', 2, 'manual', 1, 0);",
        )
        .unwrap();
        let text = prefs_export(
            &conn,
            prefs::Parts {
                rules: true,
                interests: true,
                memories: false,
            },
        )
        .unwrap();
        let (_dir2, other) = open_temp();
        let report = prefs_import(&other, &text).unwrap();
        assert_eq!(report.summary, "规则 新增 1 条 · 兴趣 新增 1 个");
        let again = prefs_import(&other, &text).unwrap();
        assert_eq!(
            again.summary,
            "规则 新增 0 条，跳过重复 1 条 · 兴趣 新增 0 个，跳过重复 1 个"
        );
        assert!(prefs_import(&other, "not json").is_err());
    }

    #[test]
    fn storage_clean_reports_and_rejects_unknown_actions() {
        let (dir, conn) = open_temp();
        let path = dir.path().join("t.db");
        conn.execute(
            "INSERT INTO ai_cache (cache_key, value, created_at) VALUES ('k', 'v', 1)",
            [],
        )
        .unwrap();
        assert_eq!(
            storage_clean(&conn, &path, "ai_cache").unwrap(),
            "已清空 AI 缓存（1 条）。"
        );
        assert_eq!(
            storage_clean(&conn, &path, "recommendation_logs").unwrap(),
            "已清空推荐日志（0 条）。"
        );
        assert_eq!(
            storage_clean(&conn, &path, "vacuum").unwrap(),
            "数据库已压缩。"
        );
        assert!(matches!(
            storage_clean(&conn, &path, "drop_everything"),
            Err(rrss_core::Error::UnknownCleanup(_))
        ));
    }

    #[test]
    fn thread_and_folder_roundtrip() {
        let (_dir, conn) = open_temp();
        let folder = create_folder(&conn, "科技", None).unwrap();
        assert!(folder > 0);
        let tid = new_thread(&conn, Some("新对话")).unwrap();
        assert!(tid > 0);
        let mid = insert_user_message(&conn, tid, "hello", &[]).unwrap();
        assert!(mid > 0);
    }

    #[test]
    fn create_rule_rescores_existing_articles() {
        use crate::views::NewRuleSpec;
        use rrss_core::articles::{self, FocusFilter};
        use rrss_core::feeds::{SubscribeRequest, subscribe};
        use rrss_core::parse::ParsedEntry;

        let (_dir, conn) = open_temp();
        let feed = subscribe(
            &conn,
            SubscribeRequest {
                url: "https://news.example.com/rss.xml".into(),
                folder_id: None,
                as_rsshub: false,
                rsshub_instance_id: None,
            },
        )
        .unwrap();
        rrss_core::ingest::upsert_entries(
            &conn,
            feed,
            &[ParsedEntry {
                guid: "r1".into(),
                canonical_url: Some("https://news.example.com/rust".into()),
                title: "Rust 1.80 shipped".into(),
                author: None,
                published_at: Some(1),
                content_html: Some("<p>release</p>".into()),
                hash: "h".into(),
                tags: vec![],
            }],
            1,
        )
        .unwrap();
        articles::set_focus_filter(&conn, FocusFilter::FocusOnly).unwrap();
        assert!(articles::list_focus(&conn, 20).unwrap().is_empty());

        create_rule(
            &conn,
            &NewRuleSpec {
                polarity: "like".into(),
                dimension: "title_kw".into(),
                pattern: "Rust".into(),
                weight: 10,
                scope_kind: "global".into(),
                scope_id: 0,
            },
        )
        .unwrap();

        let focus = articles::list_focus(&conn, 20).unwrap();
        assert_eq!(focus.len(), 1);
        assert_eq!(focus[0].title, "Rust 1.80 shipped");
        assert_eq!(focus[0].score, 10);
    }

    #[test]
    fn create_rule_rolls_back_when_rescore_fails() {
        use crate::views::NewRuleSpec;
        use rrss_core::feeds::{SubscribeRequest, subscribe};
        use rrss_core::parse::ParsedEntry;
        use rrss_core::rules::{self, RuleFilter};

        let (_dir, conn) = open_temp();
        let feed = subscribe(
            &conn,
            SubscribeRequest {
                url: "https://news.example.com/rss.xml".into(),
                folder_id: None,
                as_rsshub: false,
                rsshub_instance_id: None,
            },
        )
        .unwrap();
        rrss_core::ingest::upsert_entries(
            &conn,
            feed,
            &[ParsedEntry {
                guid: "r1".into(),
                canonical_url: Some("https://news.example.com/rust".into()),
                title: "Rust 1.80 shipped".into(),
                author: None,
                published_at: Some(1),
                content_html: Some("<p>release</p>".into()),
                hash: "h".into(),
                tags: vec![],
            }],
            1,
        )
        .unwrap();
        conn.execute(
            "CREATE TRIGGER fail_score BEFORE UPDATE OF score ON articles
             BEGIN SELECT RAISE(ABORT, 'injected'); END;",
            [],
        )
        .unwrap();

        let err = create_rule(
            &conn,
            &NewRuleSpec {
                polarity: "like".into(),
                dimension: "title_kw".into(),
                pattern: "Rust".into(),
                weight: 10,
                scope_kind: "global".into(),
                scope_id: 0,
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("injected"), "{err}");
        let n = rules::list_rules(&conn, &RuleFilter::default())
            .unwrap()
            .len();
        assert_eq!(n, 0);
    }

    fn ai_args(provider_id: i64) -> crate::views::SaveAiSettingsArgs {
        use crate::views::ProviderInput;
        crate::views::SaveAiSettingsArgs {
            provider: ProviderInput {
                id: provider_id,
                kind: None,
                name: " 云端 ".into(),
                base_url: " https://api.example.com/v1 ".into(),
                api_key: Some("sk-first-0000-1234".into()),
                clear_key: None,
                protocol: "chat".into(),
                compat_json: None,
            },
        }
    }

    #[test]
    fn a_use_can_only_pick_an_added_model_and_a_bad_batch_rolls_back() {
        use crate::views::{SaveModelBindingsArgs, TaskModelInput};
        let (_dir, conn) = open_temp();
        let id = add_provider(&conn).unwrap();
        let input = |task: &str, model: &str| TaskModelInput {
            task: task.into(),
            provider_id: if model.is_empty() { 0 } else { id },
            model: model.into(),
        };
        let save = |bindings: Vec<TaskModelInput>| {
            save_model_bindings(&conn, &SaveModelBindingsArgs { bindings })
        };
        let err = save(vec![input(ai_store::TASK_ARTICLE_TRANSLATE, "not-added")]).unwrap_err();
        assert!(
            matches!(err, rrss_core::Error::InvalidModelChoice(_)),
            "{err}"
        );
        rrss_core::model_pool::update_pool(
            &conn,
            &[
                rrss_core::model_pool::ModelRef::new(id, "text-embedding-3-small"),
                rrss_core::model_pool::ModelRef::new(id, "fast"),
            ],
            &[],
            None,
        )
        .unwrap();
        let page = save(vec![input(ai_store::TASK_ARTICLE_TRANSLATE, "fast")]).unwrap();
        let translate = page
            .tasks
            .iter()
            .find(|t| t.task == ai_store::TASK_ARTICLE_TRANSLATE)
            .unwrap();
        assert_eq!(
            (translate.provider_id, translate.model.as_str()),
            (id, "fast")
        );
        assert!(translate.capabilities.is_some());
        assert_eq!(
            ai_store::resolve_task(&conn, ai_store::TASK_ARTICLE_TRANSLATE)
                .unwrap()
                .model,
            "fast"
        );
        // 嵌入模型没打开「用于对话」，不能当对话默认；整批回滚。
        let err = save(vec![
            input(ai_store::TASK_FOCUS_SUMMARY, "fast"),
            input(ai_store::TASK_CHAT, "text-embedding-3-small"),
        ])
        .unwrap_err();
        assert!(err.to_string().contains("用于对话"), "{err}");
        assert!(ai_store::resolve_task(&conn, ai_store::TASK_FOCUS_SUMMARY).is_err());
        save(vec![input(ai_store::TASK_ARTICLE_TRANSLATE, "")]).unwrap();
        assert!(ai_store::resolve_task(&conn, ai_store::TASK_ARTICLE_TRANSLATE).is_err());
        pi_store::set_use_own_config(&conn, true).unwrap();
        assert!(save(vec![input(ai_store::TASK_CHAT, "fast")]).is_err());
        assert!(
            crate::queries::model_bindings(&conn)
                .unwrap()
                .chat_uses_own_config
        );
    }

    #[test]
    fn pi_config_patch_is_atomic_and_defaults_can_be_restored_without_starting_pi() {
        use crate::views::SavePiConfigArgs;
        let (_dir, conn) = open_temp();
        let mut args = SavePiConfigArgs {
            settings_override: Some(r#"{ "custom": true }"#.into()),
            models_override: Some("{}".into()),
            use_own_config: Some(true),
            own_model: Some("provider/model".into()),
        };
        let view = save_pi_config(&conn, &args).unwrap();
        assert_eq!(view.settings_override, r#"{"custom":true}"#);
        assert!(view.use_own_config);
        args.settings_override = Some("{}".into());
        args.use_own_config = Some(false);
        args.own_model = Some("invalid".into());
        assert!(save_pi_config(&conn, &args).is_err());
        assert_eq!(
            serde_json::to_value(crate::queries::pi_config(&conn).unwrap()).unwrap(),
            serde_json::to_value(&view).unwrap()
        );
        args.own_model = None;
        args.models_override = Some("[]".into());
        assert!(save_pi_config(&conn, &args).is_err());
        args.models_override = Some(" ".into());
        let reset = save_pi_config(&conn, &args).unwrap();
        assert_eq!(reset.settings_override, "{}");
        assert_eq!(reset.models_override, "{}");
        assert!(!reset.use_own_config);
        assert_eq!(reset.own_model, "provider/model", "absent field preserved");
    }

    #[test]
    fn save_ai_settings_creates_presets_atomically_and_preserves_kind_on_legacy_edit() {
        let (_dir, conn) = open_temp();
        let presets = crate::provider_presets::provider_presets();
        assert_eq!(
            presets.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
            [
                "openai",
                "deepseek",
                "openrouter",
                "ollama",
                "openai_compat"
            ]
        );
        for preset in presets {
            let mut args = ai_args(0);
            args.provider.kind = Some(preset.id.clone());
            if !preset.base_url.is_empty() {
                args.provider.base_url = preset.base_url;
            }
            args.provider.protocol = preset.protocol;
            let page = save_ai_settings(&conn, &args).unwrap();
            let id = page.selected_id;
            assert!(id > 0);
            assert_eq!(
                page.providers.iter().find(|p| p.id == id).unwrap().kind,
                preset.id
            );
            assert_eq!(
                ai_store::provider_compat(&conn, id).unwrap().to_string(),
                preset.compat_json
            );
            // 旧前端没有 kind 字段，不应把已选预设改回自定义。
            let compat = serde_json::json!({"supportsDeveloperRole": false});
            ai_store::set_provider_compat(&conn, id, &compat).unwrap();
            save_ai_settings(&conn, &ai_args(id)).unwrap();
            assert_eq!(ai_store::get_provider(&conn, id).unwrap().kind, preset.id);
            assert_eq!(ai_store::provider_compat(&conn, id).unwrap(), compat);
        }
    }

    #[test]
    fn save_ai_settings_updates_compat_with_connection_and_rolls_back_invalid_input() {
        let (_dir, conn) = open_temp();
        let mut args = ai_args(0);
        args.provider.compat_json = Some(r#"{"supportsDeveloperRole":false}"#.into());
        let page = save_ai_settings(&conn, &args).unwrap();
        let id = page.selected_id;
        assert_eq!(
            page.providers[0].compat_json.as_deref(),
            Some(r#"{"supportsDeveloperRole":false}"#)
        );
        assert_eq!(
            ai_store::provider_compat(&conn, id).unwrap(),
            serde_json::json!({"supportsDeveloperRole": false})
        );

        args.provider.id = id;
        args.provider.name = "updated".into();
        for invalid in ["[]", "{", r#"{"secret":"sk-sensitive","x": }"#] {
            args.provider.compat_json = Some(invalid.into());
            let err = save_ai_settings(&conn, &args).unwrap_err();
            assert!(matches!(err, rrss_core::Error::InvalidCompat(_)));
            assert!(!err.to_string().contains("sk-sensitive"));
            assert_eq!(ai_store::get_provider(&conn, id).unwrap().name, "云端");
        }
        args.provider.compat_json = None;
        let page = save_ai_settings(&conn, &args).unwrap();
        assert_eq!(
            page.providers[0].compat_json.as_deref(),
            Some(r#"{"supportsDeveloperRole":false}"#)
        );
        args.provider.compat_json = Some(" ".into());
        let page = save_ai_settings(&conn, &args).unwrap();
        assert_eq!(page.providers[0].compat_json.as_deref(), Some("{}"));
    }

    #[test]
    fn save_ai_settings_rejects_invalid_connection_without_writing_or_leaking_secrets() {
        let (_dir, conn) = open_temp();
        for field in ["kind", "name", "url", "protocol"] {
            let mut args = ai_args(0);
            match field {
                "kind" => args.provider.kind = Some("unknown".into()),
                "name" => args.provider.name = "  ".into(),
                "url" => args.provider.base_url = "file:///secret-key".into(),
                _ => args.provider.protocol = "unknown".into(),
            }
            let err = save_ai_settings(&conn, &args).unwrap_err();
            assert!(matches!(err, rrss_core::error::Error::InvalidProvider(_)));
            assert!(!err.to_string().contains("secret-key"));
            assert!(ai_store::list_providers(&conn).unwrap().is_empty());
            assert_eq!(ai_store::daily_budget_tokens(&conn).unwrap(), 200000);
        }
    }

    #[test]
    fn save_ai_settings_writes_only_the_connection_and_never_returns_the_full_key() {
        let (_dir, conn) = open_temp();
        let pid = add_provider(&conn).unwrap();
        let page = save_ai_settings(&conn, &ai_args(pid)).unwrap();

        assert_eq!(page.selected_id, pid);
        let saved = &page.providers[0];
        assert_eq!(saved.name, "云端");
        assert_eq!(saved.base_url, "https://api.example.com/v1");
        assert_eq!(
            (saved.key, saved.key_hint.as_str()),
            (crate::views::KeyState::Set, "sk-f…1234")
        );
        assert!(!serde_json::to_string(&page).unwrap().contains("sk-first"));
        assert_eq!(ai_store::daily_budget_tokens(&conn).unwrap(), 200000);
        assert!(
            !serde_json::to_value(&page)
                .unwrap()
                .as_object()
                .unwrap()
                .contains_key("tasks")
        );
        assert_eq!(
            ai_store::get_provider(&conn, pid)
                .unwrap()
                .api_key
                .as_deref(),
            Some("sk-first-0000-1234")
        );
    }

    #[test]
    fn save_ai_settings_keeps_replaces_or_clears_the_stored_key() {
        let (_dir, conn) = open_temp();
        let pid = add_provider(&conn).unwrap();
        save_ai_settings(&conn, &ai_args(pid)).unwrap();
        let key = || ai_store::get_provider(&conn, pid).unwrap().api_key;

        let mut args = ai_args(pid);
        args.provider.api_key = Some("   ".into());
        save_ai_settings(&conn, &args).unwrap();
        assert_eq!(key().as_deref(), Some("sk-first-0000-1234"));

        args.provider.api_key = Some("sk-second-9999-5678".into());
        assert_eq!(
            save_ai_settings(&conn, &args).unwrap().providers[0].key_hint,
            "sk-s…5678"
        );
        args.provider.api_key = None;
        args.provider.clear_key = Some(true);
        let page = save_ai_settings(&conn, &args).unwrap();
        assert_eq!(page.providers[0].key, crate::views::KeyState::None);
        assert_eq!(key(), None);
    }

    #[test]
    fn save_ai_settings_rolls_back_a_new_connection_when_a_late_write_fails() {
        let (_dir, conn) = open_temp();
        conn.execute(
            "CREATE TRIGGER reject_compat BEFORE UPDATE OF compat_json ON ai_providers
             BEGIN SELECT RAISE(ABORT, 'injected'); END;",
            [],
        )
        .unwrap();
        let mut args = ai_args(0);
        args.provider.kind = Some("deepseek".into());
        let err = save_ai_settings(&conn, &args).unwrap_err();
        assert!(err.to_string().contains("injected"), "{err}");
        assert!(ai_store::list_providers(&conn).unwrap().is_empty());
        assert_eq!(ai_store::daily_budget_tokens(&conn).unwrap(), 200000);
    }

    #[test]
    fn save_ai_settings_rejects_a_missing_provider_without_writing() {
        let (_dir, conn) = open_temp();
        let err = save_ai_settings(&conn, &ai_args(404)).unwrap_err();
        assert!(
            matches!(err, rrss_core::error::Error::ProviderNotFound),
            "{err}"
        );
        assert_eq!(ai_store::daily_budget_tokens(&conn).unwrap(), 200000);
    }

    fn seed_feed(conn: &Connection, url: &str) -> i64 {
        rrss_core::feeds::subscribe(
            conn,
            SubscribeRequest {
                url: url.into(),
                folder_id: None,
                as_rsshub: false,
                rsshub_instance_id: None,
            },
        )
        .unwrap()
    }

    fn find_feed(tree: &crate::views::FeedTree, id: i64) -> crate::views::FeedView {
        tree.uncategorized
            .iter()
            .chain(tree.folders.iter().flat_map(|f| f.feeds.iter()))
            .find(|f| f.id == id)
            .cloned()
            .expect("feed in tree")
    }

    #[test]
    fn save_feed_writes_title_url_and_folder_together_and_returns_the_tree() {
        let (_dir, conn) = open_temp();
        let id = seed_feed(&conn, "https://news.example.com/a.xml");
        let folder = create_folder(&conn, "科技", None).unwrap();
        let tree = save_feed(
            &conn,
            &SaveFeedArgs {
                id,
                title: Some(" 自定义名 ".into()),
                url: Some("https://news.example.com/b.xml".into()),
                folder_id: Some(folder),
                retention: None,
            },
        )
        .unwrap();
        let row = find_feed(&tree, id);
        assert_eq!(row.title, "自定义名");
        assert_eq!(row.url, "https://news.example.com/b.xml");
        assert_eq!(row.folder_id, folder);
        assert!(tree.uncategorized.iter().all(|f| f.id != id));

        // `None` 字段不动；`Some(0)` 移出文件夹。
        let tree = save_feed(
            &conn,
            &SaveFeedArgs {
                id,
                title: None,
                url: None,
                folder_id: Some(0),
                retention: None,
            },
        )
        .unwrap();
        let row = find_feed(&tree, id);
        assert_eq!(row.title, "自定义名");
        assert_eq!(row.url, "https://news.example.com/b.xml");
        assert_eq!(row.folder_id, 0);
    }

    #[test]
    fn save_feed_rolls_back_title_and_url_when_the_folder_is_missing() {
        let (_dir, conn) = open_temp();
        let id = seed_feed(&conn, "https://news.example.com/a.xml");
        let before = find_feed(&crate::queries::feed_tree(&conn).unwrap(), id);
        let err = save_feed(
            &conn,
            &SaveFeedArgs {
                id,
                title: Some("不该落库".into()),
                url: Some("https://news.example.com/changed.xml".into()),
                folder_id: Some(4040),
                retention: None,
            },
        )
        .unwrap_err();
        assert!(!err.to_string().is_empty());
        let after = find_feed(&crate::queries::feed_tree(&conn).unwrap(), id);
        assert_eq!(after.title, before.title);
        assert_eq!(after.url, before.url);
        assert_eq!(after.folder_id, 0);
    }

    #[test]
    fn save_feed_rolls_back_title_when_the_url_duplicates_another_feed() {
        let (_dir, conn) = open_temp();
        let id = seed_feed(&conn, "https://news.example.com/a.xml");
        seed_feed(&conn, "https://news.example.com/taken.xml");
        let err = save_feed(
            &conn,
            &SaveFeedArgs {
                id,
                title: Some("不该落库".into()),
                url: Some("https://news.example.com/taken.xml".into()),
                folder_id: None,
                retention: None,
            },
        )
        .unwrap_err();
        assert!(
            matches!(err, rrss_core::error::Error::DuplicateFeed(_)),
            "{err}"
        );
        let after = find_feed(&crate::queries::feed_tree(&conn).unwrap(), id);
        assert_ne!(after.title, "不该落库");
        assert_eq!(after.url, "https://news.example.com/a.xml");
    }

    #[test]
    fn save_feed_rejects_a_missing_feed() {
        let (_dir, conn) = open_temp();
        let err = save_feed(
            &conn,
            &SaveFeedArgs {
                id: 77,
                title: Some("x".into()),
                url: None,
                folder_id: None,
                retention: None,
            },
        )
        .unwrap_err();
        assert!(
            matches!(err, rrss_core::error::Error::FeedNotFound),
            "{err}"
        );
    }

    #[test]
    fn save_pi_settings_writes_the_trimmed_path_only() {
        let (dir, conn) = open_temp();
        pi_store::set_idle_timeout_secs(&conn, 60).unwrap();
        let page = save_pi_settings(
            &conn,
            dir.path(),
            &SavePiSettingsArgs {
                pi_path: "  /opt/pi/bin/pi  ".into(),
            },
        )
        .unwrap();
        assert_eq!(page.pi_path, "/opt/pi/bin/pi");
        assert_eq!(pi_store::idle_timeout_secs(&conn).unwrap(), 60);
    }

    #[test]
    fn user_skills_are_enabled_on_creation_and_leave_the_list_on_delete() {
        let (dir, conn) = open_temp();
        let draft = crate::views::SkillDraft {
            name: "notes".into(),
            description: "整理笔记".into(),
            body: "先列要点。".into(),
        };
        save_skill(&conn, dir.path(), &draft).unwrap();
        assert!(
            pi_store::enabled_skill_names(&conn)
                .unwrap()
                .contains(&"notes".into())
        );
        let read = crate::queries::pi_skill(dir.path(), "notes").unwrap();
        assert_eq!(
            (read.description.as_str(), read.body.as_str()),
            ("整理笔记", "先列要点。")
        );
        // 用户停用后再编辑，不会被悄悄重新启用。
        toggle_skill(&conn, "notes", false).unwrap();
        save_skill(
            &conn,
            dir.path(),
            &crate::views::SkillDraft {
                body: "改过".into(),
                ..draft
            },
        )
        .unwrap();
        assert!(
            !pi_store::enabled_skill_names(&conn)
                .unwrap()
                .contains(&"notes".into())
        );
        delete_skill(&conn, dir.path(), "notes").unwrap();
        assert!(crate::queries::pi_skill(dir.path(), "notes").is_err());
        std::fs::create_dir_all(dir.path().join("keep")).unwrap();
        assert!(delete_skill(&conn, dir.path(), "../../keep").is_err());
        assert!(dir.path().join("keep").is_dir());
        assert!(
            save_skill(
                &conn,
                dir.path(),
                &crate::views::SkillDraft {
                    name: "Bad Name".into(),
                    description: "x".into(),
                    body: String::new(),
                }
            )
            .is_err()
        );
    }

    #[test]
    fn blank_tool_description_restores_the_default() {
        let (_dir, conn) = open_temp();
        let tool = pi_store::list_tools(&conn).unwrap().remove(0);
        set_pi_tool_description(&conn, &tool.name, " 我的说明 ").unwrap();
        let edited = pi_store::list_tools(&conn).unwrap().remove(0);
        assert_eq!(edited.description, "我的说明");
        set_pi_tool_description(&conn, &tool.name, "  ").unwrap();
        let restored = pi_store::list_tools(&conn).unwrap().remove(0);
        assert_eq!(restored.description, tool.default_description);
    }
}
