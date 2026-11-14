//! 模型目录（每个连接获取到的模型 id）与加进来的模型（阶段 19-6 / 19-7）。
//! 所有选模型的地方只列加进来的模型；`chat` 为真的出现在对话列表。两表都随连接级联删除。
//! 日志 target：`rrss_core::model_pool`。

use crate::error::models::{CompatProblem, ModelChoiceProblem, ProviderProblem};
use std::collections::BTreeSet;

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ai_store::{self, TASK_CHAT};
use crate::error::{Error, Result};
use crate::feeds::now_ms;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Catalog {
    pub provider_id: i64,
    /// 按 id 排序、去重。
    pub models: Vec<String>,
    pub fetched_at: i64,
}

/// 「连接 + 模型 id」。
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ModelRef {
    pub provider_id: i64,
    pub model: String,
}

impl ModelRef {
    pub fn new(provider_id: i64, model: &str) -> Self {
        Self {
            provider_id,
            model: model.trim().to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Support {
    Supported,
    Unsupported,
    #[default]
    Unknown,
}

/// 四项能力。键名同时是 `probe_json` / `caps_override_json` 的键。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Caps {
    #[serde(default)]
    pub tool_calling: Support,
    #[serde(default)]
    pub structured_output: Support,
    #[serde(default)]
    pub image_input: Support,
    #[serde(default)]
    pub reasoning: Support,
}

/// 手动设置：没有即按测试结果。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapsOverride {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calling: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_output: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_input: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<bool>,
}

/// 「测试能力」的结果。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Probe {
    #[serde(flatten)]
    pub caps: Caps,
    #[serde(default)]
    pub tested_at: Option<i64>,
    #[serde(default)]
    pub recommended_compat: Value,
}

/// 加进来的一个模型与它的设置。
#[derive(Debug, Clone, PartialEq)]
pub struct PoolEntry {
    pub provider_id: i64,
    pub model: String,
    pub chat: bool,
    pub display_name: String,
    /// 对话里的思考强度；空为关。
    pub thinking: String,
    pub context_window: Option<i64>,
    pub max_output: Option<i64>,
    /// `None` 用连接的兼容设置。
    pub compat: Option<Value>,
    pub probe: Option<Probe>,
    pub overrides: CapsOverride,
}

impl PoolEntry {
    pub fn model_ref(&self) -> ModelRef {
        ModelRef::new(self.provider_id, &self.model)
    }

    /// 生效能力：手动设置 > 测试结果 > 未知。
    pub fn effective(&self) -> Caps {
        let detected = self.probe.as_ref().map(|p| p.caps).unwrap_or_default();
        let pick = |over: Option<bool>, det: Support| match over {
            Some(true) => Support::Supported,
            Some(false) => Support::Unsupported,
            None => det,
        };
        Caps {
            tool_calling: pick(self.overrides.tool_calling, detected.tool_calling),
            structured_output: pick(self.overrides.structured_output, detected.structured_output),
            image_input: pick(self.overrides.image_input, detected.image_input),
            reasoning: pick(self.overrides.reasoning, detected.reasoning),
        }
    }

    /// 设置的思考强度，空为 `off`。
    pub fn thinking_setting(&self) -> String {
        match self.thinking.trim() {
            "" => "off".into(),
            level => level.into(),
        }
    }

    /// 对话里实际用的思考强度：推理明确不支持时为关。
    pub fn chat_thinking(&self) -> &str {
        if self.effective().reasoning == Support::Unsupported {
            return "off";
        }
        match self.thinking.trim() {
            "" => "off",
            level => level,
        }
    }
}

/// 名字像嵌入模型：第一次加入时不自动打开「用于对话」。
pub fn looks_like_embedding(model: &str) -> bool {
    let m = model.to_ascii_lowercase();
    let name = m.rsplit('/').next().unwrap_or(&m);
    m.contains("embed")
        || name.starts_with("bge")
        || name.starts_with("e5-")
        || name.starts_with("gte-")
}

pub fn catalogs(conn: &Connection) -> Result<Vec<Catalog>> {
    let mut stmt = conn.prepare(
        "SELECT provider_id, model, fetched_at FROM ai_model_catalog ORDER BY provider_id, model",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, i64>(2)?,
        ))
    })?;
    let mut out: Vec<Catalog> = Vec::new();
    for row in rows {
        let (provider_id, model, fetched_at) = row?;
        match out.last_mut() {
            Some(last) if last.provider_id == provider_id => {
                last.models.push(model);
                last.fetched_at = last.fetched_at.max(fetched_at);
            }
            _ => out.push(Catalog {
                provider_id,
                models: vec![model],
                fetched_at,
            }),
        }
    }
    Ok(out)
}

/// 用新获取的列表整体覆盖该连接的目录。空列表不写：获取成功但服务没返回模型时保留旧目录。
pub fn replace_catalog(conn: &Connection, provider_id: i64, models: &[String]) -> Result<()> {
    ai_store::get_provider(conn, provider_id)?;
    let models: BTreeSet<&str> = models
        .iter()
        .map(|m| m.trim())
        .filter(|m| !m.is_empty())
        .collect();
    if models.is_empty() {
        return Ok(());
    }
    let tx = crate::db::write_transaction(conn)?;
    tx.execute(
        "DELETE FROM ai_model_catalog WHERE provider_id = ?1",
        [provider_id],
    )?;
    let now = now_ms();
    for model in models {
        tx.execute(
            "INSERT INTO ai_model_catalog (provider_id, model, fetched_at) VALUES (?1, ?2, ?3)",
            params![provider_id, model, now],
        )?;
    }
    tx.commit()?;
    Ok(())
}

const POOL_COLUMNS: &str = "provider_id, model, chat, display_name, thinking, context_window, max_output, compat_json, probe_json, caps_override_json";

fn map_entry(r: &rusqlite::Row<'_>) -> rusqlite::Result<PoolEntry> {
    let json = |raw: Option<String>| raw.and_then(|s| serde_json::from_str::<Value>(&s).ok());
    Ok(PoolEntry {
        provider_id: r.get(0)?,
        model: r.get(1)?,
        chat: r.get::<_, i64>(2)? != 0,
        display_name: r.get(3)?,
        thinking: r.get(4)?,
        context_window: r.get(5)?,
        max_output: r.get(6)?,
        compat: json(r.get(7)?).filter(Value::is_object),
        probe: r
            .get::<_, Option<String>>(8)?
            .and_then(|s| serde_json::from_str(&s).ok()),
        overrides: serde_json::from_str(&r.get::<_, String>(9)?).unwrap_or_default(),
    })
}

/// 按连接、再按加入顺序。
pub fn pool(conn: &Connection) -> Result<Vec<PoolEntry>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {POOL_COLUMNS} FROM ai_model_pool ORDER BY provider_id, created_at, rowid"
    ))?;
    let rows = stmt.query_map([], map_entry)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

pub fn get(conn: &Connection, r: &ModelRef) -> Result<Option<PoolEntry>> {
    conn.query_row(
        &format!("SELECT {POOL_COLUMNS} FROM ai_model_pool WHERE provider_id = ?1 AND model = ?2"),
        params![r.provider_id, r.model],
        map_entry,
    )
    .optional()
    .map_err(Into::into)
}

/// 用途只能选加进来的模型；对话默认还必须用于对话。
pub fn ensure_selectable(conn: &Connection, r: &ModelRef, for_chat: bool) -> Result<PoolEntry> {
    let entry = get(conn, r)?.ok_or_else(|| {
        Error::InvalidModelChoice(ModelChoiceProblem::NotAdded {
            model: r.model.clone(),
        })
    })?;
    if for_chat && !entry.chat {
        return Err(Error::InvalidModelChoice(
            ModelChoiceProblem::ChatDisabled {
                model: r.model.clone(),
            },
        ));
    }
    Ok(entry)
}

/// 移出模型后受影响的用途。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RemoveOutcome {
    /// 改用替换模型的用途。
    pub replaced: Vec<String>,
    /// 变为未选择的用途。
    pub unset: Vec<String>,
}

/// 一次原子地加入与移出。加入的模型默认不用于对话；对话列表为空时，第一个不像嵌入模型的自动打开。
/// 移出的模型正被用途使用时，这些用途改用 `replace`（必须是留下的模型；对话默认还须用于对话），
/// 没有替换或替换不适用于对话时变为未选择。
pub fn update_pool(
    conn: &Connection,
    add: &[ModelRef],
    remove: &[ModelRef],
    replace: Option<&ModelRef>,
) -> Result<RemoveOutcome> {
    let tx = crate::db::write_transaction(conn)?;
    let outcome = update_pool_in_transaction(&tx, add, remove, replace)?;
    tx.commit()?;
    Ok(outcome)
}

/// 允许应用层把用途绑定变化与其它元数据放在同一写事务。
pub fn update_pool_in_transaction(
    tx: &rusqlite::Transaction<'_>,
    add: &[ModelRef],
    remove: &[ModelRef],
    replace: Option<&ModelRef>,
) -> Result<RemoveOutcome> {
    let now = now_ms();
    let mut chat_empty: bool = tx.query_row(
        "SELECT COUNT(*) = 0 FROM ai_model_pool WHERE chat = 1",
        [],
        |r| r.get(0),
    )?;
    for entry in add {
        if entry.model.is_empty() {
            return Err(Error::InvalidProvider(ProviderProblem::ModelIdMissing));
        }
        ai_store::get_provider(tx, entry.provider_id)?;
        let chat = chat_empty && !looks_like_embedding(&entry.model);
        let n = tx.execute(
            "INSERT OR IGNORE INTO ai_model_pool (provider_id, model, created_at, chat) VALUES (?1, ?2, ?3, ?4)",
            params![entry.provider_id, entry.model, now, chat],
        )?;
        if n > 0 && chat {
            chat_empty = false;
        }
    }
    for entry in remove {
        tx.execute(
            "DELETE FROM ai_model_pool WHERE provider_id = ?1 AND model = ?2",
            params![entry.provider_id, entry.model],
        )?;
    }
    let replace = match replace {
        Some(r) => Some(get(tx, r)?.ok_or_else(|| {
            Error::InvalidModelChoice(ModelChoiceProblem::ReplacementMissing {
                model: r.model.clone(),
            })
        })?),
        None => None,
    };
    let mut outcome = RemoveOutcome::default();
    for binding in ai_store::list_task_bindings(tx)? {
        let (Some(provider_id), Some(model)) = (binding.provider_id, binding.model.as_deref())
        else {
            continue;
        };
        if !remove.contains(&ModelRef::new(provider_id, model)) {
            continue;
        }
        let target = replace
            .as_ref()
            .filter(|e| binding.task != TASK_CHAT || e.chat);
        match target {
            Some(e) => {
                ai_store::set_task_binding(tx, &binding.task, Some(e.provider_id), Some(&e.model))?;
                outcome.replaced.push(binding.task);
            }
            None => {
                ai_store::set_task_binding(tx, &binding.task, None, None)?;
                outcome.unset.push(binding.task);
            }
        }
    }
    Ok(outcome)
}

/// 单个模型的设置修改；`None` 表示不改。
#[derive(Debug, Clone, Default)]
pub struct EntryPatch {
    pub chat: Option<bool>,
    pub display_name: Option<String>,
    pub thinking: Option<String>,
    /// `Some(None)` 清空。
    pub context_window: Option<Option<i64>>,
    pub max_output: Option<Option<i64>>,
    /// `Some(None)` 改回用连接的兼容设置。
    pub compat: Option<Option<Value>>,
    pub overrides: Option<CapsOverride>,
}

/// 思考强度由低到高；与对话引擎的档位顺序一致，模型页与线程级共用。不支持的档位在开会话时降到最近可用档。
pub const THINKING_LEVELS: &[&str] = &["off", "minimal", "low", "medium", "high", "xhigh", "max"];

pub fn update_entry(conn: &Connection, r: &ModelRef, patch: &EntryPatch) -> Result<PoolEntry> {
    let tx = crate::db::write_transaction(conn)?;
    let mut e = get(&tx, r)?.ok_or_else(|| {
        Error::InvalidModelChoice(ModelChoiceProblem::NotFound {
            model: r.model.clone(),
        })
    })?;
    if let Some(chat) = patch.chat {
        if !chat && ai_store::chat_default(&tx)?.as_ref() == Some(r) {
            return Err(Error::InvalidModelChoice(
                ModelChoiceProblem::ChatDefaultRequired,
            ));
        }
        e.chat = chat;
    }
    if let Some(name) = &patch.display_name {
        e.display_name = name.trim().to_string();
    }
    if let Some(level) = &patch.thinking {
        let level = level.trim();
        if !level.is_empty() && !THINKING_LEVELS.contains(&level) {
            return Err(Error::InvalidModelChoice(
                ModelChoiceProblem::ThinkingUnknown {
                    level: level.into(),
                },
            ));
        }
        e.thinking = if level == "off" {
            String::new()
        } else {
            level.to_string()
        };
    }
    let positive = |v: Option<i64>, problem: ModelChoiceProblem| match v {
        Some(n) if n <= 0 => Err(Error::InvalidModelChoice(problem)),
        other => Ok(other),
    };
    if let Some(v) = patch.context_window {
        e.context_window = positive(v, ModelChoiceProblem::ContextWindowNonPositive)?;
    }
    if let Some(v) = patch.max_output {
        e.max_output = positive(v, ModelChoiceProblem::MaxOutputNonPositive)?;
    }
    if let Some(compat) = &patch.compat {
        if compat.as_ref().is_some_and(|c| !c.is_object()) {
            return Err(Error::InvalidCompat(CompatProblem::ObjectRequired));
        }
        e.compat = compat.clone();
    }
    if let Some(o) = patch.overrides {
        e.overrides = o;
    }
    tx.execute(
        "UPDATE ai_model_pool SET chat = ?3, display_name = ?4, thinking = ?5, context_window = ?6,
           max_output = ?7, compat_json = ?8, caps_override_json = ?9
         WHERE provider_id = ?1 AND model = ?2",
        params![
            r.provider_id,
            r.model,
            e.chat,
            e.display_name,
            e.thinking,
            e.context_window,
            e.max_output,
            e.compat.as_ref().map(Value::to_string),
            serde_json::to_string(&e.overrides).expect("serializable overrides"),
        ],
    )?;
    tx.commit()?;
    Ok(e)
}

/// 记下「测试能力」的结果。模型已被移出时不记。
pub fn set_probe(conn: &Connection, r: &ModelRef, probe: &Probe) -> Result<()> {
    conn.execute(
        "UPDATE ai_model_pool SET probe_json = ?3 WHERE provider_id = ?1 AND model = ?2",
        params![
            r.provider_id,
            r.model,
            serde_json::to_string(probe).expect("serializable probe")
        ],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai_store::{NewProvider, TASK_DIGEST_DAILY, TASK_FOCUS_SUMMARY};
    use crate::db::{PooledConnection, open_at};
    use tempfile::tempdir;

    fn open_temp() -> (tempfile::TempDir, PooledConnection) {
        let dir = tempdir().unwrap();
        let conn = open_at(&dir.path().join("rrss.db")).unwrap();
        (dir, conn)
    }

    fn provider(conn: &Connection, name: &str) -> i64 {
        ai_store::insert_provider(
            conn,
            &NewProvider {
                name: name.into(),
                base_url: format!("http://{name}/v1"),
                api_key: None,
                protocol: "chat".into(),
            },
        )
        .unwrap()
    }

    fn r(provider_id: i64, model: &str) -> ModelRef {
        ModelRef::new(provider_id, model)
    }

    fn chat_models(conn: &Connection) -> Vec<String> {
        pool(conn)
            .unwrap()
            .into_iter()
            .filter(|e| e.chat)
            .map(|e| e.model)
            .collect()
    }

    #[test]
    fn catalog_is_replaced_whole_and_an_empty_fetch_keeps_it() {
        let (_dir, conn) = open_temp();
        let id = provider(&conn, "a");
        replace_catalog(&conn, id, &["m2".into(), " m1 ".into(), "m2".into()]).unwrap();
        assert_eq!(catalogs(&conn).unwrap()[0].models, ["m1", "m2"]);
        replace_catalog(&conn, id, &[]).unwrap();
        assert_eq!(catalogs(&conn).unwrap()[0].models, ["m1", "m2"]);
        replace_catalog(&conn, id, &["m3".into()]).unwrap();
        assert_eq!(catalogs(&conn).unwrap()[0].models, ["m3"]);
        assert!(matches!(
            replace_catalog(&conn, 999, &["x".into()]),
            Err(Error::ProviderNotFound)
        ));
    }

    #[test]
    fn only_the_first_added_chat_capable_model_joins_the_chat_list() {
        let (_dir, conn) = open_temp();
        let (a, b) = (provider(&conn, "a"), provider(&conn, "b"));
        update_pool(
            &conn,
            &[r(a, "text-embedding-3-small"), r(a, "x"), r(b, "x")],
            &[],
            None,
        )
        .unwrap();
        assert_eq!(
            pool(&conn).unwrap().len(),
            3,
            "the same id on two connections is two entries"
        );
        assert_eq!(chat_models(&conn), ["x"]);
        update_pool(&conn, &[r(a, "y")], &[], None).unwrap();
        assert_eq!(
            chat_models(&conn),
            ["x"],
            "later additions stay out of chat"
        );
    }

    #[test]
    fn removing_a_used_model_replaces_or_unsets_its_uses_atomically() {
        let (_dir, conn) = open_temp();
        let a = provider(&conn, "a");
        update_pool(&conn, &[r(a, "x"), r(a, "y"), r(a, "z")], &[], None).unwrap();
        for task in [TASK_CHAT, TASK_FOCUS_SUMMARY] {
            ai_store::set_task_binding(&conn, task, Some(a), Some("x")).unwrap();
        }
        ai_store::set_task_binding(&conn, TASK_DIGEST_DAILY, Some(a), Some("z")).unwrap();
        // y 不用于对话：摘要改用 y，对话只能变为未选择。
        let out = update_pool(&conn, &[], &[r(a, "x")], Some(&r(a, "y"))).unwrap();
        assert_eq!(out.replaced, [TASK_FOCUS_SUMMARY]);
        assert_eq!(out.unset, [TASK_CHAT]);
        assert_eq!(
            ai_store::resolve_task(&conn, TASK_FOCUS_SUMMARY)
                .unwrap()
                .model,
            "y"
        );
        assert_eq!(ai_store::chat_default(&conn).unwrap(), None);
        let out = update_pool(&conn, &[], &[r(a, "z")], None).unwrap();
        assert_eq!(out.unset, [TASK_DIGEST_DAILY]);
        // 替换对象本身被移出，或不在列表里：整次拒绝。
        assert!(update_pool(&conn, &[], &[r(a, "y")], Some(&r(a, "y"))).is_err());
        assert!(update_pool(&conn, &[], &[r(a, "y")], Some(&r(a, "nope"))).is_err());
        assert_eq!(pool(&conn).unwrap().len(), 1);
    }

    #[test]
    fn a_bad_entry_rolls_back_the_whole_update_and_deleting_a_connection_cascades() {
        let (_dir, conn) = open_temp();
        let a = provider(&conn, "a");
        assert!(update_pool(&conn, &[r(a, "x"), r(a, " ")], &[], None).is_err());
        assert!(update_pool(&conn, &[r(a, "x"), r(999, "y")], &[], None).is_err());
        assert!(pool(&conn).unwrap().is_empty());
        update_pool(&conn, &[r(a, "x")], &[], None).unwrap();
        replace_catalog(&conn, a, &["x".into()]).unwrap();
        ai_store::delete_provider(&conn, a).unwrap();
        assert!(pool(&conn).unwrap().is_empty());
        assert!(catalogs(&conn).unwrap().is_empty());
    }

    #[test]
    fn per_model_settings_and_the_effective_capabilities() {
        let (_dir, conn) = open_temp();
        let a = provider(&conn, "a");
        let x = r(a, "x");
        update_pool(&conn, std::slice::from_ref(&x), &[], None).unwrap();
        let e = get(&conn, &x).unwrap().unwrap();
        assert_eq!(e.effective(), Caps::default());
        assert_eq!(e.chat_thinking(), "off");
        set_probe(
            &conn,
            &x,
            &Probe {
                caps: Caps {
                    tool_calling: Support::Supported,
                    structured_output: Support::Unsupported,
                    ..Caps::default()
                },
                tested_at: Some(5),
                recommended_compat: serde_json::json!({"supportsStrictMode": false}),
            },
        )
        .unwrap();
        let e = update_entry(
            &conn,
            &x,
            &EntryPatch {
                display_name: Some(" X ".into()),
                thinking: Some("high".into()),
                context_window: Some(Some(128_000)),
                overrides: Some(CapsOverride {
                    structured_output: Some(true),
                    reasoning: Some(false),
                    ..CapsOverride::default()
                }),
                ..EntryPatch::default()
            },
        )
        .unwrap();
        assert_eq!(
            e,
            get(&conn, &x).unwrap().unwrap(),
            "what is returned is what is stored"
        );
        assert_eq!(e.display_name, "X");
        let caps = e.effective();
        assert_eq!(caps.tool_calling, Support::Supported);
        assert_eq!(
            caps.structured_output,
            Support::Supported,
            "manual setting wins"
        );
        assert_eq!(caps.reasoning, Support::Unsupported);
        assert_eq!(e.chat_thinking(), "off", "no thinking without reasoning");
        assert_eq!(e.probe.as_ref().unwrap().tested_at, Some(5));
        for bad in [
            EntryPatch {
                thinking: Some("extreme".into()),
                ..EntryPatch::default()
            },
            EntryPatch {
                max_output: Some(Some(0)),
                ..EntryPatch::default()
            },
            EntryPatch {
                compat: Some(Some(serde_json::json!([1]))),
                ..EntryPatch::default()
            },
        ] {
            assert!(update_entry(&conn, &x, &bad).is_err());
        }
        assert!(update_entry(&conn, &r(a, "nope"), &EntryPatch::default()).is_err());
    }

    #[test]
    fn the_chat_default_cannot_leave_the_chat_list() {
        let (_dir, conn) = open_temp();
        let a = provider(&conn, "a");
        update_pool(&conn, &[r(a, "x"), r(a, "y")], &[], None).unwrap();
        ai_store::set_task_binding(&conn, TASK_CHAT, Some(a), Some("x")).unwrap();
        let off = EntryPatch {
            chat: Some(false),
            ..EntryPatch::default()
        };
        assert!(matches!(
            update_entry(&conn, &r(a, "x"), &off),
            Err(Error::InvalidModelChoice(_))
        ));
        assert!(
            ensure_selectable(&conn, &r(a, "y"), true).is_err(),
            "y is not in chat"
        );
        assert!(ensure_selectable(&conn, &r(a, "y"), false).is_ok());
        assert!(ensure_selectable(&conn, &r(a, "nope"), false).is_err());
    }

    #[test]
    fn embedding_names_are_recognised() {
        for m in [
            "text-embedding-3-small",
            "nomic-embed-text",
            "BAAI/bge-m3",
            "bge-m3:latest",
            "intfloat/e5-large",
        ] {
            assert!(looks_like_embedding(m), "{m}");
        }
        for m in ["gpt-5", "qwen2.5:14b", "anthropic/claude-sonnet-4.5"] {
            assert!(!looks_like_embedding(m), "{m}");
        }
    }
}
