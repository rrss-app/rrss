//! 可编辑长期记忆。不得静默改过滤。日志 target：`rrss_core::memories`。

use crate::error::rules::MemoryProblem;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use unicode_segmentation::UnicodeSegmentation;

use crate::ai_store::truncate_chars;
use crate::error::{Error, Result};
use crate::feeds::now_ms;

pub const INJECT_CHARS: usize = 900;
const EPISODE_BATCH: i64 = 10;
const RECENT_MESSAGES: i64 = 10;

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
    if kind != MemoryKind::Semantic {
        return insert_global_memory(conn, kind, content, source, strength);
    }
    crate::db::atomic_write(conn, |conn| {
        remember_fact(conn, content)?;
        insert_global_memory(conn, kind, content, source, strength)
    })
}

fn insert_global_memory(
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

pub fn user_statements(text: &str) -> Vec<&str> {
    use pulldown_cmark::{Event, Parser, Tag, TagEnd};

    let mut paragraphs = Vec::new();
    let mut depth = 0usize;
    let mut start = None;
    for (event, range) in Parser::new(text).into_offset_iter() {
        match event {
            Event::Start(tag) => {
                if depth == 0 && tag == Tag::Paragraph {
                    start = Some(range.start);
                } else {
                    start = None;
                }
                depth += 1;
            }
            Event::End(tag) => {
                depth -= 1;
                if depth == 0
                    && tag == TagEnd::Paragraph
                    && let Some(start) = start.take()
                {
                    paragraphs.push(&text[start..range.end]);
                }
            }
            Event::Text(_) | Event::SoftBreak | Event::HardBreak => {}
            _ => start = None,
        }
    }
    paragraphs
        .into_iter()
        .flat_map(str::unicode_sentences)
        .map(str::trim)
        .filter(|text| {
            !text.ends_with(['?', '？']) && !text.starts_with(['"', '“', '‘', '「', '『'])
        })
        .filter(|text| text.chars().count() <= 280)
        .take(32)
        .collect()
}

pub(crate) fn remember_fact(conn: &Connection, content: &str) -> Result<bool> {
    let normalized = content
        .trim()
        .trim_end_matches(['.', '!', '?', '。', '！', '？'])
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    Ok(conn.execute(
        "INSERT OR IGNORE INTO memory_candidate_receipts (fingerprint) VALUES (?1)",
        [crate::ai_store::hash_input(&normalized)],
    )? == 1)
}

pub(crate) fn remember_existing_facts(conn: &Connection) -> Result<()> {
    let mut stmt = conn.prepare("SELECT content FROM memories WHERE kind = 'semantic'")?;
    for row in stmt.query_map([], |row| row.get::<_, String>(0))? {
        remember_fact(conn, &row?)?;
    }
    Ok(())
}

pub fn insert_new_candidates(conn: &Connection, facts: Vec<String>) -> Result<Vec<String>> {
    crate::db::atomic_write(conn, |conn| {
        let mut stored = Vec::new();
        for fact in facts {
            if remember_fact(conn, &fact)? {
                insert_global_memory(conn, MemoryKind::Semantic, &fact, "pending", 0.0)?;
                stored.push(fact);
            }
        }
        Ok(stored)
    })
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
    if conn.execute("DELETE FROM memories WHERE id = ?1", [id])? == 0 {
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
    let mut stmt = conn.prepare(
        "SELECT id, kind, content, scope, pinned, strength, source, last_used_at, created_at,
                CASE WHEN embedding IS NOT NULL AND length(embedding) > 0 THEN 1 ELSE 0 END
         FROM model_memories WHERE content LIKE ?1
         ORDER BY pinned DESC, COALESCE(last_used_at, 0) DESC, id DESC LIMIT ?2",
    )?;
    let like = stmt
        .query_map(
            params![format!("%{}%", query.trim()), limit.max(0)],
            map_memory,
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
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
    crate::db::atomic_write(conn, |conn| {
        let kind = conn
            .query_row(
                "UPDATE memories SET content = ?1, embedding = NULL, space_id = NULL
                 WHERE id = ?2 RETURNING kind",
                params![content, id],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .ok_or(Error::MemoryNotFound)?;
        if kind == MemoryKind::Semantic.as_str() {
            remember_fact(conn, content)?;
        }
        Ok(())
    })
}

pub fn inject_preview(conn: &Connection) -> Result<(String, usize)> {
    let text = inject_prefix(conn)?;
    let chars = text.chars().count();
    Ok((text, chars))
}

pub fn clear_memories(conn: &Connection) -> Result<usize> {
    crate::db::atomic_write(conn, |conn| {
        conn.execute_batch(
            "INSERT INTO memory_episode_progress (thread_id, through_message_id)
                 SELECT thread_id, MAX(id) FROM messages GROUP BY thread_id
             ON CONFLICT(thread_id) DO UPDATE SET
                 through_message_id = max(through_message_id, excluded.through_message_id),
                 generation = randomblob(16);
             DELETE FROM memory_candidate_receipts;",
        )?;
        Ok(conn.execute("DELETE FROM memories", [])?)
    })
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
        "SELECT content FROM model_memories
         ORDER BY pinned DESC, COALESCE(last_used_at, 0) DESC, id DESC",
    )?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    let mut out = String::from("User memory (confirmed):\n");
    for row in rows {
        let line = format!("- {}\n", row?);
        if out.chars().count() + line.chars().count() > INJECT_CHARS {
            continue;
        }
        out.push_str(&line);
    }
    if out.trim() == "User memory (confirmed):" {
        return Ok(String::new());
    }
    Ok(truncate_chars(&out, INJECT_CHARS))
}

pub fn episode_context(conn: &Connection, session_file: &str) -> Result<String> {
    let mut stmt = conn.prepare(
        "SELECT substr(content, 1, 300) FROM memories
         WHERE kind = 'episode' AND scope = 'thread' AND strength > 0
           AND COALESCE(source, '') NOT IN ('pending', 'rejected')
           AND scope_id = (SELECT min(id) FROM threads WHERE pi_session_file = ?1 HAVING count(*) = 1)
         ORDER BY id DESC LIMIT 3",
    )?;
    let mut episodes = stmt
        .query_map([session_file], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    episodes.reverse();
    Ok(episodes.join("\n\n"))
}

pub struct EpisodeBatch {
    pub lines: Vec<(String, String)>,
    thread_id: i64,
    generation: Vec<u8>,
    through_before: i64,
    through_after: i64,
}

pub fn episode_batch(
    conn: &Connection,
    thread_id: i64,
    settled_id: i64,
) -> Result<Option<EpisodeBatch>> {
    crate::db::atomic_write(conn, |conn| {
        conn.execute(
            "INSERT OR IGNORE INTO memory_episode_progress (thread_id)
             SELECT thread_id FROM messages WHERE thread_id = ?1 AND id = ?2 AND role = 'assistant'",
            params![thread_id, settled_id],
        )?;
        let Some((through_before, generation)) = conn
            .query_row(
                "SELECT through_message_id, generation FROM memory_episode_progress WHERE thread_id = ?1
                 AND EXISTS (SELECT 1 FROM messages WHERE thread_id = ?1 AND id = ?2 AND role = 'assistant')",
                params![thread_id, settled_id],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?)),
            )
            .optional()?
        else {
            return Ok(None);
        };
        let mut stmt = conn.prepare(
            "SELECT id, role, substr(content, 1, 200) FROM messages
             WHERE thread_id = ?1 AND id > ?2
               AND id <= (SELECT id FROM messages WHERE thread_id = ?1 AND id <= ?3
                          ORDER BY id DESC LIMIT 1 OFFSET ?4)
             ORDER BY id LIMIT ?5",
        )?;
        let mut rows = stmt.query(params![
            thread_id,
            through_before,
            settled_id,
            RECENT_MESSAGES,
            EPISODE_BATCH
        ])?;
        let mut lines = Vec::new();
        let mut through_after = through_before;
        while let Some(row) = rows.next()? {
            through_after = row.get(0)?;
            lines.push((row.get(1)?, row.get(2)?));
        }
        Ok(
            (lines.len() as i64 == EPISODE_BATCH).then_some(EpisodeBatch {
                lines,
                thread_id,
                generation,
                through_before,
                through_after,
            }),
        )
    })
}

pub fn store_episode(conn: &Connection, batch: &EpisodeBatch, body: &str) -> Result<bool> {
    crate::db::atomic_write(conn, |conn| {
        let saved = conn.execute(
            "UPDATE memory_episode_progress SET through_message_id = ?1
             WHERE thread_id = ?2 AND generation = ?3 AND through_message_id = ?4",
            params![
                batch.through_after,
                batch.thread_id,
                batch.generation,
                batch.through_before
            ],
        )?;
        if saved == 0 {
            return Ok(false);
        }
        conn.execute(
            "INSERT INTO memories (kind, content, scope, scope_id, source, created_at)
             VALUES ('episode', ?1, 'thread', ?2, 'compress', ?3)",
            params![truncate_chars(body, 300), batch.thread_id, now_ms()],
        )?;
        Ok(true)
    })
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

    #[test]
    fn candidate_fingerprints_survive_rejection_and_confirmation_but_not_clear() {
        let dir = tempdir().unwrap();
        let conn = open_at(&dir.path().join("rrss.db")).unwrap();
        let fact = "I prefer weekly summaries.";
        let propose = |text: &str| insert_new_candidates(&conn, vec![text.to_owned()]).unwrap();
        assert_eq!(propose(fact), [fact]);
        assert!(propose("I PREFER  WEEKLY SUMMARIES!").is_empty());
        let first = list_memories(&conn, "").unwrap()[0].id;
        delete_memory(&conn, first).unwrap();
        assert!(propose(fact).is_empty());
        assert!(list_memories(&conn, "").unwrap().is_empty());

        let other = "I prefer offline reading.";
        assert_eq!(propose(other), [other]);
        let id = list_memories(&conn, "").unwrap()[0].id;
        confirm_memory(&conn, id).unwrap();
        assert!(propose(other).is_empty());
        clear_memories(&conn).unwrap();
        assert_eq!(propose(other), [other]);
        assert_eq!(propose(fact), [fact]);
    }

    #[test]
    fn directly_added_and_edited_facts_register_persistent_receipts() {
        let dir = tempdir().unwrap();
        let conn = open_at(&dir.path().join("rrss.db")).unwrap();
        let original = "I prefer weekly summaries.";
        let edited = "I prefer offline summaries.";
        let id = insert_pending_fact(&conn, original).unwrap();
        let propose = |text: &str| insert_new_candidates(&conn, vec![text.to_owned()]).unwrap();
        assert!(propose("I PREFER WEEKLY SUMMARIES!").is_empty());
        update_memory_content(&conn, id, edited).unwrap();
        assert!(propose(original).is_empty());
        assert!(propose(edited).is_empty());
        delete_memory(&conn, id).unwrap();
        assert!(propose(edited).is_empty());
    }

    #[test]
    fn episode_write_is_fenced_by_clear_and_thread_identity() {
        use crate::threads::{self, MessageRole};

        let dir = tempdir().unwrap();
        let conn = open_at(&dir.path().join("rrss.db")).unwrap();
        let thread = threads::create_thread(&conn, None).unwrap().id;
        let fill = |thread, turns| {
            let mut settled = 0;
            for _ in 0..turns {
                threads::insert_message(&conn, thread, MessageRole::User, "question", None)
                    .unwrap();
                settled =
                    threads::insert_message(&conn, thread, MessageRole::Assistant, "answer", None)
                        .unwrap()
                        .id;
            }
            settled
        };
        let first = fill(thread, 10);
        let stale = episode_batch(&conn, thread, first).unwrap().unwrap();
        clear_memories(&conn).unwrap();
        assert!(!store_episode(&conn, &stale, "discarded").unwrap());
        let second = fill(thread, 5);
        assert!(episode_batch(&conn, thread, second).unwrap().is_none());
        let third = fill(thread, 5);
        let valid = episode_batch(&conn, thread, third).unwrap().unwrap();
        assert!(store_episode(&conn, &valid, "original thread context").unwrap());
        assert!(!store_episode(&conn, &valid, "duplicate response").unwrap());
        let fourth = fill(thread, 5);
        let stale = episode_batch(&conn, thread, fourth).unwrap().unwrap();
        assert_eq!(list_memories(&conn, "").unwrap().len(), 1);

        conn.execute("DELETE FROM threads WHERE id = ?1", [thread])
            .unwrap();
        let replacement = threads::create_thread(&conn, None).unwrap().id;
        assert_eq!(thread, replacement);
        let last = fill(replacement, 10);
        assert!(episode_batch(&conn, replacement, last).unwrap().is_some());
        assert!(!store_episode(&conn, &stale, "stale thread").unwrap());
        threads::set_pi_session_file(&conn, replacement, Some("replacement.jsonl")).unwrap();
        assert!(
            episode_context(&conn, "replacement.jsonl")
                .unwrap()
                .is_empty()
        );
        assert!(list_memories(&conn, "").unwrap().is_empty());
    }
}
