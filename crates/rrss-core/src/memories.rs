//! 可编辑长期记忆。不得静默改过滤。日志 target：`rrss_core::memories`。

use crate::error::rules::MemoryProblem;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::ai_store::truncate_chars;
use crate::error::{Error, Result};
use crate::feeds::now_ms;

pub const INJECT_CHARS: usize = 900;
pub const PENDING_BACKLOG_LIMIT: usize = 8;
pub const EXTRACTION_EXISTING_LIMIT: usize = 40;
const EPISODE_BATCH: i64 = 10;
const RECENT_MESSAGES: i64 = 10;
const MAX_USER_MESSAGE_CHARS: usize = 1500;
const MAX_CANDIDATE_CHARS: usize = 240;
const CONTEXT_EXCERPT_CHARS: usize = 260;

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
    pub replaces_id: Option<i64>,
    pub replaces_content: Option<String>,
    pub replaces_pinned: bool,
}

impl Memory {
    pub fn pending(&self) -> bool {
        self.source.as_deref() == Some("pending") || self.strength == 0.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryCandidate {
    pub kind: MemoryKind,
    pub content: String,
    pub replaces_id: Option<i64>,
}

impl From<&str> for MemoryCandidate {
    fn from(content: &str) -> Self {
        Self {
            kind: MemoryKind::Semantic,
            content: content.to_owned(),
            replaces_id: None,
        }
    }
}

impl From<String> for MemoryCandidate {
    fn from(content: String) -> Self {
        Self {
            kind: MemoryKind::Semantic,
            content,
            replaces_id: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExistingMemorySummary {
    pub id: i64,
    pub kind: &'static str,
    pub content: String,
    pub pending: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PreviousTurn {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    pub assistant: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExtractionInput<'a> {
    pub user_message: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_turn: Option<&'a PreviousTurn>,
    pub existing_memories: &'a [ExistingMemorySummary],
}

pub fn insert_memory(
    conn: &Connection,
    kind: MemoryKind,
    content: &str,
    source: &str,
    strength: f64,
) -> Result<i64> {
    if kind == MemoryKind::Episode {
        return insert_global_memory(conn, kind, content, source, strength, None);
    }
    crate::db::atomic_write(conn, |conn| {
        remember_fact(conn, content)?;
        insert_global_memory(conn, kind, content, source, strength, None)
    })
}

fn insert_global_memory(
    conn: &Connection,
    kind: MemoryKind,
    content: &str,
    source: &str,
    strength: f64,
    replaces_id: Option<i64>,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO memories (kind, content, scope, pinned, strength, source, replaces_id, created_at)
         VALUES (?1, ?2, 'global', 0, ?3, ?4, ?5, ?6)",
        params![
            kind.as_str(),
            content,
            strength,
            source,
            replaces_id,
            now_ms()
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn insert_pending_fact(conn: &Connection, content: &str) -> Result<i64> {
    insert_memory(conn, MemoryKind::Semantic, content, "pending", 0.0)
}

/// Strip blockquotes and fenced code blocks while preserving the user's own
/// prose, inline formatting, lists, and questions.
pub fn clean_user_message(text: &str) -> String {
    use pulldown_cmark::{Event, Parser, Tag, TagEnd};

    let mut out = String::new();
    let mut skip_depth = 0usize;
    for event in Parser::new(text) {
        match event {
            Event::Start(Tag::BlockQuote(_) | Tag::CodeBlock(_)) => {
                skip_depth += 1;
            }
            Event::End(TagEnd::BlockQuote(_) | TagEnd::CodeBlock) => {
                skip_depth = skip_depth.saturating_sub(1);
            }
            _ if skip_depth > 0 => {}
            Event::Text(segment) | Event::Code(segment) => out.push_str(&segment),
            Event::SoftBreak | Event::HardBreak => out.push('\n'),
            Event::End(TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::Item) => out.push('\n'),
            _ => {}
        }
    }
    let collapsed = out
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    truncate_chars(&collapsed, MAX_USER_MESSAGE_CHARS)
}

pub use crate::memories_gate::{
    evidence_matches, has_explicit_memory_signal, should_extract_memory,
};

fn normalize_fingerprint(content: &str) -> String {
    content
        .trim()
        .trim_end_matches(['.', '!', '?', '。', '！', '？'])
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

pub(crate) fn remember_fact(conn: &Connection, content: &str) -> Result<bool> {
    let normalized = normalize_fingerprint(content);
    Ok(conn.execute(
        "INSERT OR IGNORE INTO memory_candidate_receipts (fingerprint) VALUES (?1)",
        [crate::ai_store::hash_input(&normalized)],
    )? == 1)
}

pub(crate) fn remember_existing_facts(conn: &Connection) -> Result<()> {
    let mut stmt = conn.prepare("SELECT content FROM memories WHERE kind != 'episode'")?;
    for row in stmt.query_map([], |row| row.get::<_, String>(0))? {
        remember_fact(conn, &row?)?;
    }
    Ok(())
}

pub fn pending_count(conn: &Connection) -> Result<usize> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM memories
         WHERE scope = 'global' AND kind != 'episode'
           AND (COALESCE(source, '') = 'pending' OR strength = 0.0)",
        [],
        |row| row.get(0),
    )?;
    Ok(count.max(0) as usize)
}

pub fn existing_for_extraction(
    conn: &Connection,
    limit: usize,
) -> Result<Vec<ExistingMemorySummary>> {
    let mut stmt = conn.prepare(
        "SELECT id, kind, content,
                CASE WHEN COALESCE(source, '') = 'pending' OR strength = 0.0 THEN 1 ELSE 0 END AS is_pending
         FROM memories
         WHERE scope = 'global' AND kind != 'episode'
           AND COALESCE(source, '') != 'rejected'
         ORDER BY is_pending ASC, pinned DESC, COALESCE(last_used_at, 0) DESC, id DESC
         LIMIT ?1",
    )?;
    let rows = stmt.query_map([limit as i64], |row| {
        let kind_raw: String = row.get(1)?;
        let kind = match kind_raw.as_str() {
            "procedural" => "procedural",
            _ => "semantic",
        };
        Ok(ExistingMemorySummary {
            id: row.get(0)?,
            kind,
            content: row.get(2)?,
            pending: row.get::<_, i64>(3)? != 0,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// Fetch a compact excerpt of the turn immediately preceding the settled turn
/// in the same thread, so the extractor can understand what the user is
/// reacting to or correcting.
pub fn previous_turn_context(
    conn: &Connection,
    thread_id: i64,
    settled_id: i64,
) -> Result<Option<PreviousTurn>> {
    let current_user_id: Option<i64> = conn
        .query_row(
            "SELECT MAX(id) FROM messages
             WHERE thread_id = ?1 AND role = 'user' AND id < ?2",
            params![thread_id, settled_id],
            |row| row.get(0),
        )
        .optional()?
        .flatten();
    let Some(current_user_id) = current_user_id else {
        return Ok(None);
    };
    let mut stmt = conn.prepare(
        "SELECT role, content FROM messages
         WHERE thread_id = ?1 AND id < ?2 AND role IN ('user', 'assistant')
         ORDER BY id DESC LIMIT 2",
    )?;
    let mut recent = stmt
        .query_map(params![thread_id, current_user_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    recent.reverse();
    let assistant = recent
        .iter()
        .rev()
        .find(|(role, _)| role == "assistant")
        .map(|(_, content)| truncate_chars(content.trim(), CONTEXT_EXCERPT_CHARS))
        .filter(|s| !s.is_empty());
    let Some(assistant) = assistant else {
        return Ok(None);
    };
    let user = recent
        .iter()
        .find(|(role, _)| role == "user")
        .map(|(_, content)| truncate_chars(&clean_user_message(content), CONTEXT_EXCERPT_CHARS))
        .filter(|s| !s.is_empty());
    Ok(Some(PreviousTurn { user, assistant }))
}

pub fn insert_new_candidates(
    conn: &Connection,
    candidates: Vec<MemoryCandidate>,
) -> Result<Vec<String>> {
    crate::db::atomic_write(conn, |conn| {
        let mut stored = Vec::new();
        for candidate in candidates {
            if candidate.kind == MemoryKind::Episode {
                continue;
            }
            let content = candidate.content.trim();
            if content.is_empty() || content.chars().count() > MAX_CANDIDATE_CHARS {
                continue;
            }
            if let Some(target_id) = candidate.replaces_id {
                let target_content: Option<String> = conn
                    .query_row(
                        "SELECT content FROM model_memories WHERE id = ?1",
                        [target_id],
                        |row| row.get(0),
                    )
                    .optional()?;
                let Some(target_content) = target_content else {
                    continue;
                };
                if normalize_fingerprint(&target_content) == normalize_fingerprint(content) {
                    continue;
                }
            }
            if !remember_fact(conn, content)? {
                continue;
            }
            if let Some(target_id) = candidate.replaces_id {
                conn.execute(
                    "DELETE FROM memories
                     WHERE replaces_id = ?1
                       AND (COALESCE(source, '') = 'pending' OR strength = 0.0)",
                    [target_id],
                )?;
            }
            insert_global_memory(
                conn,
                candidate.kind,
                content,
                "pending",
                0.0,
                candidate.replaces_id,
            )?;
            stored.push(content.to_owned());
        }
        Ok(stored)
    })
}

pub fn confirm_memory(conn: &Connection, id: i64) -> Result<()> {
    crate::db::atomic_write(conn, |conn| {
        let row: Option<(String, String, Option<i64>)> = conn
            .query_row(
                "SELECT kind, content, replaces_id FROM memories WHERE id = ?1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        let Some((kind, content, replaces_id)) = row else {
            return Err(Error::MemoryNotFound);
        };
        if let Some(target_id) = replaces_id {
            let target_exists: bool = conn
                .query_row(
                    "SELECT 1 FROM model_memories WHERE id = ?1",
                    [target_id],
                    |_| Ok(()),
                )
                .optional()?
                .is_some();
            if target_exists {
                conn.execute(
                    "UPDATE memories
                     SET kind = ?1, content = ?2, embedding = NULL, space_id = NULL
                     WHERE id = ?3",
                    params![kind, content, target_id],
                )?;
                remember_fact(conn, &content)?;
                conn.execute("DELETE FROM memories WHERE id = ?1", [id])?;
                return Ok(());
            }
        }
        conn.execute(
            "UPDATE memories SET source = 'chat', strength = 1.0, replaces_id = NULL WHERE id = ?1",
            [id],
        )?;
        Ok(())
    })
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
        "SELECT m.id, m.kind, m.content, m.scope, m.pinned, m.strength, m.source,
                m.last_used_at, m.created_at,
                CASE WHEN m.embedding IS NOT NULL AND length(m.embedding) > 0 THEN 1 ELSE 0 END,
                r.id, r.content, COALESCE(r.pinned, 0)
         FROM memories m
         LEFT JOIN model_memories r ON r.id = m.replaces_id
         WHERE m.id = ?1",
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
            "SELECT m.id, m.kind, m.content, m.scope, m.pinned, m.strength, m.source,
                    m.last_used_at, m.created_at,
                    CASE WHEN m.embedding IS NOT NULL AND length(m.embedding) > 0 THEN 1 ELSE 0 END,
                    r.id, r.content, COALESCE(r.pinned, 0)
             FROM memories m
             LEFT JOIN model_memories r ON r.id = m.replaces_id
             ORDER BY (CASE WHEN COALESCE(m.source, '') = 'pending' OR m.strength = 0.0 THEN 0 ELSE 1 END),
                      m.pinned DESC, COALESCE(m.last_used_at, 0) DESC, m.id DESC",
        )?;
        let rows = stmt.query_map([], map_memory)?;
        return rows
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into);
    }
    let like = format!("%{q}%");
    let mut stmt = conn.prepare(
        "SELECT m.id, m.kind, m.content, m.scope, m.pinned, m.strength, m.source,
                m.last_used_at, m.created_at,
                CASE WHEN m.embedding IS NOT NULL AND length(m.embedding) > 0 THEN 1 ELSE 0 END,
                r.id, r.content, COALESCE(r.pinned, 0)
         FROM memories m
         LEFT JOIN model_memories r ON r.id = m.replaces_id
         WHERE m.content LIKE ?1 OR COALESCE(r.content, '') LIKE ?1
         ORDER BY (CASE WHEN COALESCE(m.source, '') = 'pending' OR m.strength = 0.0 THEN 0 ELSE 1 END),
                  m.pinned DESC, COALESCE(m.last_used_at, 0) DESC, m.id DESC",
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
                CASE WHEN embedding IS NOT NULL AND length(embedding) > 0 THEN 1 ELSE 0 END,
                NULL, NULL, 0
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
        if kind != MemoryKind::Episode.as_str() {
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
                "replaces_id": m.replaces_id,
            })
        })
        .collect();
    Ok(serde_json::to_string_pretty(&v).unwrap_or_else(|_| "[]".into()))
}

/// 注入 Pi 系统前缀：先按 pinned 与时间在预算内挑选，再按「通用规则 / 背景与偏好」分组输出。
pub fn inject_prefix(conn: &Connection) -> Result<String> {
    let mut stmt = conn.prepare(
        "SELECT kind, content FROM model_memories
         ORDER BY pinned DESC, COALESCE(last_used_at, 0) DESC, id DESC",
    )?;
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    const HEADER: &str = "User memory (confirmed):\n";
    const RULES_HEAD: &str = "Standing rules:\n";
    const FACTS_HEAD: &str = "User background & preferences:\n";

    let mut rules = Vec::new();
    let mut facts = Vec::new();
    let mut used = HEADER.chars().count();
    for (kind, content) in rows {
        let line_len = content.chars().count() + 3; // "- " + "\n"
        let is_rule = kind == MemoryKind::Procedural.as_str();
        let section_head_len = if is_rule && rules.is_empty() {
            RULES_HEAD.chars().count()
        } else if !is_rule && facts.is_empty() {
            FACTS_HEAD.chars().count()
        } else {
            0
        };
        if used + section_head_len + line_len > INJECT_CHARS {
            continue;
        }
        used += section_head_len + line_len;
        if is_rule {
            rules.push(content);
        } else {
            facts.push(content);
        }
    }
    if rules.is_empty() && facts.is_empty() {
        return Ok(String::new());
    }
    let mut out = String::from(HEADER);
    if !rules.is_empty() {
        out.push_str(RULES_HEAD);
        for item in rules {
            out.push_str("- ");
            out.push_str(&item);
            out.push('\n');
        }
    }
    if !facts.is_empty() {
        out.push_str(FACTS_HEAD);
        for item in facts {
            out.push_str("- ");
            out.push_str(&item);
            out.push('\n');
        }
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
        replaces_id: row.get(10)?,
        replaces_content: row.get(11)?,
        replaces_pinned: row.get::<_, i64>(12)? != 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_at;
    use tempfile::tempdir;

    #[test]
    fn clean_user_message_strips_quotes_and_code_blocks_but_keeps_questions_and_inline_code() {
        let raw = "我主要关注 `Rust` 和 **数据库** 新闻，简报里能少放加密货币吗？\n\n> Quoted article text with I always prefer X.\n\n```rust\nfn main() {}\n```";
        let clean = clean_user_message(raw);
        assert!(clean.contains("Rust"));
        assert!(clean.contains("数据库"));
        assert!(clean.contains("简报里能少放加密货币吗？"));
        assert!(!clean.contains("Quoted article"));
        assert!(!clean.contains("fn main"));
    }

    #[test]
    fn trigger_gate_skips_ordinary_qa_and_throttles_backlog() {
        assert!(!should_extract_memory("谢谢", 0));
        assert!(!should_extract_memory("这篇文章的核心观点是什么？", 0));
        assert!(!should_extract_memory("帮我把这段翻译成中文。", 0));
        assert!(!should_extract_memory(
            "What does Wren 12 do when a tab goes over its memory cap?",
            0
        ));

        // Soft signals pass below backlog limit, but pause once backlog is full.
        let soft = "我主要关注 Rust 和数据库方面的新闻，加密货币不太感兴趣。";
        assert!(should_extract_memory(soft, 0));
        assert!(!should_extract_memory(soft, PENDING_BACKLOG_LIMIT));

        // Explicit signals pass even when backlog is full.
        let explicit = "记住，以后讲硬件概念都少堆术语，多打比方。";
        assert!(should_extract_memory(explicit, PENDING_BACKLOG_LIMIT));
        assert!(should_extract_memory(
            "From now on, please lead with the bottom line.",
            PENDING_BACKLOG_LIMIT
        ));
    }

    #[test]
    fn evidence_grounding_tolerates_punctuation_and_rejects_ungrounded_text() {
        let clean = clean_user_message(
            "这段太学术了，你以后讲芯片架构都少堆术语、多打比方。\n\n> I always prefer dark mode.",
        );
        assert!(evidence_matches(
            &clean,
            "你以后讲芯片架构都少堆术语,多打比方"
        ));
        assert!(!evidence_matches(&clean, "I always prefer dark mode"));
        assert!(!evidence_matches(&clean, "太"));
        assert!(!evidence_matches(&clean, "the"));
    }

    #[test]
    fn pending_not_injected_until_confirm_and_groups_procedural_vs_semantic() {
        let dir = tempdir().unwrap();
        let conn = open_at(&dir.path().join("rrss.db")).unwrap();
        let fact_id = insert_pending_fact(&conn, "关注 Rust 与数据库。").unwrap();
        let rule_id = insert_memory(
            &conn,
            MemoryKind::Procedural,
            "先给结论再展开。",
            "pending",
            0.0,
        )
        .unwrap();
        assert!(inject_prefix(&conn).unwrap().is_empty());
        confirm_memory(&conn, fact_id).unwrap();
        confirm_memory(&conn, rule_id).unwrap();
        let prefix = inject_prefix(&conn).unwrap();
        assert!(prefix.contains("Standing rules:\n- 先给结论再展开。\n"));
        assert!(prefix.contains("User background & preferences:\n- 关注 Rust 与数据库。\n"));
        assert!(!get_memory(&conn, fact_id).unwrap().pending());
        delete_memory(&conn, fact_id).unwrap();
        delete_memory(&conn, rule_id).unwrap();
        assert!(inject_prefix(&conn).unwrap().is_empty());
    }

    #[test]
    fn replacement_confirm_updates_target_in_place_and_preserves_pin() {
        let dir = tempdir().unwrap();
        let conn = open_at(&dir.path().join("rrss.db")).unwrap();
        let target = insert_memory(
            &conn,
            MemoryKind::Semantic,
            "主力机使用 Linux。",
            "chat",
            1.0,
        )
        .unwrap();
        set_pinned(&conn, target, true).unwrap();
        // Seed a fake embedding so we verify replacement clears it for re-embedding.
        conn.execute(
            "UPDATE memories SET embedding = X'0000803F' WHERE id = ?1",
            [target],
        )
        .unwrap();

        let stored = insert_new_candidates(
            &conn,
            vec![MemoryCandidate {
                kind: MemoryKind::Procedural,
                content: "主力机使用 Windows 11，命令示例优先用 PowerShell。".into(),
                replaces_id: Some(target),
            }],
        )
        .unwrap();
        assert_eq!(stored.len(), 1);

        // Before confirmation, old memory remains active and candidate is pending.
        let prefix_before = inject_prefix(&conn).unwrap();
        assert!(prefix_before.contains("主力机使用 Linux。"));
        assert!(!prefix_before.contains("Windows 11"));

        let listed = list_memories(&conn, "").unwrap();
        assert_eq!(listed.len(), 2);
        assert!(listed[0].pending());
        assert_eq!(listed[0].replaces_id, Some(target));
        assert_eq!(
            listed[0].replaces_content.as_deref(),
            Some("主力机使用 Linux。")
        );
        assert!(listed[0].replaces_pinned);

        // A second pending update for the same target supersedes the earlier pending update.
        let newer = insert_new_candidates(
            &conn,
            vec![MemoryCandidate {
                kind: MemoryKind::Procedural,
                content: "主力机使用 Windows 11，优先给出 Windows 命令。".into(),
                replaces_id: Some(target),
            }],
        )
        .unwrap();
        assert_eq!(newer.len(), 1);
        let listed = list_memories(&conn, "").unwrap();
        assert_eq!(listed.len(), 2);
        let pending_id = listed[0].id;

        confirm_memory(&conn, pending_id).unwrap();
        let remaining = list_memories(&conn, "").unwrap();
        assert_eq!(remaining.len(), 1);
        let updated = &remaining[0];
        assert_eq!(updated.id, target);
        assert_eq!(updated.kind, MemoryKind::Procedural);
        assert_eq!(
            updated.content,
            "主力机使用 Windows 11，优先给出 Windows 命令。"
        );
        assert!(updated.pinned);
        assert!(!updated.has_embedding);
    }

    #[test]
    fn candidate_fingerprints_survive_rejection_and_confirmation_but_not_clear() {
        let dir = tempdir().unwrap();
        let conn = open_at(&dir.path().join("rrss.db")).unwrap();
        let fact = "I prefer weekly summaries.";
        let propose = |text: &str| {
            insert_new_candidates(
                &conn,
                vec![MemoryCandidate {
                    kind: MemoryKind::Semantic,
                    content: text.to_owned(),
                    replaces_id: None,
                }],
            )
            .unwrap()
        };
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
        let propose = |text: &str| {
            insert_new_candidates(
                &conn,
                vec![MemoryCandidate {
                    kind: MemoryKind::Semantic,
                    content: text.to_owned(),
                    replaces_id: None,
                }],
            )
            .unwrap()
        };
        assert!(propose("I PREFER WEEKLY SUMMARIES!").is_empty());
        update_memory_content(&conn, id, edited).unwrap();
        assert!(propose(original).is_empty());
        assert!(propose(edited).is_empty());
        delete_memory(&conn, id).unwrap();
        assert!(propose(edited).is_empty());
    }

    #[test]
    fn previous_turn_context_reads_the_turn_before_the_current_one() {
        use crate::threads::{self, MessageRole};

        let dir = tempdir().unwrap();
        let conn = open_at(&dir.path().join("rrss.db")).unwrap();
        let thread = threads::create_thread(&conn, None).unwrap().id;
        threads::insert_message(&conn, thread, MessageRole::User, "讲一下芯片架构", None).unwrap();
        threads::insert_message(
            &conn,
            thread,
            MessageRole::Assistant,
            "乱序执行流水线通过重命名寄存器消除假依赖。",
            None,
        )
        .unwrap();
        threads::insert_message(
            &conn,
            thread,
            MessageRole::User,
            "太学术了，以后多打比方。",
            None,
        )
        .unwrap();
        let settled = threads::insert_message(
            &conn,
            thread,
            MessageRole::Assistant,
            "好的，我们可以把它想象成餐厅后厨。",
            None,
        )
        .unwrap()
        .id;

        let prev = previous_turn_context(&conn, thread, settled)
            .unwrap()
            .unwrap();
        assert_eq!(prev.user.as_deref(), Some("讲一下芯片架构"));
        assert!(prev.assistant.contains("乱序执行流水线"));
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
