//! 偏好单独导出 / 导入：规则、兴趣、记忆。换机时只带偏好、不带文章，不含密钥。
//!
//! 范围不写本机 id：源用 URL，文件夹用从根开始的名称路径。导入是合并，按内容去重；
//! 范围在本机找不到的条目跳过并写进报告。预设包里的规则只导出包 id，导入时重新安装。
//! 待确认的建议规则和记忆不导出；线程范围的记忆不导出。

use crate::error::backup::BackupProblem;
use std::collections::HashSet;

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::feeds::now_ms;
use crate::rules::{Dimension, NewRule, Polarity, RuleSource, ScopeKind};

pub const FORMAT: u32 = 1;
const KIND: &str = "rrss-prefs";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Parts {
    pub rules: bool,
    pub interests: bool,
    pub memories: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Scope {
    Global,
    Feed { url: String },
    Folder { path: Vec<String> },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuleOut {
    pub polarity: Polarity,
    pub dimension: Dimension,
    pub pattern: String,
    pub weight: i64,
    pub enabled: bool,
    pub scope: Scope,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InterestOut {
    pub label: String,
    pub weight: i64,
    pub enabled: bool,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryOut {
    pub kind: String,
    pub content: String,
    pub pinned: bool,
    pub strength: f64,
    pub source: Option<String>,
    pub scope: Scope,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Export {
    pub format: u32,
    pub kind: String,
    pub created_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rules: Option<Vec<RuleOut>>,
    /// 已安装的预设包 id。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub packs: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interests: Option<Vec<InterestOut>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memories: Option<Vec<MemoryOut>>,
}

/// 可导出的条数，供界面在勾选框旁显示。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ExportCounts {
    pub rules: i64,
    pub interests: i64,
    pub memories: i64,
}

pub fn export_counts(conn: &Connection) -> Result<ExportCounts> {
    let rules: i64 = conn.query_row(
        "SELECT (SELECT COUNT(*) FROM rules WHERE source = 'manual' AND (pack_id IS NULL OR pack_id = ''))
              + (SELECT COUNT(DISTINCT pack_id) FROM rules WHERE pack_id IS NOT NULL AND pack_id != '')",
        [],
        |r| r.get(0),
    )?;
    let interests: i64 = conn.query_row("SELECT COUNT(*) FROM interests", [], |r| r.get(0))?;
    let memories: i64 = conn.query_row(
        "SELECT COUNT(*) FROM memories
         WHERE scope != 'thread' AND COALESCE(source, '') != 'pending' AND strength > 0",
        [],
        |r| r.get(0),
    )?;
    Ok(ExportCounts {
        rules,
        interests,
        memories,
    })
}

pub fn export(conn: &Connection, parts: Parts) -> Result<Export> {
    let mut out = Export {
        format: FORMAT,
        kind: KIND.into(),
        created_at: now_ms(),
        rules: None,
        packs: None,
        interests: None,
        memories: None,
    };
    if parts.rules {
        let mut rules = Vec::new();
        let mut stmt = conn.prepare(
            "SELECT polarity, dimension, pattern, weight, enabled, scope_kind, scope_id
             FROM rules WHERE source = 'manual' AND (pack_id IS NULL OR pack_id = '') ORDER BY id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, bool>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, Option<i64>>(6)?,
            ))
        })?;
        for row in rows {
            let (polarity, dimension, pattern, weight, enabled, scope_kind, scope_id) = row?;
            let Some(scope) = scope_out(conn, &scope_kind, scope_id)? else {
                continue;
            };
            rules.push(RuleOut {
                polarity: Polarity::parse_str(&polarity)?,
                dimension: Dimension::parse_str(&dimension)?,
                pattern,
                weight,
                enabled,
                scope,
            });
        }
        out.rules = Some(rules);
        out.packs = Some(crate::presets::installed_packs(conn)?);
    }
    if parts.interests {
        let mut stmt =
            conn.prepare("SELECT label, weight, enabled, source FROM interests ORDER BY id")?;
        let rows = stmt.query_map([], |r| {
            Ok(InterestOut {
                label: r.get(0)?,
                weight: r.get(1)?,
                enabled: r.get(2)?,
                source: r.get(3)?,
            })
        })?;
        out.interests = Some(rows.collect::<rusqlite::Result<_>>()?);
    }
    if parts.memories {
        let mut memories = Vec::new();
        let mut stmt = conn.prepare(
            "SELECT kind, content, pinned, strength, source, scope, scope_id FROM memories
             WHERE scope != 'thread' AND COALESCE(source, '') != 'pending' AND strength > 0
             ORDER BY id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, bool>(2)?,
                r.get::<_, f64>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, Option<i64>>(6)?,
            ))
        })?;
        for row in rows {
            let (kind, content, pinned, strength, source, scope_kind, scope_id) = row?;
            let Some(scope) = scope_out(conn, &scope_kind, scope_id)? else {
                continue;
            };
            memories.push(MemoryOut {
                kind,
                content,
                pinned,
                strength,
                source,
                scope,
            });
        }
        out.memories = Some(memories);
    }
    Ok(out)
}

fn scope_out(conn: &Connection, kind: &str, id: Option<i64>) -> Result<Option<Scope>> {
    Ok(match (kind, id) {
        ("global", _) => Some(Scope::Global),
        ("feed", Some(id)) => conn
            .query_row("SELECT url FROM feeds WHERE id = ?1", [id], |r| r.get(0))
            .optional()?
            .map(|url| Scope::Feed { url }),
        ("folder", Some(id)) => folder_path(conn, id)?.map(|path| Scope::Folder { path }),
        _ => None,
    })
}

fn folder_path(conn: &Connection, id: i64) -> Result<Option<Vec<String>>> {
    let mut path = Vec::new();
    let mut cur = Some(id);
    while let Some(id) = cur {
        if path.len() > 64 {
            return Ok(None);
        }
        let row: Option<(String, Option<i64>)> = conn
            .query_row(
                "SELECT name, parent_id FROM folders WHERE id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some((name, parent)) = row else {
            return Ok(None);
        };
        path.push(name);
        cur = parent;
    }
    path.reverse();
    Ok(Some(path))
}

fn resolve_folder(conn: &Connection, path: &[String]) -> Result<Option<i64>> {
    let mut parent: Option<i64> = None;
    for name in path {
        let id: Option<i64> = conn
            .query_row(
                "SELECT id FROM folders WHERE name = ?1 AND parent_id IS ?2 ORDER BY id LIMIT 1",
                params![name, parent],
                |r| r.get(0),
            )
            .optional()?;
        match id {
            Some(id) => parent = Some(id),
            None => return Ok(None),
        }
    }
    Ok(parent)
}

/// `(scope_kind, scope_id)`；本机找不到时为 `None`。
fn resolve_scope(conn: &Connection, scope: &Scope) -> Result<Option<(&'static str, Option<i64>)>> {
    Ok(match scope {
        Scope::Global => Some(("global", None)),
        Scope::Feed { url } => conn
            .query_row("SELECT id FROM feeds WHERE url = ?1", [url], |r| {
                r.get::<_, i64>(0)
            })
            .optional()?
            .map(|id| ("feed", Some(id))),
        Scope::Folder { path } => resolve_folder(conn, path)?.map(|id| ("folder", Some(id))),
    })
}

fn scope_text(scope: &Scope) -> String {
    match scope {
        Scope::Global => "全局".into(),
        Scope::Feed { url } => url.clone(),
        Scope::Folder { path } => format!("文件夹「{}」", path.join(" / ")),
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportReport {
    pub rules_added: u32,
    pub rules_duplicate: u32,
    pub packs_added: u32,
    pub interests_added: u32,
    pub interests_duplicate: u32,
    pub memories_added: u32,
    pub memories_duplicate: u32,
    /// 没有导入的条目，每条一句说明。
    pub skipped: Vec<String>,
}

pub fn parse(text: &str) -> Result<Export> {
    let export: Export =
        serde_json::from_str(text).map_err(|_| Error::Backup(BackupProblem::PreferencesInvalid))?;
    if export.kind != KIND {
        return Err(Error::Backup(BackupProblem::PreferencesKindInvalid));
    }
    if export.format > FORMAT {
        return Err(Error::Backup(BackupProblem::PreferencesFormatUnsupported));
    }
    Ok(export)
}

/// 合并导入。调用方负责事务。
pub fn import(conn: &Connection, export: &Export) -> Result<ImportReport> {
    let mut report = ImportReport::default();
    for rule in export.rules.iter().flatten() {
        let what = format!(
            "{}{}「{}」",
            if rule.polarity == Polarity::Like {
                "喜欢"
            } else {
                "不喜欢"
            },
            rule.dimension.label_zh(),
            rule.pattern
        );
        let Some((kind, scope_id)) = resolve_scope(conn, &rule.scope)? else {
            report.skipped.push(format!(
                "规则 {what}：限定的 {} 在本机不存在",
                scope_text(&rule.scope)
            ));
            continue;
        };
        let exists: i64 = conn.query_row(
            "SELECT COUNT(*) FROM rules WHERE polarity = ?1 AND dimension = ?2 AND pattern = ?3
               AND scope_kind = ?4 AND scope_id IS ?5",
            params![
                rule.polarity.as_str(),
                rule.dimension.as_str(),
                rule.pattern.trim(),
                kind,
                scope_id
            ],
            |r| r.get(0),
        )?;
        if exists > 0 {
            report.rules_duplicate += 1;
            continue;
        }
        let spec = NewRule {
            polarity: rule.polarity,
            dimension: rule.dimension,
            pattern: rule.pattern.clone(),
            scope_kind: ScopeKind::parse_str(kind)?,
            scope_id,
            weight: rule.weight,
            source: RuleSource::Manual,
            enabled: rule.enabled,
            pack_id: None,
        };
        match crate::rules::insert_rule(conn, spec) {
            Ok(_) => report.rules_added += 1,
            Err(Error::InvalidRule(msg)) => report.skipped.push(format!("规则 {what}：{msg}")),
            Err(err) => return Err(err),
        }
    }
    let installed: HashSet<String> = crate::presets::installed_packs(conn)?.into_iter().collect();
    for pack in export.packs.iter().flatten() {
        if installed.contains(pack) {
            continue;
        }
        match crate::presets::apply_pack(conn, pack) {
            Ok(_) => report.packs_added += 1,
            Err(Error::PresetNotFound(_)) => report
                .skipped
                .push(format!("预设包 {pack}：本机版本没有这个包")),
            Err(err) => return Err(err),
        }
    }
    for interest in export.interests.iter().flatten() {
        let label = interest.label.trim();
        if label.is_empty() {
            continue;
        }
        let exists: i64 = conn.query_row(
            "SELECT COUNT(*) FROM interests WHERE lower(trim(label)) = lower(?1)",
            [label],
            |r| r.get(0),
        )?;
        if exists > 0 {
            report.interests_duplicate += 1;
            continue;
        }
        let source = if interest.source == "from_chat" {
            "from_chat"
        } else {
            "manual"
        };
        conn.execute(
            "INSERT INTO interests (label, weight, source, enabled, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![label, interest.weight, source, interest.enabled, now_ms()],
        )?;
        report.interests_added += 1;
    }
    for memory in export.memories.iter().flatten() {
        let content = memory.content.trim();
        if content.is_empty() || crate::memories::MemoryKind::parse_str(&memory.kind).is_err() {
            continue;
        }
        let exists: i64 = conn.query_row(
            "SELECT COUNT(*) FROM memories WHERE trim(content) = ?1",
            [content],
            |r| r.get(0),
        )?;
        if exists > 0 {
            report.memories_duplicate += 1;
            continue;
        }
        let Some((kind, scope_id)) = resolve_scope(conn, &memory.scope)? else {
            report.skipped.push(format!(
                "记忆「{content}」：限定的 {} 在本机不存在",
                scope_text(&memory.scope)
            ));
            continue;
        };
        conn.execute(
            "INSERT INTO memories (kind, content, scope, scope_id, pinned, strength, source, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![memory.kind, content, kind, scope_id, memory.pinned, memory.strength, memory.source, now_ms()],
        )?;
        report.memories_added += 1;
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_at;
    use tempfile::tempdir;

    const ALL: Parts = Parts {
        rules: true,
        interests: true,
        memories: true,
    };

    fn seed(conn: &Connection) {
        conn.execute_batch(
            "INSERT INTO folders (id, parent_id, name, created_at) VALUES (1, NULL, '技术', 0), (2, 1, '博客', 0);
             INSERT INTO feeds (id, url, title, folder_id, created_at) VALUES (7, 'https://news.example.com/rss', 'N', 2, 0);
             INSERT INTO rules (polarity, dimension, pattern, scope_kind, scope_id, weight, source, enabled, created_at) VALUES
               ('dislike', 'title_kw', '广告', 'feed', 7, 10, 'manual', 1, 0),
               ('like', 'author', 'Alice', 'folder', 2, 5, 'manual', 0, 0),
               ('like', 'tag', 'rust', 'global', NULL, 10, 'manual', 1, 0),
               ('like', 'tag', 'suggest', 'global', NULL, 10, 'suggested', 1, 0);
             INSERT INTO interests (label, weight, source, enabled, embedding, created_at) VALUES ('Rust', 3, 'manual', 1, x'00', 0);
             INSERT INTO memories (kind, content, scope, scope_id, pinned, strength, source, created_at) VALUES
               ('semantic', '喜欢短文', 'global', NULL, 1, 1.0, 'chat', 0),
               ('semantic', '这个源偏技术', 'feed', 7, 0, 0.8, 'chat', 0),
               ('semantic', '待确认', 'global', NULL, 0, 0.0, 'pending', 0),
               ('episode', '线程里的', 'thread', 3, 0, 1.0, 'chat', 0);",
        )
        .unwrap();
    }

    #[test]
    fn export_uses_urls_and_folder_paths_and_skips_unconfirmed() {
        let dir = tempdir().unwrap();
        let conn = open_at(&dir.path().join("a.db")).unwrap();
        seed(&conn);
        let out = export(&conn, ALL).unwrap();
        let rules = out.rules.as_ref().unwrap();
        assert_eq!(rules.len(), 3);
        assert_eq!(
            rules[0].scope,
            Scope::Feed {
                url: "https://news.example.com/rss".into()
            }
        );
        assert_eq!(
            rules[1].scope,
            Scope::Folder {
                path: vec!["技术".into(), "博客".into()]
            }
        );
        assert!(!rules[1].enabled);
        assert_eq!(out.memories.as_ref().unwrap().len(), 2);
        let counts = export_counts(&conn).unwrap();
        assert_eq!((counts.rules, counts.interests, counts.memories), (3, 1, 2));
        let text = serde_json::to_string(&out).unwrap();
        assert!(!text.contains("api_key"));
        assert_eq!(parse(&text).unwrap(), out);

        let only_rules = export(
            &conn,
            Parts {
                rules: true,
                ..Parts::default()
            },
        )
        .unwrap();
        assert!(only_rules.interests.is_none() && only_rules.memories.is_none());
    }

    #[test]
    fn import_into_empty_db_reports_missing_scopes_then_is_idempotent() {
        let dir = tempdir().unwrap();
        let src = open_at(&dir.path().join("a.db")).unwrap();
        seed(&src);
        let data = export(&src, ALL).unwrap();

        let dst = open_at(&dir.path().join("b.db")).unwrap();
        let report = import(&dst, &data).unwrap();
        assert_eq!(report.rules_added, 1);
        assert_eq!(report.interests_added, 1);
        assert_eq!(report.memories_added, 1);
        assert_eq!(report.skipped.len(), 3, "{:?}", report.skipped);
        assert!(report.skipped[0].contains("news.example.com/rss"));
        assert!(report.skipped[1].contains("文件夹「技术 / 博客」"));

        // 本机建好同名文件夹和同 URL 的源后再导入：补上跳过的，不重复已有的。
        dst.execute_batch(
            "INSERT INTO folders (id, parent_id, name, created_at) VALUES (11, NULL, '技术', 0), (12, 11, '博客', 0);
             INSERT INTO feeds (id, url, created_at) VALUES (21, 'https://news.example.com/rss', 0);",
        )
        .unwrap();
        let again = import(&dst, &data).unwrap();
        assert_eq!(again.rules_added, 2);
        assert_eq!(again.rules_duplicate, 1);
        assert_eq!(again.interests_duplicate, 1);
        assert_eq!(again.memories_added, 1);
        assert_eq!(again.memories_duplicate, 1);
        assert!(again.skipped.is_empty(), "{:?}", again.skipped);
        let scope: (String, i64) = dst
            .query_row(
                "SELECT scope_kind, scope_id FROM rules WHERE pattern = 'Alice'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(scope, ("folder".into(), 12));
        let third = import(&dst, &data).unwrap();
        assert_eq!(
            (
                third.rules_added,
                third.interests_added,
                third.memories_added
            ),
            (0, 0, 0)
        );
        // 兴趣向量不随文件带走，导入后等作业重新生成。
        let embedded: i64 = dst
            .query_row(
                "SELECT COUNT(*) FROM interests WHERE embedding IS NOT NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(embedded, 0);
    }

    #[test]
    fn packs_export_as_ids_and_reinstall() {
        let dir = tempdir().unwrap();
        let src = open_at(&dir.path().join("a.db")).unwrap();
        let pack = crate::presets::list_packs().unwrap().remove(0).id;
        crate::presets::apply_pack(&src, &pack).unwrap();
        let data = export(&src, ALL).unwrap();
        assert_eq!(data.packs.as_deref(), Some(&[pack.clone()][..]));
        assert!(data.rules.as_ref().unwrap().is_empty());

        let dst = open_at(&dir.path().join("b.db")).unwrap();
        let mut with_unknown = data.clone();
        with_unknown
            .packs
            .as_mut()
            .unwrap()
            .push("no-such-pack".into());
        let report = import(&dst, &with_unknown).unwrap();
        assert_eq!(report.packs_added, 1);
        assert_eq!(
            report.skipped,
            vec!["预设包 no-such-pack：本机版本没有这个包".to_string()]
        );
        assert_eq!(crate::presets::installed_packs(&dst).unwrap(), vec![pack]);
    }

    #[test]
    fn parse_rejects_other_files() {
        assert!(parse("{}").is_err());
        assert!(parse("<opml/>").is_err());
        let newer = format!(
            r#"{{"format":{},"kind":"rrss-prefs","created_at":0}}"#,
            FORMAT + 1
        );
        assert!(matches!(
            parse(&newer),
            Err(Error::Backup(BackupProblem::PreferencesFormatUnsupported))
        ));
    }
}
