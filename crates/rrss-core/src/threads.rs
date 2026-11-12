//! 会话与消息。引用快照进该条 `messages`。日志 target：`rrss_core::threads`。

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::feeds::now_ms;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    User,
    Assistant,
    System,
}

impl MessageRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::System => "system",
        }
    }

    pub fn parse_str(value: &str) -> Result<Self> {
        match value {
            "user" => Ok(Self::User),
            "assistant" => Ok(Self::Assistant),
            "system" => Ok(Self::System),
            other => Err(Error::InvalidRule(format!("unknown message role {other}"))),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Thread {
    pub id: i64,
    pub title: Option<String>,
    pub pinned: bool,
    pub folder_id: Option<i64>,
    pub archived_at: Option<i64>,
    pub anchor_kind: Option<String>,
    pub anchor_id: Option<i64>,
    pub pi_session_file: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    /// 这条对话选定的「连接 id + 模型 id」；为空跟随对话默认。连接 id 为 0 表示自有 Pi 配置下的 `provider/id`。
    pub chat_model: Option<(i64, String)>,
    pub chat_thinking: Option<String>,
    pub context_used: Option<i64>,
    pub context_window: Option<i64>,
    pub context_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: i64,
    pub thread_id: i64,
    pub role: MessageRole,
    pub content: String,
    pub citations_json: Option<String>,
    pub created_at: i64,
}

pub fn create_thread(conn: &Connection, title: Option<&str>) -> Result<Thread> {
    let now = now_ms();
    conn.execute(
        "INSERT INTO threads (title, created_at, updated_at) VALUES (?1, ?2, ?3)",
        params![title, now, now],
    )?;
    get_thread(conn, conn.last_insert_rowid())
}

pub fn get_thread(conn: &Connection, id: i64) -> Result<Thread> {
    conn.query_row(
        "SELECT id, title, pinned, anchor_kind, anchor_id, pi_session_file, created_at, updated_at,
                chat_provider_id, chat_model, chat_thinking, context_used, context_window, context_at,
                folder_id, archived_at
         FROM threads WHERE id = ?1",
        [id],
        map_thread,
    )
    .optional()?
    .ok_or(Error::ThreadNotFound)
}

pub fn list_threads(conn: &Connection, limit: i64) -> Result<Vec<Thread>> {
    let mut stmt = conn.prepare(
        "SELECT id, title, pinned, anchor_kind, anchor_id, pi_session_file, created_at, updated_at,
                chat_provider_id, chat_model, chat_thinking, context_used, context_window, context_at,
                folder_id, archived_at
         FROM threads
         ORDER BY pinned DESC, updated_at DESC, id DESC
         LIMIT ?1",
    )?;
    let rows = stmt.query_map([limit], map_thread)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// 查找某个文档锚点已存在的会话；读取页面本身不创建空会话。
pub fn find_anchored_thread(
    conn: &Connection,
    anchor_kind: &str,
    anchor_id: i64,
) -> Result<Option<Thread>> {
    conn.query_row(
        "SELECT id, title, pinned, anchor_kind, anchor_id, pi_session_file, created_at, updated_at,
                chat_provider_id, chat_model, chat_thinking, context_used, context_window, context_at,
                folder_id, archived_at
         FROM threads WHERE anchor_kind = ?1 AND anchor_id = ?2 ORDER BY id LIMIT 1",
        params![anchor_kind, anchor_id],
        map_thread,
    )
    .optional()
    .map_err(Into::into)
}

/// 单条写语句原子检查并插入，避免同时发送时为同一期创建多个线程。
pub fn get_or_create_anchored_thread(
    conn: &Connection,
    title: &str,
    anchor_kind: &str,
    anchor_id: i64,
) -> Result<Thread> {
    crate::db::atomic_write(conn, |conn| {
        if anchor_kind == "article" {
            crate::articles::collect_existing_article(conn, anchor_id)?;
        }
        let now = now_ms();
        conn.execute(
            "INSERT INTO threads (title, anchor_kind, anchor_id, created_at, updated_at)
         SELECT ?1, ?2, ?3, ?4, ?4
         WHERE NOT EXISTS (SELECT 1 FROM threads WHERE anchor_kind = ?2 AND anchor_id = ?3)",
            params![title, anchor_kind, anchor_id, now],
        )?;
        find_anchored_thread(conn, anchor_kind, anchor_id)?.ok_or(Error::ThreadNotFound)
    })
}

/// 深度对话列表只包含无文档锚点的线程；简报追问在对应期内恢复。
pub fn list_unanchored_threads(conn: &Connection, limit: i64) -> Result<Vec<Thread>> {
    let mut stmt = conn.prepare(
        "SELECT id, title, pinned, anchor_kind, anchor_id, pi_session_file, created_at, updated_at,
                chat_provider_id, chat_model, chat_thinking, context_used, context_window, context_at,
                folder_id, archived_at
         FROM threads WHERE anchor_kind IS NULL AND archived_at IS NULL
           AND EXISTS (SELECT 1 FROM messages WHERE messages.thread_id = threads.id)
         ORDER BY pinned DESC, updated_at DESC, id DESC LIMIT ?1",
    )?;
    let rows = stmt.query_map([limit], map_thread)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

pub fn list_archived_threads(conn: &Connection) -> Result<Vec<Thread>> {
    let mut stmt = conn.prepare(
        "SELECT id, title, pinned, anchor_kind, anchor_id, pi_session_file, created_at, updated_at,
                chat_provider_id, chat_model, chat_thinking, context_used, context_window, context_at,
                folder_id, archived_at
         FROM threads WHERE anchor_kind IS NULL AND archived_at IS NOT NULL
         ORDER BY archived_at DESC, id DESC",
    )?;
    Ok(stmt
        .query_map([], map_thread)?
        .collect::<rusqlite::Result<_>>()?)
}

pub fn set_thread_title(conn: &Connection, id: i64, title: &str) -> Result<()> {
    let n = conn.execute(
        "UPDATE threads SET title = ?1, updated_at = ?2 WHERE id = ?3",
        params![title, now_ms(), id],
    )?;
    if n == 0 {
        Err(Error::ThreadNotFound)
    } else {
        Ok(())
    }
}

pub fn set_pi_session_file(conn: &Connection, id: i64, file: Option<&str>) -> Result<()> {
    let n = conn.execute(
        "UPDATE threads SET pi_session_file = ?1, updated_at = ?2 WHERE id = ?3",
        params![file, now_ms(), id],
    )?;
    if n == 0 {
        Err(Error::ThreadNotFound)
    } else {
        Ok(())
    }
}

/// 记下这条对话选定的模型；`None` 清除，之后跟随对话默认。不更新 `updated_at`，换模型不算新消息。
pub fn set_chat_model(conn: &Connection, id: i64, choice: Option<(i64, &str)>) -> Result<()> {
    let (provider, model) = match choice {
        Some((provider, model)) if provider >= 0 && !model.trim().is_empty() => {
            (Some(provider), Some(model.trim()))
        }
        _ => (None, None),
    };
    let n = conn.execute(
        "UPDATE threads SET chat_provider_id = ?1, chat_model = ?2 WHERE id = ?3",
        params![provider, model, id],
    )?;
    if n == 0 {
        Err(Error::ThreadNotFound)
    } else {
        Ok(())
    }
}

pub fn set_chat_thinking(conn: &Connection, id: i64, thinking: Option<&str>) -> Result<()> {
    let n = conn.execute(
        "UPDATE threads SET chat_thinking = ?1 WHERE id = ?2",
        params![thinking, id],
    )?;
    if n == 0 {
        Err(Error::ThreadNotFound)
    } else {
        Ok(())
    }
}

pub fn set_context(
    conn: &Connection,
    id: i64,
    used: i64,
    window: Option<i64>,
    at: i64,
) -> Result<()> {
    let n = conn.execute(
        "UPDATE threads SET context_used = ?1, context_window = ?2, context_at = ?3 WHERE id = ?4",
        params![used, window, at, id],
    )?;
    if n == 0 {
        Err(Error::ThreadNotFound)
    } else {
        Ok(())
    }
}

pub fn set_context_window(conn: &Connection, id: i64, window: Option<i64>) -> Result<()> {
    let n = conn.execute(
        "UPDATE threads SET context_window = ?1 WHERE id = ?2",
        params![window, id],
    )?;
    if n == 0 {
        Err(Error::ThreadNotFound)
    } else {
        Ok(())
    }
}

pub fn insert_message(
    conn: &Connection,
    thread_id: i64,
    role: MessageRole,
    content: &str,
    citations_json: Option<&str>,
) -> Result<Message> {
    crate::db::atomic_write(conn, |conn| {
        get_thread(conn, thread_id)?;
        for citation in crate::cite::parse_citations_json(citations_json) {
            crate::articles::collect_existing_article(conn, citation.article_id)?;
        }
        let now = now_ms();
        conn.execute(
            "INSERT INTO messages (thread_id, role, content, citations_json, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
            params![thread_id, role.as_str(), content, citations_json, now],
        )?;
        conn.execute(
            "UPDATE threads SET updated_at = ?1 WHERE id = ?2",
            params![now, thread_id],
        )?;
        get_message(conn, conn.last_insert_rowid())
    })
}

/// 发送被运行时拒绝时，仅撤销本次尚未得到回复的用户消息。
pub fn discard_unsent_message(conn: &Connection, thread_id: i64, message_id: i64) -> Result<()> {
    conn.execute(
        "DELETE FROM messages WHERE id = ?1 AND thread_id = ?2 AND role = 'user'
         AND NOT EXISTS (SELECT 1 FROM messages WHERE thread_id = ?2 AND id > ?1)",
        params![message_id, thread_id],
    )?;
    Ok(())
}

pub fn get_message(conn: &Connection, id: i64) -> Result<Message> {
    conn.query_row(
        "SELECT id, thread_id, role, content, citations_json, created_at FROM messages WHERE id = ?1",
        [id],
        map_message,
    )
    .optional()?
    .ok_or(Error::ThreadNotFound)
}

pub fn list_messages(conn: &Connection, thread_id: i64) -> Result<Vec<Message>> {
    get_thread(conn, thread_id)?;
    let mut stmt = conn.prepare(
        "SELECT id, thread_id, role, content, citations_json, created_at
         FROM messages WHERE thread_id = ?1
         ORDER BY created_at ASC, id ASC",
    )?;
    let rows = stmt.query_map([thread_id], map_message)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// 历史窗口：返回 `before_id` 之前（不含）最新的至多 `limit` 条，按 id 升序；
/// 第二个值表示窗口之前还有更早的消息。`before_id` 为 `None` 时取最新一页。
pub fn list_messages_before(
    conn: &Connection,
    thread_id: i64,
    before_id: Option<i64>,
    limit: usize,
) -> Result<(Vec<Message>, bool)> {
    get_thread(conn, thread_id)?;
    let limit = limit.max(1);
    let mut stmt = conn.prepare(
        "SELECT id, thread_id, role, content, citations_json, created_at
         FROM messages WHERE thread_id = ?1 AND (?2 IS NULL OR id < ?2)
         ORDER BY id DESC LIMIT ?3",
    )?;
    let rows = stmt.query_map(params![thread_id, before_id, limit as i64 + 1], map_message)?;
    let mut rows = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    let has_more = rows.len() > limit;
    rows.truncate(limit);
    rows.reverse();
    Ok((rows, has_more))
}

/// 会话列表标题用的第一条非空消息，不读取整段历史。
pub fn first_message_text(conn: &Connection, thread_id: i64) -> Result<Option<String>> {
    conn.query_row(
        "SELECT content FROM messages WHERE thread_id = ?1 AND TRIM(content) <> ''
         ORDER BY id ASC LIMIT 1",
        [thread_id],
        |row| row.get(0),
    )
    .optional()
    .map_err(Into::into)
}

fn map_thread(row: &rusqlite::Row<'_>) -> rusqlite::Result<Thread> {
    Ok(Thread {
        id: row.get(0)?,
        title: row.get(1)?,
        pinned: row.get::<_, i64>(2)? != 0,
        anchor_kind: row.get(3)?,
        anchor_id: row.get(4)?,
        pi_session_file: row.get(5)?,
        created_at: row.get(6)?,
        updated_at: row.get(7)?,
        chat_thinking: row.get(10)?,
        context_used: row.get(11)?,
        context_window: row.get(12)?,
        context_at: row.get(13)?,
        folder_id: row.get(14)?,
        archived_at: row.get(15)?,
        chat_model: match (
            row.get::<_, Option<i64>>(8)?,
            row.get::<_, Option<String>>(9)?,
        ) {
            (Some(provider), Some(model)) if !model.trim().is_empty() => Some((provider, model)),
            _ => None,
        },
    })
}

fn map_message(row: &rusqlite::Row<'_>) -> rusqlite::Result<Message> {
    let role_raw: String = row.get(2)?;
    let role = MessageRole::parse_str(&role_raw).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(2, rusqlite::types::Type::Text, Box::new(err))
    })?;
    Ok(Message {
        id: row.get(0)?,
        thread_id: row.get(1)?,
        role,
        content: row.get(3)?,
        citations_json: row.get(4)?,
        created_at: row.get(5)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{PooledConnection, open_at};
    use tempfile::tempdir;

    fn open_temp() -> (tempfile::TempDir, PooledConnection) {
        let dir = tempdir().unwrap();
        let conn = open_at(&dir.path().join("rrss.db")).unwrap();
        (dir, conn)
    }

    #[test]
    fn anchored_threads_are_reused_and_separate_from_chat() {
        let (_dir, conn) = open_temp();
        assert!(find_anchored_thread(&conn, "digest", 1).unwrap().is_none());
        assert!(list_threads(&conn, 80).unwrap().is_empty());
        let first = get_or_create_anchored_thread(&conn, "日报追问", "digest", 1).unwrap();
        insert_message(&conn, first.id, MessageRole::User, "展开第二条", None).unwrap();
        let again = get_or_create_anchored_thread(&conn, "仍是当期", "digest", 1).unwrap();
        assert_eq!(first.id, again.id);
        assert_eq!(list_messages(&conn, again.id).unwrap().len(), 1);
        let other = get_or_create_anchored_thread(&conn, "上期追问", "digest", 2).unwrap();
        assert_ne!(first.id, other.id);
        assert!(list_messages(&conn, other.id).unwrap().is_empty());
        let chat = create_thread(&conn, Some("普通对话")).unwrap();
        assert_eq!(list_threads(&conn, 80).unwrap().len(), 3);
        assert!(list_unanchored_threads(&conn, 80).unwrap().is_empty());
        insert_message(&conn, chat.id, MessageRole::User, "你好", None).unwrap();
        assert_eq!(list_unanchored_threads(&conn, 80).unwrap()[0].id, chat.id);
        assert_eq!(list_unanchored_threads(&conn, 80).unwrap().len(), 1);
    }

    #[test]
    fn chat_model_is_stored_per_thread_and_cleared() {
        let (_dir, conn) = open_temp();
        let thread = create_thread(&conn, Some("对话")).unwrap();
        assert_eq!(thread.chat_model, None);
        set_chat_model(&conn, thread.id, Some((3, " qwen2.5:14b "))).unwrap();
        assert_eq!(
            get_thread(&conn, thread.id).unwrap().chat_model,
            Some((3, "qwen2.5:14b".into()))
        );
        set_chat_model(&conn, thread.id, None).unwrap();
        assert_eq!(get_thread(&conn, thread.id).unwrap().chat_model, None);
        assert!(set_chat_model(&conn, 999, None).is_err());
    }

    #[test]
    fn concurrent_anchor_creation_is_atomic() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("rrss.db");
        let _conn = open_at(&path).unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let tasks: Vec<_> = (0..2)
            .map(|_| {
                let path = path.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    let conn = open_at(&path).unwrap();
                    barrier.wait();
                    get_or_create_anchored_thread(&conn, "日报", "digest", 42)
                        .unwrap()
                        .id
                })
            })
            .collect();
        let ids: Vec<_> = tasks.into_iter().map(|t| t.join().unwrap()).collect();
        assert_eq!(ids[0], ids[1]);
        assert_eq!(list_threads(&_conn, 80).unwrap().len(), 1);
    }

    #[test]
    fn failed_send_cleanup_only_removes_its_unanswered_user_message() {
        let (_dir, conn) = open_temp();
        let thread = create_thread(&conn, Some("普通对话")).unwrap();
        let user = insert_message(&conn, thread.id, MessageRole::User, "已回答", None).unwrap();
        let reply = insert_message(&conn, thread.id, MessageRole::Assistant, "回答", None).unwrap();
        discard_unsent_message(&conn, thread.id, user.id).unwrap();
        discard_unsent_message(&conn, thread.id, reply.id).unwrap();
        let failed = insert_message(&conn, thread.id, MessageRole::User, "未发出", None).unwrap();
        discard_unsent_message(&conn, thread.id, failed.id).unwrap();
        assert_eq!(list_messages(&conn, thread.id).unwrap().len(), 2);
    }

    #[test]
    fn citations_snapshot_stays_on_message() {
        let (_dir, conn) = open_temp();
        let feed = crate::feeds::manual_feed(&conn).unwrap();
        conn.execute(
            "INSERT INTO articles(id,feed_id,title,created_at) VALUES(1,?1,'old title',0)",
            [feed],
        )
        .unwrap();
        let thread = create_thread(&conn, Some("查证图书馆")).unwrap();
        let snap = r#"[{"kind":"article","article_id":1,"title":"old title"}]"#;
        insert_message(&conn, thread.id, MessageRole::User, "请查证", Some(snap)).unwrap();
        insert_message(&conn, thread.id, MessageRole::Assistant, "好", None).unwrap();
        let msgs = list_messages(&conn, thread.id).unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].citations_json.as_deref(), Some(snap));
        assert_eq!(msgs[1].role, MessageRole::Assistant);
        set_pi_session_file(&conn, thread.id, Some("sess.jsonl")).unwrap();
        assert_eq!(
            get_thread(&conn, thread.id)
                .unwrap()
                .pi_session_file
                .as_deref(),
            Some("sess.jsonl")
        );
        assert!(matches!(
            insert_message(&conn, 999, MessageRole::User, "x", None),
            Err(Error::ThreadNotFound)
        ));
    }

    #[test]
    fn deleted_article_citations_and_anchors_preserve_chat_snapshots() {
        let (_dir, conn) = open_temp();
        let feed = crate::feeds::manual_feed(&conn).unwrap();
        conn.execute(
            "INSERT INTO articles(id,feed_id,title,created_at,expires_at) VALUES(1,?1,'deleted',0,1)",
            [feed],
        ).unwrap();
        conn.execute("DELETE FROM articles WHERE id=1", []).unwrap();
        let thread = get_or_create_anchored_thread(&conn, "Deleted article", "article", 1).unwrap();
        let snap =
            r#"[{"kind":"article","article_id":1,"title":"deleted","excerpt":"Saved text"}]"#;
        let message =
            insert_message(&conn, thread.id, MessageRole::User, "Question", Some(snap)).unwrap();
        assert_eq!(
            get_message(&conn, message.id)
                .unwrap()
                .citations_json
                .as_deref(),
            Some(snap)
        );
        assert_eq!(
            crate::cite::parse_citations_json(Some(snap))[0].article_id,
            1
        );
        assert!(matches!(
            crate::articles::collect_article(&conn, 1),
            Err(Error::ArticleNotFound)
        ));
    }

    #[test]
    fn message_window_pages_backwards_by_id_without_gaps_or_duplicates() {
        let (_dir, conn) = open_temp();
        let thread = create_thread(&conn, Some("窗口")).unwrap();
        let mut ids = Vec::new();
        for i in 0..7 {
            ids.push(
                insert_message(&conn, thread.id, MessageRole::User, &format!("m{i}"), None)
                    .unwrap()
                    .id,
            );
        }
        let (latest, more) = list_messages_before(&conn, thread.id, None, 3).unwrap();
        assert_eq!(latest.iter().map(|m| m.id).collect::<Vec<_>>(), ids[4..]);
        assert!(more);
        let (middle, more) = list_messages_before(&conn, thread.id, Some(latest[0].id), 3).unwrap();
        assert_eq!(middle.iter().map(|m| m.id).collect::<Vec<_>>(), ids[1..4]);
        assert!(more);
        let (oldest, more) = list_messages_before(&conn, thread.id, Some(middle[0].id), 3).unwrap();
        assert_eq!(oldest.iter().map(|m| m.id).collect::<Vec<_>>(), ids[..1]);
        assert!(!more);
        let (all, more) = list_messages_before(&conn, thread.id, None, 7).unwrap();
        assert_eq!(all.len(), 7);
        assert!(!more);
        assert_eq!(
            list_messages_before(&conn, thread.id, None, 0)
                .unwrap()
                .0
                .len(),
            1
        );
        assert!(list_messages_before(&conn, 9999, None, 3).is_err());
        assert_eq!(
            first_message_text(&conn, thread.id).unwrap().as_deref(),
            Some("m0")
        );
        let empty = create_thread(&conn, None).unwrap();
        assert_eq!(first_message_text(&conn, empty.id).unwrap(), None);
    }
}
