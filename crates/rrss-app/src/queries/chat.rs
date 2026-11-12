use rusqlite::Connection;

use rrss_core::parse_citations_json;
use rrss_core::threads::{self, MessageRole};

use super::ai::markdown_document_json;
use crate::views::*;

pub fn threads(conn: &Connection) -> rrss_core::Result<Vec<ThreadView>> {
    Ok(thread_list(conn)?.threads)
}

pub fn thread_list(conn: &Connection) -> rrss_core::Result<ThreadListView> {
    rrss_core::db::read_snapshot(conn, |conn| {
        Ok(ThreadListView {
            folders: thread_folders(conn)?,
            threads: threads::list_unanchored_threads(conn, 1000)?
                .into_iter().map(|t| thread_view(conn, t)).collect(),
            archived_count: conn.query_row(
                "SELECT COUNT(*) FROM threads WHERE anchor_kind IS NULL AND archived_at IS NOT NULL",
                [], |row| row.get(0),
            )?,
        })
    })
}

pub fn archived_threads(conn: &Connection) -> rrss_core::Result<Vec<ThreadView>> {
    Ok(threads::list_archived_threads(conn)?
        .into_iter()
        .map(|t| thread_view(conn, t))
        .collect())
}

pub fn search_threads(
    conn: &Connection,
    query: &str,
    limit: i64,
) -> rrss_core::Result<Vec<ThreadSearchHitView>> {
    use rrss_core::search;
    let query = search::normalize_search_query(query);
    if query.is_empty() || limit == 0 {
        return Ok(Vec::new());
    }
    rrss_core::db::read_snapshot(conn, |conn| {
        // 先物化 BM25，避免 FTS 辅助函数在窗口函数上下文中执行。
        let mut stmt = conn.prepare(r#"
            WITH fts AS MATERIALIZED (
                SELECT rowid, bm25(messages_fts) AS relevance
                FROM messages_fts WHERE messages_fts MATCH ?1
            ), candidates AS (
                SELECT rowid AS id, relevance FROM fts
                UNION ALL
                SELECT id, 0.0 FROM messages
                WHERE ?2 AND role IN ('user', 'assistant') AND content LIKE ?3 ESCAPE '\'
                    AND id NOT IN (SELECT rowid FROM fts)
            ), hits AS (
                SELECT m.id, m.thread_id, c.relevance,
                    ROW_NUMBER() OVER (PARTITION BY m.thread_id
                        ORDER BY c.relevance, m.created_at DESC, m.id DESC) AS n
                FROM candidates c JOIN messages m ON m.id=c.id JOIN threads t ON t.id=m.thread_id
                WHERE t.anchor_kind IS NULL AND t.archived_at IS NULL
                    AND m.role IN ('user', 'assistant')
            )
            SELECT t.id, t.title, t.folder_id, t.created_at, t.updated_at,
                h.id, h.relevance, COALESCE(m.role, ''), COALESCE(m.content, ''), m.created_at
            FROM threads t LEFT JOIN hits h ON h.thread_id=t.id AND h.n=1
            LEFT JOIN messages m ON m.id=COALESCE(h.id, (
                SELECT id FROM messages WHERE thread_id=t.id AND role IN ('user', 'assistant')
                ORDER BY id DESC LIMIT 1
            ))
            WHERE t.anchor_kind IS NULL AND t.archived_at IS NULL
                AND (h.id IS NOT NULL OR TRIM(COALESCE(t.title, ''))='' OR t.title LIKE ?3 ESCAPE '\')
        "#)?;
        let mut rows = stmt.query(rusqlite::params![
            search::fts_phrase(&query).unwrap_or_else(|| "\"\"".into()),
            search::needs_like_fallback(&query),
            format!("%{}%", search::like_escape(&query)),
        ])?;
        let mut hits = Vec::new();
        while let Some(row) = rows.next()? {
            let thread_id = row.get(0)?;
            let stored: Option<String> = row.get(1)?;
            let title = thread_list_title(conn, thread_id, stored.as_deref());
            let title_match = search::contains_query(&title, &query);
            let message_id: Option<i64> = row.get(5)?;
            if message_id.is_none() && !title_match {
                continue;
            }
            let role: String = row.get(7)?;
            let relevance =
                row.get::<_, Option<f64>>(6)?.unwrap_or(0.0) - if title_match { 1.0 } else { 0.0 };
            hits.push((
                relevance,
                role == "assistant",
                ThreadSearchHitView {
                    thread_id,
                    title,
                    folder_id: row.get(2)?,
                    message_id,
                    role: if message_id.is_some() {
                        role
                    } else {
                        String::new()
                    },
                    snippet: row.get(8)?,
                    created_at: if message_id.is_some() {
                        row.get(9)?
                    } else {
                        row.get(3)?
                    },
                    updated_at: row.get(4)?,
                    title_match,
                },
            ));
        }
        hits.sort_by(|a, b| {
            a.0.total_cmp(&b.0)
                .then(b.2.updated_at.cmp(&a.2.updated_at))
                .then(b.2.thread_id.cmp(&a.2.thread_id))
        });
        // 沿用文章搜索的 limit 约定：0 为空、负数不限；只转换最终结果的 Markdown。
        if limit > 0 {
            hits.truncate(limit as usize);
        }
        Ok(hits
            .into_iter()
            .map(|(_, assistant, mut hit)| {
                if assistant {
                    hit.snippet = rrss_render::document_to_plain(
                        &rrss_render::markdown_to_document(&hit.snippet),
                    );
                }
                hit.snippet = if hit.message_id.is_some() {
                    search::hit_snippet(&hit.snippet, &query)
                } else {
                    search::truncate_search_text(&hit.snippet, search::SNIPPET_OPENING_GRAPHEMES)
                };
                hit
            })
            .collect())
    })
}

pub fn thread_folders(conn: &Connection) -> rusqlite::Result<Vec<ThreadFolderView>> {
    conn.prepare("SELECT id, name, position, collapsed FROM thread_folders ORDER BY position, id")?
        .query_map([], |row| {
            Ok(ThreadFolderView {
                id: row.get(0)?,
                name: row.get(1)?,
                position: row.get(2)?,
                collapsed: row.get(3)?,
            })
        })?
        .collect()
}

fn thread_view(conn: &Connection, t: threads::Thread) -> ThreadView {
    ThreadView {
        id: t.id,
        title: thread_list_title(conn, t.id, t.title.as_deref()),
        chat_model: choice(&t),
        chat_thinking: t.chat_thinking.clone(),
        context: context(&t),
        folder_id: t.folder_id,
        pinned: t.pinned,
        updated_at: t.updated_at,
        archived_at: t.archived_at,
    }
}

fn context(thread: &threads::Thread) -> Option<ContextUsageView> {
    Some(ContextUsageView {
        used: thread.context_used?,
        window: thread.context_window,
        at: thread.context_at?,
    })
}

fn choice(thread: &threads::Thread) -> Option<ModelChoice> {
    thread
        .chat_model
        .clone()
        .map(|(provider_id, model)| ModelChoice { provider_id, model })
}

fn thread_list_title(conn: &Connection, id: i64, stored: Option<&str>) -> String {
    let named = stored.map(str::trim).filter(|s| !s.is_empty());
    if let Some(label) = named {
        return label.to_string();
    }
    threads::first_message_text(conn, id)
        .ok()
        .flatten()
        .map(|content| {
            let text = content.split_whitespace().collect::<Vec<_>>().join(" ");
            let mut n = 0;
            let mut out = String::new();
            for ch in text.chars() {
                if n >= 20 {
                    out.push('…');
                    break;
                }
                out.push(ch);
                n += 1;
            }
            out
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "对话".into())
}

pub fn digest_thread(conn: &Connection, digest_id: i64) -> anyhow::Result<DigestThreadView> {
    crate::digest::get_digest(conn, Some(digest_id), "daily")?;
    match threads::find_anchored_thread(conn, "digest", digest_id)? {
        Some(thread) => {
            let page = messages(conn, thread.id, None, None)?;
            Ok(DigestThreadView {
                thread_id: thread.id,
                chat_model: choice(&thread),
                chat_thinking: thread.chat_thinking.clone(),
                context: context(&thread),
                messages: page.messages,
                has_more: page.has_more,
            })
        }
        None => Ok(DigestThreadView {
            thread_id: 0,
            chat_model: None,
            chat_thinking: None,
            context: None,
            messages: Vec::new(),
            has_more: false,
        }),
    }
}

/// 一页历史默认条数；前端向前加载与刷新都按这个窗口取，避免长会话整段进 IPC。
pub const MESSAGE_PAGE_DEFAULT: usize = 40;
pub const MESSAGE_PAGE_MAX: usize = 200;

/// 按 id 向前分页的历史窗口；`limit` 缺省 [`MESSAGE_PAGE_DEFAULT`]，上限 [`MESSAGE_PAGE_MAX`]。
pub fn messages(
    conn: &Connection,
    thread_id: i64,
    before_id: Option<i64>,
    limit: Option<i64>,
) -> rrss_core::Result<MessagesPage> {
    let limit = limit
        .filter(|n| *n > 0)
        .map(|n| (n as usize).min(MESSAGE_PAGE_MAX))
        .unwrap_or(MESSAGE_PAGE_DEFAULT);
    let (rows, has_more) = threads::list_messages_before(conn, thread_id, before_id, limit)?;
    let messages = rows
        .into_iter()
        .map(|msg| {
            let cites = parse_citations_json(msg.citations_json.as_deref());
            let first = cites.first();
            let document = if msg.role == MessageRole::Assistant {
                markdown_document_json(&msg.content)
            } else {
                None
            };
            ChatMessageView {
                id: msg.id,
                role: msg.role.as_str().into(),
                text: msg.content,
                streaming: false,
                cite_article_id: first.map(|c| c.article_id).unwrap_or(0),
                cite_title: first.map(|c| c.title.clone()).unwrap_or_default(),
                citations: cites
                    .into_iter()
                    .map(|c| CitationView {
                        kind: c.kind,
                        article_id: c.article_id,
                        title: c.title,
                        feed_title: c.feed_title,
                        url: c.url,
                        excerpt: c.excerpt,
                        block_id: c.block_id,
                    })
                    .collect(),
                document,
            }
        })
        .collect();
    Ok(MessagesPage { messages, has_more })
}

/// 一次最多预览多少篇；对话里一条回复的本地链接远少于这个数。
const PREVIEW_LIMIT: usize = 50;

/// 按 id 批量取本地文章预览（对话里的链接与引用小签悬停时用）。库里已没有的按请求顺序返回 `removed`，
/// 重复与非正 id 忽略；只读，不标已读。
pub fn article_previews(
    conn: &Connection,
    ids: &[i64],
) -> rusqlite::Result<Vec<ArticlePreviewView>> {
    let mut wanted: Vec<i64> = Vec::new();
    for &id in ids {
        if id > 0 && !wanted.contains(&id) && wanted.len() < PREVIEW_LIMIT {
            wanted.push(id);
        }
    }
    if wanted.is_empty() {
        return Ok(Vec::new());
    }
    let slots = std::iter::repeat_n("?", wanted.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT a.id, a.title, COALESCE(f.title, ''), a.summary, a.published_at, a.read, COALESCE(a.canonical_url, '')
         FROM articles a JOIN feeds f ON f.id = a.feed_id WHERE a.id IN ({slots})"
    );
    let mut stmt = conn.prepare(&sql)?;
    let found = stmt
        .query_map(rusqlite::params_from_iter(wanted.iter()), |row| {
            let summary: Option<String> = row.get(3)?;
            Ok(ArticlePreviewView {
                article_id: row.get(0)?,
                title: row.get(1)?,
                feed_title: row.get(2)?,
                summary: summary
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .map(|s| s.chars().take(240).collect()),
                published_at: row.get(4)?,
                read: row.get::<_, i64>(5)? != 0,
                url: row.get(6)?,
                status: "available".into(),
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(wanted
        .into_iter()
        .map(|id| {
            found
                .iter()
                .find(|p| p.article_id == id)
                .cloned()
                .unwrap_or(ArticlePreviewView {
                    article_id: id,
                    title: String::new(),
                    feed_title: String::new(),
                    summary: None,
                    published_at: None,
                    read: false,
                    url: String::new(),
                    status: "removed".into(),
                })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rrss_core::db;

    #[test]
    fn thread_search_matches_phrases_substrings_and_selects_the_best_message() {
        let dir = tempfile::tempdir().unwrap();
        let conn = db::open_at(&dir.path().join("t.db")).unwrap();
        let t = threads::create_thread(&conn, Some("研究")).unwrap();
        let best = threads::insert_message(
            &conn,
            t.id,
            MessageRole::User,
            "Library opening library opening",
            None,
        )
        .unwrap();
        threads::insert_message(
            &conn,
            t.id,
            MessageRole::Assistant,
            &format!("library opening {}", "unrelated ".repeat(100)),
            None,
        )
        .unwrap();
        let chinese = threads::insert_message(
            &conn,
            t.id,
            MessageRole::Assistant,
            "城市图书馆新增 AI 服务，预算 10%",
            None,
        )
        .unwrap();
        let other = threads::create_thread(&conn, Some("其他")).unwrap();
        threads::insert_message(
            &conn,
            other.id,
            MessageRole::User,
            "library grand opening",
            None,
        )
        .unwrap();
        let hits = search_threads(&conn, "  \"library opening\"  ", 20).unwrap();
        assert_eq!(hits.len(), 1, "literal phrase, one result per thread");
        assert_eq!(hits[0].message_id, Some(best.id));
        assert_eq!(hits[0].created_at, best.created_at);
        for query in ["书馆", "图书馆", "AI", "10%"] {
            let hits = search_threads(&conn, query, 20).unwrap();
            assert_eq!(hits.len(), 1, "{query}");
            assert_eq!(hits[0].message_id, Some(chinese.id));
        }
        assert!(search_threads(&conn, "  ", 20).unwrap().is_empty());
        assert!(search_threads(&conn, "library", 0).unwrap().is_empty());
    }

    #[test]
    fn thread_search_title_only_uses_sidebar_title_and_latest_plain_message() {
        let dir = tempfile::tempdir().unwrap();
        let conn = db::open_at(&dir.path().join("t.db")).unwrap();
        let t = threads::create_thread(&conn, Some("  Needle notes  ")).unwrap();
        let folder = crate::actions::create_thread_folder(&conn, "资料").unwrap();
        crate::actions::set_thread_folder(&conn, t.id, Some(folder.id)).unwrap();
        threads::insert_message(&conn, t.id, MessageRole::User, "旧消息", None).unwrap();
        threads::insert_message(&conn, t.id, MessageRole::Assistant, "# 最新 **回答**", None)
            .unwrap();
        let hit = search_threads(&conn, "needle", 20).unwrap().remove(0);
        assert_eq!(hit.title, thread_list(&conn).unwrap().threads[0].title);
        assert_eq!(hit.folder_id, Some(folder.id));
        assert!(hit.title_match);
        assert_eq!(hit.message_id, None);
        assert_eq!(hit.role, "");
        assert_eq!(hit.snippet, "最新 回答");
        assert_eq!(hit.created_at, t.created_at);
        crate::actions::rename_thread(&conn, t.id, "").unwrap();
        conn.execute(
            "UPDATE messages SET content='自动   标题' WHERE thread_id=?1 AND role='user'",
            [t.id],
        )
        .unwrap();
        let hit = search_threads(&conn, "自动 标题", 20).unwrap().remove(0);
        assert_eq!(hit.title, "自动 标题");
        assert!(hit.title_match);
        assert_eq!(hit.message_id, None);
    }

    #[test]
    fn thread_search_excludes_archived_anchored_and_system_messages() {
        let dir = tempfile::tempdir().unwrap();
        let conn = db::open_at(&dir.path().join("t.db")).unwrap();
        let ordinary = threads::create_thread(&conn, Some("visible")).unwrap();
        let archived = threads::create_thread(&conn, Some("needle archived")).unwrap();
        let anchored =
            threads::get_or_create_anchored_thread(&conn, "needle digest", "digest", 1).unwrap();
        for t in [&ordinary, &archived, &anchored] {
            threads::insert_message(&conn, t.id, MessageRole::User, "needle", None).unwrap();
        }
        crate::actions::archive_thread(&conn, archived.id).unwrap();
        let system = threads::create_thread(&conn, Some("system context")).unwrap();
        threads::insert_message(&conn, system.id, MessageRole::System, "needle", None).unwrap();
        for query in ["needle", "ne"] {
            let hits = search_threads(&conn, query, -1).unwrap();
            assert_eq!(
                hits.iter().map(|h| h.thread_id).collect::<Vec<_>>(),
                [ordinary.id]
            );
        }
    }

    #[test]
    fn thread_search_breaks_relevance_ties_by_updated_time() {
        let dir = tempfile::tempdir().unwrap();
        let conn = db::open_at(&dir.path().join("t.db")).unwrap();
        let a = threads::create_thread(&conn, Some("A")).unwrap().id;
        let b = threads::create_thread(&conn, Some("B")).unwrap().id;
        for id in [a, b] {
            threads::insert_message(&conn, id, MessageRole::User, "needle", None).unwrap();
        }
        conn.execute("UPDATE threads SET updated_at=10 WHERE id=?1", [a])
            .unwrap();
        conn.execute("UPDATE threads SET updated_at=20 WHERE id=?1", [b])
            .unwrap();
        let hits = search_threads(&conn, "needle", 1).unwrap();
        assert_eq!(hits[0].thread_id, b);
        assert_eq!(hits[0].updated_at, 20);
        crate::actions::rename_thread(&conn, a, "needle").unwrap();
        assert_eq!(search_threads(&conn, "needle", 1).unwrap()[0].thread_id, a);
    }

    #[test]
    fn thread_search_markdown_snippet_preserves_emoji_and_combining_graphemes() {
        let dir = tempfile::tempdir().unwrap();
        let conn = db::open_at(&dir.path().join("t.db")).unwrap();
        let t = threads::create_thread(&conn, Some("snippet")).unwrap();
        let before = "👩🏽‍💻";
        let after = "e\u{301}";
        let body = format!("# {}**needle**{}", before.repeat(80), after.repeat(80));
        threads::insert_message(&conn, t.id, MessageRole::Assistant, &body, None).unwrap();
        let hit = search_threads(&conn, "needle", 20).unwrap().remove(0);
        assert_eq!(hit.role, "assistant");
        assert_eq!(
            hit.snippet,
            format!("…{}needle{}…", before.repeat(60), after.repeat(60))
        );
        assert!(!hit.snippet.contains('�'));
    }

    #[test]
    fn ordinary_thread_list_limits_messages_and_orders_pinned_then_recent() {
        let dir = tempfile::tempdir().unwrap();
        let conn = db::open_at(&dir.path().join("t.db")).unwrap();
        conn.execute_batch("WITH RECURSIVE ids(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM ids WHERE n<1002)
            INSERT INTO threads(id,title,created_at,updated_at) SELECT n,'对话',n,n FROM ids;
            INSERT INTO messages(thread_id,role,content,created_at) SELECT id,'user','问题',id FROM threads;
            INSERT INTO threads(id,created_at,updated_at) VALUES(1003,1003,1003);
            UPDATE threads SET anchor_kind='digest',anchor_id=1 WHERE id=1002;").unwrap();
        crate::actions::set_thread_pinned(&conn, 1, true).unwrap();
        let list = thread_list(&conn).unwrap();
        assert_eq!(list.threads.len(), 1000);
        assert_eq!(list.threads[0].id, 1);
        assert!(list.threads[0].pinned);
        assert_eq!(list.threads[1].id, 1001);
        assert_eq!(list.threads[1].updated_at, 1001);
        assert_eq!(list.threads.last().unwrap().id, 3);
    }

    #[test]
    fn article_previews_keep_request_order_and_mark_missing_articles_removed() {
        let dir = tempfile::tempdir().unwrap();
        let conn = db::open_at(&dir.path().join("t.db")).unwrap();
        conn.execute("INSERT INTO feeds (id, url, title, created_at) VALUES (1, 'https://blog.example/feed', 'Rust Blog', 0)", []).unwrap();
        conn.execute(
            "INSERT INTO articles (id, feed_id, title, canonical_url, published_at, read, summary, created_at)
             VALUES (7, 1, 'Rust 1.97.1', 'https://blog.example/1971', 1700000000000, 1, '  修了三处回归。  ', 0),
                    (8, 1, '未读的一篇', NULL, NULL, 0, '', 0)",
            [],
        )
        .unwrap();
        let previews = article_previews(&conn, &[8, 404, 7, 8, 0, -3]).unwrap();
        assert_eq!(
            previews.iter().map(|p| p.article_id).collect::<Vec<_>>(),
            vec![8, 404, 7]
        );
        assert_eq!(previews[0].status, "available");
        assert!(!previews[0].read);
        assert_eq!(previews[0].summary, None);
        assert_eq!(previews[1].status, "removed");
        assert_eq!(previews[2].feed_title, "Rust Blog");
        assert_eq!(previews[2].summary.as_deref(), Some("修了三处回归。"));
        assert!(previews[2].read);
        assert_eq!(previews[2].url, "https://blog.example/1971");
        assert!(article_previews(&conn, &[]).unwrap().is_empty());
    }

    #[test]
    fn message_window_clamps_limit_and_reports_more() {
        let dir = tempfile::tempdir().unwrap();
        let conn = db::open_at(&dir.path().join("t.db")).unwrap();
        let thread = threads::create_thread(&conn, None).unwrap();
        for i in 0..(MESSAGE_PAGE_DEFAULT + 3) {
            threads::insert_message(&conn, thread.id, MessageRole::User, &format!("m{i}"), None)
                .unwrap();
        }
        let page = messages(&conn, thread.id, None, None).unwrap();
        assert_eq!(page.messages.len(), MESSAGE_PAGE_DEFAULT);
        assert!(page.has_more);
        assert_eq!(
            page.messages.last().unwrap().text,
            format!("m{}", MESSAGE_PAGE_DEFAULT + 2)
        );
        let older = messages(&conn, thread.id, Some(page.messages[0].id), Some(0)).unwrap();
        assert_eq!(older.messages.len(), 3);
        assert!(!older.has_more);
        assert_eq!(older.messages[0].text, "m0");
        let capped = messages(&conn, thread.id, None, Some(10_000)).unwrap();
        assert_eq!(capped.messages.len(), MESSAGE_PAGE_DEFAULT + 3);
        assert!(!capped.has_more);
        assert!(messages(&conn, 4040, None, None).is_err());
        assert_eq!(threads(&conn).unwrap()[0].title, "m0");
    }
}
