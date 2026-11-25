//! Application write orchestration. Domain modules are re-exported here.

mod feeds;
pub use feeds::{
    add_feed, create_folder, delete_folder, dismiss_feed_health, export_opml, import_opml,
    move_tree_items, rename_folder, restore_tree_layout, save_feed, unsubscribe, unsubscribe_many,
};

mod reading_activity;
pub use reading_activity::{record_article_events, record_exposures, record_read_sessions};

mod rules;
pub use rules::{
    allow_rule_for_feed, apply_preset, confirm_feature, confirm_suggested_rule, create_rule,
    discard_rule, release_article, remove_preset, rescore, set_rule_action, unrelease_article,
    update_rule,
};

mod storage;
pub use storage::{
    APP_VERSION, backup_cancel_restore, backup_create, backup_inspect, backup_restore,
    dismiss_restore_notice, retention_clean_now, run_retention, run_scheduled_backup,
    set_backup_schedule, set_retention_days, storage_clean,
};

mod threads;
pub use threads::{
    archive_thread, create_thread_folder, delete_thread, delete_thread_folder, insert_user_message,
    new_thread, rename_thread, rename_thread_folder, reorder_thread_folders, set_thread_folder,
    set_thread_folder_collapsed, set_thread_pinned, unarchive_thread,
};

use rrss_core::ai_store::{AiProvider, NewProvider};
use rrss_core::articles;
use rrss_core::error::{
    backup::BackupProblem,
    models::{CompatProblem, ModelChoiceProblem, ProviderProblem},
    rules::MemoryProblem,
    settings::SettingProblem,
};
use rrss_core::feed_scope::{self, FeedScope};
use rrss_core::feedback::{self, FeedbackKind};
use rrss_core::interests;
use rrss_core::memories;
use rrss_core::pi_store;
use rrss_core::prefs;
use rrss_core::rag;
use rrss_core::timeline;
use rrss_core::translations;
use rrss_core::{ai_store, db, rescore_all};
use rusqlite::Connection;
use std::path::Path;

use crate::views::{
    CitationView, PrefsImportView, PrefsScopeView, PrefsSkipView, RatingUndoView,
    SaveAiSettingsArgs, SavePiSettingsArgs, UiError,
};

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
    interests::set_enabled(conn, id, enabled)?;
    interest_changed(conn);
    Ok(())
}

pub fn delete_interest(conn: &Connection, id: i64) -> rrss_core::Result<()> {
    interests::delete_interest(conn, id)?;
    interest_changed(conn);
    Ok(())
}

fn interest_changed(conn: &Connection) {
    if let Some(path) = conn.path() {
        crate::scheduler::model_capacity::Capacity::for_database(Path::new(path))
            .derived
            .send_modify(|version| *version += 1);
    }
}

pub fn add_interest(conn: &Connection, label: &str) -> rrss_core::Result<i64> {
    interests::insert_interest(conn, label.trim(), "manual")
}

pub fn rate_article(conn: &Connection, id: i64, up: bool) -> rrss_core::Result<RatingUndoView> {
    let kind = if up {
        FeedbackKind::Up
    } else {
        FeedbackKind::Down
    };
    let result = feedback::rate_article(conn, id, kind)?;
    crate::jobs::rank::request(conn)?;
    let previous = result.previous;
    Ok(RatingUndoView {
        up,
        previous: match previous.map(|p| p.kind) {
            Some(FeedbackKind::Up) => 1,
            Some(FeedbackKind::Down) => -1,
            None => 0,
        },
        previous_at: previous.map_or(0, |p| p.created_at),
        suggested_rule_id: result.suggested_rule_id,
    })
}

/// 撤销 `rate_article`：评分没再改过时放回原评分，并撤掉那次新生成的待确认建议。
pub fn undo_rating(conn: &Connection, id: i64, undo: &RatingUndoView) -> rrss_core::Result<()> {
    let kind = |up| {
        if up {
            FeedbackKind::Up
        } else {
            FeedbackKind::Down
        }
    };
    let previous = (undo.previous != 0).then(|| feedback::PreviousFeedback {
        kind: kind(undo.previous > 0),
        created_at: undo.previous_at,
    });
    feedback::undo_rating(
        conn,
        id,
        feedback::RatingUndo {
            kind: kind(undo.up),
            previous,
            suggested_rule_id: undo.suggested_rule_id,
        },
    )?;
    crate::jobs::rank::request(conn)?;
    Ok(())
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
            return Err(rrss_core::Error::InvalidMemory(MemoryProblem::AlreadyConfirmed));
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
        return Err(Error::InvalidProvider(ProviderProblem::NameMissing));
    }
    crate::capabilities::validate_base_url(&input.base_url)?;
    if !matches!(input.protocol.trim(), "chat" | "responses" | "auto") {
        return Err(Error::InvalidProvider(ProviderProblem::ProtocolUnsupported));
    }
    let preset = input
        .kind
        .as_deref()
        .or((input.id == 0).then_some("openai_compat"))
        .map(|kind| {
            crate::provider_presets::provider_presets()
                .into_iter()
                .find(|preset| preset.id == kind)
                .ok_or_else(|| Error::InvalidProvider(ProviderProblem::PresetUnknown))
        })
        .transpose()?;
    let compat = input
        .compat_json
        .as_deref()
        .map(|raw| {
            let raw = if raw.trim().is_empty() { "{}" } else { raw };
            let value: serde_json::Value = serde_json::from_str(raw)
                .map_err(|_| Error::InvalidCompat(CompatProblem::ProviderObjectRequired))?;
            if !value.is_object() {
                return Err(Error::InvalidCompat(CompatProblem::ProviderObjectRequired));
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
            // 新填的密钥替换；「清除」删掉；否则保留（前端拿不到完整密钥）。
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
                    .map_err(|_| Error::InvalidCompat(CompatProblem::PresetInvalid))?;
                ai_store::set_provider_compat(tx, provider_id, &compat)?;
            }
        }
        if let Some(compat) = compat {
            ai_store::set_provider_compat(tx, provider_id, &compat)?;
        }
        if let Some(limit) = input.max_concurrency {
            ai_store::set_provider_concurrency(tx, provider_id, limit)?;
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
        return Err(rrss_core::Error::InvalidProvider(ProviderProblem::ChatUsesOwnConfig));
    }
    let model = model.trim();
    if provider_id == 0 && model.is_empty() {
        return ai_store::set_task_binding(conn, task, None, None);
    }
    if provider_id <= 0 || model.is_empty() {
        return Err(rrss_core::Error::InvalidModelChoice(ModelChoiceProblem::SelectionIncomplete));
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
    article_citation(conn, id, "article", String::new(), -1)
}

pub fn cite_selection(
    conn: &Connection,
    article_id: i64,
    excerpt: &str,
    block_id: i64,
) -> rrss_core::Result<CitationView> {
    article_citation(
        conn,
        article_id,
        "span",
        excerpt.chars().take(500).collect(),
        block_id,
    )
}

fn article_citation(
    conn: &Connection,
    article_id: i64,
    kind: &str,
    excerpt: String,
    block_id: i64,
) -> rrss_core::Result<CitationView> {
    let article = articles::citation_metadata(conn, article_id)?;
    if article.temporary {
        articles::collect_article(conn, article_id)?;
    }
    Ok(CitationView {
        kind: kind.into(),
        article_id,
        title: article.title,
        feed_title: article.feed_title,
        url: article.canonical_url.unwrap_or_default(),
        excerpt,
        block_id,
    })
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

/// 只导出偏好（规则、兴趣、记忆），不含密钥。
pub fn prefs_export(conn: &Connection, parts: prefs::Parts) -> rrss_core::Result<String> {
    let export = prefs::export(conn, parts)?;
    serde_json::to_string_pretty(&export)
        .map_err(|_| rrss_core::Error::Backup(BackupProblem::Serialization))
}

/// 合并导入偏好；规则变化后重算可见列表。
pub fn prefs_import(conn: &Connection, text: &str) -> rrss_core::Result<PrefsImportView> {
    let export = prefs::parse(text)?;
    let r = persist_then_rescore(conn, |c| prefs::import(c, &export))?;
    use crate::views::BackupMessage as M;
    let mut parts = Vec::new();
    if export.rules.is_some() {
        parts.push(M::PrefsRules {
            count: r.rules_added,
            duplicates: r.rules_duplicate,
        });
    }
    if r.packs_added > 0 {
        parts.push(M::PrefsPacks {
            count: r.packs_added,
        });
    }
    if export.interests.is_some() {
        parts.push(M::PrefsInterests {
            count: r.interests_added,
            duplicates: r.interests_duplicate,
        });
    }
    if export.memories.is_some() {
        parts.push(M::PrefsMemories {
            count: r.memories_added,
            duplicates: r.memories_duplicate,
        });
    }
    if parts.is_empty() {
        parts.push(M::PrefsEmpty {});
    }
    Ok(PrefsImportView {
        summary: parts,
        skipped: r.skipped.into_iter().map(prefs_skip).collect(),
    })
}

fn prefs_skip(skip: rrss_core::prefs::ImportSkip) -> PrefsSkipView {
    use rrss_core::prefs::{ImportSkip as S, Scope};
    let scope = |scope: Scope| match scope {
        Scope::Global => PrefsScopeView::Global,
        Scope::Feed { url } => PrefsScopeView::Feed { url },
        Scope::Folder { path } => PrefsScopeView::Folder { path },
    };
    match skip {
        S::RuleScopeMissing {
            polarity,
            dimension,
            pattern,
            scope: within,
        } => PrefsSkipView::RuleScopeMissing {
            polarity: polarity.as_str().into(),
            dimension: dimension.as_str().into(),
            pattern,
            scope: scope(within),
        },
        S::RuleInvalid {
            polarity,
            dimension,
            pattern,
            reason,
        } => PrefsSkipView::RuleInvalid {
            polarity: polarity.as_str().into(),
            dimension: dimension.as_str().into(),
            pattern,
            error: UiError::from(rrss_core::Error::InvalidRule(reason)),
        },
        S::PackMissing { pack } => PrefsSkipView::PackMissing { pack },
        S::MemoryScopeMissing {
            content,
            scope: within,
        } => PrefsSkipView::MemoryScopeMissing {
            content,
            scope: scope(within),
        },
    }
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
            return Err(rrss_core::Error::InvalidSetting(SettingProblem::VectorSpaceStale));
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
fn open_temp() -> (tempfile::TempDir, rrss_core::db::PooledConnection) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("t.db");
    let conn = db::open_at(&path).unwrap();
    (dir, conn)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rrss_core::db;

    #[test]
    fn citations_keep_snapshot_fields_and_only_collect_temporary_articles() {
        let (_dir, conn) = open_temp();
        conn.execute_batch(
            "INSERT INTO feeds(id,url,created_at) VALUES(1,'https://notes.example/feed',0);
             INSERT INTO articles(id,feed_id,title,canonical_url,created_at,expires_at)
             VALUES(1,1,'Fixture','https://notes.example/fixture',0,1),
                   (2,1,'Selection fixture',NULL,0,1)",
        )
        .unwrap();

        let whole = cite_article(&conn, 1).unwrap();
        assert_eq!(whole.kind, "article");
        assert_eq!(whole.article_id, 1);
        assert_eq!(whole.title, "Fixture");
        assert_eq!(whole.feed_title, "notes.example");
        assert_eq!(whole.url, "https://notes.example/fixture");
        assert!(whole.excerpt.is_empty());
        assert_eq!(whole.block_id, -1);
        assert!(!articles::is_temporary(&conn, 1).unwrap());

        let excerpt = "é".repeat(501);
        let span = cite_selection(&conn, 2, &excerpt, 7).unwrap();
        assert_eq!(span.kind, "span");
        assert_eq!(span.article_id, 2);
        assert!(span.url.is_empty());
        assert_eq!(span.excerpt, "é".repeat(500));
        assert_eq!(span.block_id, 7);
        assert!(!articles::is_temporary(&conn, 2).unwrap());

        conn.execute("UPDATE articles SET title='Renamed fixture' WHERE id=2", [])
            .unwrap();
        conn.execute_batch("PRAGMA query_only=ON").unwrap();
        assert!(cite_article(&conn, 1).is_ok());
        let next = cite_selection(&conn, 2, "another excerpt", 8).unwrap();
        assert_eq!(next.title, "Renamed fixture");
        assert_eq!(next.excerpt, "another excerpt");
        assert_eq!(next.block_id, 8);
        assert!(matches!(
            cite_article(&conn, 999),
            Err(rrss_core::Error::ArticleNotFound)
        ));
        assert!(matches!(
            cite_selection(&conn, 999, "missing", 0),
            Err(rrss_core::Error::ArticleNotFound)
        ));
        conn.execute_batch("PRAGMA query_only=OFF").unwrap();
    }

    #[test]
    #[ignore = "synthetic citation latency probe"]
    fn citations_metadata_latency() {
        use std::hint::black_box;
        use std::time::Instant;

        let (dir, conn) = open_temp();
        conn.execute_batch("INSERT INTO feeds(id,url,created_at) VALUES(1,'https://notes.example/feed',0)")
            .unwrap();
        for (id, kib) in [(1, 4), (2, 512)] {
            let text = "Synthetic block ".repeat(kib * 64);
            conn.execute(
                "INSERT INTO articles(id,feed_id,title,created_at,content_html,blocks_json,fulltext_md)
                 VALUES(?1,1,'Fixture',0,?2,?3,?4)",
                rusqlite::params![
                    id,
                    format!("<p>{text}</p>"),
                    serde_json::json!({"text": text}).to_string(),
                    text,
                ],
            )
            .unwrap();
        }
        let path = dir.path().join("t.db");
        let run = |id, blocks: &[i64], fresh: bool, baseline: bool| {
            let mut timings = Vec::new();
            let warm = Connection::open(&path).unwrap();
            if !fresh {
                drop(articles::get_article(&warm, id).unwrap());
            }
            for _ in 0..30 {
                let cold = fresh.then(|| Connection::open(&path).unwrap());
                let connection = cold.as_ref().unwrap_or(&warm);
                let started = Instant::now();
                for &block_id in blocks {
                    if baseline {
                        articles::collect_article(connection, id).unwrap();
                        let article = articles::get_article(connection, id).unwrap();
                        black_box(CitationView {
                            kind: if block_id < 0 { "article" } else { "span" }.into(),
                            article_id: article.id,
                            title: article.title,
                            feed_title: article.feed_title,
                            url: article.canonical_url.unwrap_or_default(),
                            excerpt: if block_id < 0 { "" } else { "Synthetic excerpt" }
                                .chars()
                                .take(500)
                                .collect(),
                            block_id,
                        });
                    } else if block_id < 0 {
                        black_box(cite_article(connection, id).unwrap());
                    } else {
                        black_box(
                            cite_selection(connection, id, "Synthetic excerpt", block_id).unwrap(),
                        );
                    }
                }
                timings.push(started.elapsed().as_secs_f64() * 1_000.0);
            }
            timings.sort_by(f64::total_cmp);
            ((timings[14] + timings[15]) / 2.0, timings[28])
        };
        for (id, kib) in [(1, 4), (2, 512)] {
            for (kind, blocks) in [("article", &[-1_i64][..]), ("three spans", &[0, 1, 2][..])] {
                for (cache, fresh) in [("new connection", true), ("warm connection", false)] {
                    let before = run(id, blocks, fresh, true);
                    let after = run(id, blocks, fresh, false);
                    println!(
                        "citation db {kib} KiB/field {kind} {cache} n=30 median/p95 ms: {:.3}/{:.3} -> {:.3}/{:.3}",
                        before.0, before.1, after.0, after.1
                    );
                }
            }
        }
    }

    #[tokio::test]
    async fn interest_edits_wake_existing_background_rebuild() {
        use rusqlite::params;

        let (dir, conn) = open_temp();
        let path = dir.path().join("t.db");
        let capacity = crate::scheduler::model_capacity::Capacity::for_database(&path);
        let mut derived = capacity.derived.subscribe();
        let vector = rag::f32s_to_le(&[1.0, 0.0, 0.0]);
        let tx = db::write_transaction(&conn).unwrap();
        tx.execute_batch(
            "INSERT INTO vector_spaces(id,base_url,model,dims,prefix_version,normalized,state,created_at)
             VALUES(1,'','fixture',3,1,0,'active',0);
             INSERT INTO feeds(id,url,created_at) VALUES(1,'https://interest.example/feed',0);",
        ).unwrap();
        for id in 1..=2049 {
            tx.execute(
                "INSERT INTO articles(id,feed_id,title,created_at) VALUES(?1,1,'fixture',0)",
                [id],
            )
            .unwrap();
            tx.execute("INSERT INTO chunks(article_id,ordinal,kind,text,embedding,space_id) VALUES(?1,0,'title_summary','fixture',?2,1)", params![id,vector]).unwrap();
        }
        for id in 1..=2 {
            tx.execute("INSERT INTO interests(id,label,source,embedding,space_id,created_at) VALUES(?1,'fixture','manual',?2,1,0)", params![id,vector]).unwrap();
        }
        tx.commit().unwrap();
        crate::jobs::run_interest_boost(&path).await.unwrap();
        set_interest_enabled(&conn, 1, false).unwrap();
        assert!(derived.has_changed().unwrap());
        derived.borrow_and_update();
        assert_eq!(
            db::get_setting(&conn, "interest_boost_sig")
                .unwrap()
                .as_deref(),
            Some("recheck")
        );
        assert!(!interests::list_interests(&conn).unwrap()[0].enabled);
        crate::jobs::run_interest_boost(&path).await.unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT interest_boost FROM articles WHERE id=2049",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            interests::DEFAULT_WEIGHT
        );
        set_interest_enabled(&conn, 1, true).unwrap();
        assert!(derived.has_changed().unwrap());
        derived.borrow_and_update();
        delete_interest(&conn, 1).unwrap();
        assert!(derived.has_changed().unwrap());
        derived.borrow_and_update();
        crate::jobs::run_interest_boost(&path).await.unwrap();
        assert_eq!(interests::list_interests(&conn).unwrap().len(), 1);
        assert_eq!(
            conn.query_row(
                "SELECT interest_boost FROM articles WHERE id=2049",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            interests::DEFAULT_WEIGHT
        );
        assert_eq!(
            crate::UiError::from(set_interest_enabled(&conn, 1, true).unwrap_err()).code(),
            "interest_not_found"
        );
        assert!(!derived.has_changed().unwrap());
    }

    #[test]
    fn memory_conversion_rejects_repeats_but_keeps_distinct_sources() {
        let (_dir, conn) = open_temp();
        let first = memories::insert_pending_fact(&conn, "local first").unwrap();
        let second = memories::insert_pending_fact(&conn, "local first").unwrap();
        let interest = memory_as_interest(&conn, first).unwrap();
        assert!(!memories::get_memory(&conn, first).unwrap().pending());
        assert_eq!(crate::UiError::from(memory_as_interest(&conn, first).unwrap_err()).code(), "memory_already_confirmed");
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

    #[test]
    fn successful_feedback_events_and_sessions_request_rank_refresh() {
        let (_dir, conn) = open_temp();
        conn.execute_batch(
            "INSERT INTO feeds(id,url,created_at) VALUES(1,'https://rank.invalid/feed',0);
            INSERT INTO articles(id,feed_id,title,created_at) VALUES(1,1,'one',0);",
        )
        .unwrap();
        let pending = || db::get_setting(&conn, "rank_dirty_at").unwrap().is_some();
        let liked = rate_article(&conn, 1, true).unwrap();
        assert!(pending());
        conn.execute("DELETE FROM settings WHERE key='rank_dirty_at'", [])
            .unwrap();
        let disliked = rate_article(&conn, 1, false).unwrap();
        assert_eq!((disliked.up, disliked.previous), (false, 1));
        conn.execute("DELETE FROM settings WHERE key='rank_dirty_at'", [])
            .unwrap();
        undo_rating(&conn, 1, &disliked).unwrap();
        assert!(pending());
        assert_eq!(
            feedback::feedback_for_article(&conn, 1).unwrap(),
            Some(FeedbackKind::Up)
        );
        assert_eq!(liked.previous, 0);
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
        assert_eq!(
            serde_json::to_value(report.summary).unwrap(),
            serde_json::json!([{ "code": "prefs_rules", "params": { "count": 1, "duplicates": 0 } }, { "code": "prefs_interests", "params": { "count": 1, "duplicates": 0 } }])
        );
        let again = prefs_import(&other, &text).unwrap();
        assert_eq!(
            serde_json::to_value(again.summary).unwrap(),
            serde_json::json!([{ "code": "prefs_rules", "params": { "count": 0, "duplicates": 1 } }, { "code": "prefs_interests", "params": { "count": 0, "duplicates": 1 } }])
        );
        assert!(prefs_import(&other, "not json").is_err());
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
                max_concurrency: None,
            },
        }
    }

    #[test]
    fn a_use_can_only_pick_an_added_model_and_a_bad_batch_rolls_back() {
        use crate::views::{SaveModelBindingsArgs, TaskModelInput};
        let (_dir, conn) = open_temp();
        let id = crate::test_net::fake_provider(&conn, "新端点", "http://model.example/v1".into());
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
        let error = crate::UiError::from(err);
        assert_eq!(error.code(), "model_chat_disabled");
        assert_eq!(serde_json::to_value(error).unwrap()["params"], serde_json::json!({"model":"text-embedding-3-small"}));
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
        let pid = crate::test_net::fake_provider(&conn, "新端点", "http://model.example/v1".into());
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
        let pid = crate::test_net::fake_provider(&conn, "新端点", "http://model.example/v1".into());
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
