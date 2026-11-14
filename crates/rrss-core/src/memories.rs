//! 可编辑长期记忆。不得静默改过滤。日志 target：`rrss_core::memories`。

use crate::error::rules::MemoryProblem;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::ai_store::truncate_chars;
use crate::error::{Error, Result};
use crate::feeds::now_ms;

pub const INJECT_CHARS: usize = 900;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryKind {
    Episode,
    Semantic,
    Procedural,
}

impl MemoryKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Episode => "episode",
            Self::Semantic => "semantic",
            Self::Procedural => "procedural",
        }
    }

    pub fn parse_str(value: &str) -> Result<Self> {
        match value {
            "episode" => Ok(Self::Episode),
            "semantic" => Ok(Self::Semantic),
            "procedural" => Ok(Self::Procedural),
            other => Err(Error::InvalidMemory(MemoryProblem::KindUnknown {
                kind: other.into(),
            })),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Memory {
    pub id: i64,
    pub kind: MemoryKind,
    pub content: String,
    pub scope: String,
    pub pinned: bool,
    pub strength: f64,
    pub source: Option<String>,
    pub last_used_at: Option<i64>,
    pub created_at: i64,
    pub has_embedding: bool,
}

impl Memory {
    pub fn pending(&self) -> bool {
        self.source.as_deref() == Some("pending") || self.strength == 0.0
    }
}

pub fn insert_memory(
    conn: &Connection,
    kind: MemoryKind,
    content: &str,
    source: &str,
    strength: f64,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO memories (kind, content, scope, pinned, strength, source, created_at)
         VALUES (?1, ?2, 'global', 0, ?3, ?4, ?5)",
        params![kind.as_str(), content, strength, source, now_ms()],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn insert_pending_fact(conn: &Connection, content: &str) -> Result<i64> {
    insert_memory(conn, MemoryKind::Semantic, content, "pending", 0.0)
}

pub fn confirm_memory(conn: &Connection, id: i64) -> Result<()> {
    let n = conn.execute(
        "UPDATE memories SET source = 'chat', strength = 1.0 WHERE id = ?1",
        [id],
    )?;
    if n == 0 {
        Err(Error::MemoryNotFound)
    } else {
        Ok(())
    }
}

pub fn delete_memory(conn: &Connection, id: i64) -> Result<()> {
    let n = conn.execute("DELETE FROM memories WHERE id = ?1", [id])?;
    if n == 0 {
        Err(Error::MemoryNotFound)
    } else {
        Ok(())
    }
}

pub fn set_pinned(conn: &Connection, id: i64, pinned: bool) -> Result<()> {
    let n = conn.execute(
        "UPDATE memories SET pinned = ?1 WHERE id = ?2",
        params![i64::from(pinned), id],
    )?;
    if n == 0 {
        Err(Error::MemoryNotFound)
    } else {
        Ok(())
    }
}

pub fn get_memory(conn: &Connection, id: i64) -> Result<Memory> {
    conn.query_row(
        "SELECT id, kind, content, scope, pinned, strength, source, last_used_at, created_at,
                CASE WHEN embedding IS NOT NULL AND length(embedding) > 0 THEN 1 ELSE 0 END
         FROM memories WHERE id = ?1",
        [id],
        map_memory,
    )
    .optional()?
    .ok_or(Error::MemoryNotFound)
}

pub fn list_memories(conn: &Connection, query: &str) -> Result<Vec<Memory>> {
    let q = query.trim();
    if q.is_empty() {
        let mut stmt = conn.prepare(
            "SELECT id, kind, content, scope, pinned, strength, source, last_used_at, created_at,
                    CASE WHEN embedding IS NOT NULL AND length(embedding) > 0 THEN 1 ELSE 0 END
             FROM memories
             ORDER BY pinned DESC, COALESCE(last_used_at, 0) DESC, id DESC",
        )?;
        let rows = stmt.query_map([], map_memory)?;
        return rows
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into);
    }
    let like = format!("%{q}%");
    let mut stmt = conn.prepare(
        "SELECT id, kind, content, scope, pinned, strength, source, last_used_at, created_at,
                CASE WHEN embedding IS NOT NULL AND length(embedding) > 0 THEN 1 ELSE 0 END
         FROM memories WHERE content LIKE ?1
         ORDER BY pinned DESC, COALESCE(last_used_at, 0) DESC, id DESC",
    )?;
    let rows = stmt.query_map([like], map_memory)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

pub fn search_memory(conn: &Connection, query: &str, limit: i64) -> Result<Vec<Memory>> {
    search_memory_hybrid(conn, query, None, limit)
}

pub fn search_memory_hybrid(
    conn: &Connection,
    query: &str,
    query_vec: Option<&[f32]>,
    limit: i64,
) -> Result<Vec<Memory>> {
    let like = list_memories(conn, query)?;
    let Some(vec) = query_vec.filter(|v| !v.is_empty()) else {
        return Ok(like.into_iter().take(limit.max(0) as usize).collect());
    };
    let knn = crate::rag::knn_memories(conn, vec, limit.max(1) * 2).unwrap_or_default();
    Ok(crate::rag::rrf_merge(&like, &knn, limit, |m| m.id))
}

pub fn update_memory_content(conn: &Connection, id: i64, content: &str) -> Result<()> {
    let content = content.trim();
    if content.is_empty() {
        return Err(Error::InvalidMemory(MemoryProblem::ContentMissing));
    }
    let n = conn.execute(
        "UPDATE memories SET content = ?1, embedding = NULL, space_id = NULL WHERE id = ?2",
        params![content, id],
    )?;
    if n == 0 {
        Err(Error::MemoryNotFound)
    } else {
        Ok(())
    }
}

pub fn inject_preview(conn: &Connection) -> Result<(String, usize)> {
    let text = inject_prefix(conn)?;
    let chars = text.chars().count();
    Ok((text, chars))
}

pub fn clear_memories(conn: &Connection) -> Result<usize> {
    let n = conn.execute("DELETE FROM memories", [])?;
    Ok(n)
}

pub fn export_memories_json(conn: &Connection) -> Result<String> {
    let rows = list_memories(conn, "")?;
    let v: Vec<serde_json::Value> = rows
        .iter()
        .map(|m| {
            serde_json::json!({
                "id": m.id,
                "kind": m.kind.as_str(),
                "content": m.content,
                "pinned": m.pinned,
                "source": m.source,
                "pending": m.pending(),
            })
        })
        .collect();
    Ok(serde_json::to_string_pretty(&v).unwrap_or_else(|_| "[]".into()))
}

/// 注入 Pi 系统前缀：pinned 优先，再 `last_used_at`，约 600 token。
pub fn inject_prefix(conn: &Connection) -> Result<String> {
    let mut stmt = conn.prepare(
        "SELECT content FROM memories
         WHERE COALESCE(source, '') != 'pending' AND strength > 0
         ORDER BY pinned DESC, COALESCE(last_used_at, 0) DESC, id DESC",
    )?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    let mut out = String::from("用户记忆（已确认）：\n");
    for row in rows {
        let line = format!("- {}\n", row?);
        if out.len() + line.len() > INJECT_CHARS {
            break;
        }
        out.push_str(&line);
    }
    if out.trim() == "用户记忆（已确认）：" {
        return Ok(String::new());
    }
    Ok(truncate_chars(&out, INJECT_CHARS))
}

pub(crate) fn map_memory(row: &rusqlite::Row<'_>) -> rusqlite::Result<Memory> {
    let kind_raw: String = row.get(1)?;
    let kind = MemoryKind::parse_str(&kind_raw).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(err))
    })?;
    Ok(Memory {
        id: row.get(0)?,
        kind,
        content: row.get(2)?,
        scope: row.get(3)?,
        pinned: row.get::<_, i64>(4)? != 0,
        strength: row.get(5)?,
        source: row.get(6)?,
        last_used_at: row.get(7)?,
        created_at: row.get(8)?,
        has_embedding: row.get::<_, i64>(9)? != 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_at;
    use tempfile::tempdir;

    #[test]
    fn pending_not_injected_until_confirm() {
        let dir = tempdir().unwrap();
        let conn = open_at(&dir.path().join("rrss.db")).unwrap();
        let id = insert_pending_fact(&conn, "喜欢短摘要").unwrap();
        assert!(inject_prefix(&conn).unwrap().is_empty());
        confirm_memory(&conn, id).unwrap();
        let prefix = inject_prefix(&conn).unwrap();
        assert!(prefix.contains("喜欢短摘要"));
        assert!(!get_memory(&conn, id).unwrap().pending());
        delete_memory(&conn, id).unwrap();
        assert!(inject_prefix(&conn).unwrap().is_empty());
    }
}
