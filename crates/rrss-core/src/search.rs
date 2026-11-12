//! FTS 检索（trigram 分词，中文子串可走倒排）；短词、中文与未填满时 LIKE 补齐。混合检索见 `rag`。日志 target：`rrss_core::search`。

use rusqlite::Connection;
use unicode_segmentation::UnicodeSegmentation;

use crate::ai_store::truncate_chars;
use crate::articles::hide_threshold;
use crate::error::Result;
use crate::score::strip_html;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SearchScope {
    #[default]
    All,
    Unread,
    Starred,
}

#[derive(Debug, Clone)]
pub struct SearchQuery {
    pub query: String,
    pub scope: SearchScope,
    pub feed_ids: Vec<i64>,
    pub published_since: Option<i64>,
    pub limit: i64,
}

impl Default for SearchQuery {
    fn default() -> Self {
        Self {
            query: String::new(),
            scope: SearchScope::All,
            feed_ids: vec![],
            published_since: None,
            limit: 20,
        }
    }
}

pub const SNIPPET_CONTEXT_GRAPHEMES: usize = 60;
pub const SNIPPET_OPENING_GRAPHEMES: usize = 120;
const LEGACY_SNIPPET_CHARS: usize = 180;
/// FTS5 trigram 对短于 3 个字符的 MATCH 无倒排项（`AI` / `Go` / 两字中文）。
const TRIGRAM_MIN_CHARS: usize = 3;

#[derive(Debug, Clone)]
pub struct SearchHit {
    pub id: i64,
    pub title: String,
    pub feed_id: i64,
    pub feed_title: String,
    pub published_at: Option<i64>,
    pub score: i64,
    pub snippet: String,
    pub read: bool,
    pub starred: bool,
    pub title_match: bool,
    pub body_match: bool,
}

/// 无筛选调用沿用匹配 / 排序；片段与结构化面板查询一致。
pub fn search_articles(conn: &Connection, query: &str, limit: i64) -> Result<Vec<SearchHit>> {
    search_articles_with_query(
        conn,
        &SearchQuery {
            query: query.into(),
            limit,
            ..Default::default()
        },
    )
}

/// 对话 / CLI 的检索上下文保持原摘要优先片段，不受搜索面板改版影响。
pub fn search_articles_for_retrieval(
    conn: &Connection,
    query: &str,
    limit: i64,
) -> Result<Vec<SearchHit>> {
    run_search(
        conn,
        &SearchQuery {
            query: query.into(),
            limit,
            ..Default::default()
        },
        true,
    )
}

pub fn search_articles_with_query(
    conn: &Connection,
    query: &SearchQuery,
) -> Result<Vec<SearchHit>> {
    run_search(conn, query, false)
}

fn run_search(
    conn: &Connection,
    options: &SearchQuery,
    legacy_snippet: bool,
) -> Result<Vec<SearchHit>> {
    let q = normalize_search_query(&options.query);
    let limit = options.limit;
    if q.is_empty() {
        return Ok(Vec::new());
    }
    let threshold = hide_threshold(conn)?;
    // SQLite: LIMIT 0 → empty; negative LIMIT → unbounded.
    let mut hits = fts_search(conn, &q, threshold, limit, options, legacy_snippet)?;
    if needs_like_fallback(&q) {
        if let Some(rest) = remaining_limit(limit, hits.len()) {
            let extra = like_substring_search(
                conn,
                &q,
                threshold,
                rest,
                &hit_ids(&hits),
                options,
                legacy_snippet,
            )?;
            hits.extend(extra);
        }
    }
    Ok(hits)
}

/// 只剥一层首尾配对 `"`（`"AI"` → `AI`），保留内部引号；FTS 语法由 `fts_phrase` 转义。
pub fn normalize_search_query(query: &str) -> String {
    let q = query.trim();
    if q.len() >= 2 && q.starts_with('"') && q.ends_with('"') {
        q[1..q.len() - 1].trim().to_string()
    } else {
        q.to_string()
    }
}

pub fn needs_like_fallback(q: &str) -> bool {
    contains_cjk(q)
        || query_char_len(q) < TRIGRAM_MIN_CHARS
        || q.chars().any(|c| c.is_ascii_punctuation())
}

fn query_char_len(q: &str) -> usize {
    q.chars().count()
}

/// `None` = already filled to a non-negative limit (including `limit == 0`).
/// `Some(n)` = fetch up to `n` more rows; `n < 0` means unbounded.
fn remaining_limit(limit: i64, used: usize) -> Option<i64> {
    if limit == 0 {
        return None;
    }
    if limit < 0 {
        return Some(-1);
    }
    let rest = limit - used as i64;
    if rest <= 0 { None } else { Some(rest) }
}

fn hit_ids(hits: &[SearchHit]) -> Vec<i64> {
    hits.iter().map(|h| h.id).collect()
}

fn append_filters(
    sql: &mut String,
    values: &mut Vec<rusqlite::types::Value>,
    options: &SearchQuery,
) {
    match options.scope {
        SearchScope::All => {}
        SearchScope::Unread => sql.push_str(" AND a.read = 0"),
        SearchScope::Starred => sql.push_str(" AND a.starred = 1"),
    }
    if !options.feed_ids.is_empty() {
        sql.push_str(" AND a.feed_id IN (");
        sql.push_str(&vec!["?"; options.feed_ids.len()].join(", "));
        sql.push(')');
        values.extend(options.feed_ids.iter().copied().map(Into::into));
    }
    if let Some(since) = options.published_since {
        sql.push_str(" AND a.published_at >= ?");
        values.push(since.into());
    }
}

fn fts_search(
    conn: &Connection,
    query: &str,
    threshold: i64,
    limit: i64,
    options: &SearchQuery,
    legacy_snippet: bool,
) -> Result<Vec<SearchHit>> {
    let Some(match_query) = fts_phrase(query) else {
        return Ok(Vec::new());
    };
    let mut sql = String::from(
        "SELECT a.id, a.title, COALESCE(f.title, f.url), a.published_at, a.score,
                a.summary, a.fulltext_md, a.content_md, a.content_html, a.read, a.starred, a.feed_id
         FROM articles_fts
         JOIN articles a ON a.id = articles_fts.rowid
         JOIN feeds f ON f.id = a.feed_id
         WHERE a.expires_at IS NULL AND articles_fts MATCH ? AND a.score > ?",
    );
    let mut values = vec![match_query.into(), threshold.into()];
    append_filters(&mut sql, &mut values, options);
    sql.push_str(" ORDER BY bm25(articles_fts) LIMIT ?");
    values.push(limit.into());
    let mut stmt = conn.prepare(&sql)?;
    collect_search_hits(stmt.query_map(rusqlite::params_from_iter(values), |row| {
        map_search_row(row, query, legacy_snippet)
    })?)
}

fn like_substring_search(
    conn: &Connection,
    query: &str,
    threshold: i64,
    limit: i64,
    exclude: &[i64],
    options: &SearchQuery,
    legacy_snippet: bool,
) -> Result<Vec<SearchHit>> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    let pattern = format!("%{}%", like_escape(query));
    let not_in = if exclude.is_empty() {
        String::new()
    } else {
        let placeholders = vec!["?"; exclude.len()].join(", ");
        format!(" AND a.id NOT IN ({placeholders})")
    };
    let mut sql = format!(
        "SELECT a.id, a.title, COALESCE(f.title, f.url), a.published_at, a.score,
                a.summary, a.fulltext_md, a.content_md, a.content_html, a.read, a.starred, a.feed_id
         FROM articles a
         JOIN feeds f ON f.id = a.feed_id
         WHERE a.expires_at IS NULL AND a.score > ?1
           AND (
             a.title LIKE ?2 ESCAPE '\\'
             OR COALESCE(a.author, '') LIKE ?2 ESCAPE '\\'
             OR COALESCE(a.summary, '') LIKE ?2 ESCAPE '\\'
             OR COALESCE(a.fulltext_md, a.content_md, a.content_html, '') LIKE ?2 ESCAPE '\\'
           )
           {not_in}"
    );
    let mut vals: Vec<rusqlite::types::Value> = vec![threshold.into(), pattern.into()];
    vals.extend(exclude.iter().copied().map(Into::into));
    append_filters(&mut sql, &mut vals, options);
    sql.push_str(" ORDER BY a.published_at DESC, a.id DESC LIMIT ?");
    vals.push(limit.into());
    let mut stmt = conn.prepare(&sql)?;
    collect_search_hits(stmt.query_map(rusqlite::params_from_iter(vals), |row| {
        map_search_row(row, query, legacy_snippet)
    })?)
}

fn map_search_row(
    row: &rusqlite::Row<'_>,
    query: &str,
    legacy_snippet: bool,
) -> rusqlite::Result<SearchHit> {
    let summary: Option<String> = row.get(5)?;
    let fulltext: Option<String> = row.get(6)?;
    let content_md: Option<String> = row.get(7)?;
    let content_html: Option<String> = row.get(8)?;
    // 同 FTS body 列的 COALESCE 优先级；只在选出的行上去标签 / 找命中。
    let body = strip_html(
        fulltext
            .as_deref()
            .or(content_md.as_deref())
            .or(content_html.as_deref())
            .unwrap_or(""),
    );
    let title: String = row.get(1)?;
    Ok(SearchHit {
        id: row.get(0)?,
        title_match: contains_query(&title, query),
        body_match: contains_query(&body, query),
        title,
        feed_id: row.get(11)?,
        feed_title: row.get(2)?,
        published_at: row.get(3)?,
        score: row.get(4)?,
        read: row.get(9)?,
        starred: row.get(10)?,
        snippet: if legacy_snippet {
            snippet(
                summary.as_deref(),
                fulltext.as_deref(),
                content_md.as_deref(),
                content_html.as_deref(),
            )
        } else {
            hit_snippet(&body, query)
        },
    })
}

fn collect_search_hits(
    rows: impl Iterator<Item = rusqlite::Result<SearchHit>>,
) -> Result<Vec<SearchHit>> {
    let mut hits: Vec<SearchHit> = rows.collect::<rusqlite::Result<_>>()?;
    for hit in &mut hits {
        hit.feed_title = crate::feeds::display_title(&hit.feed_title, "");
    }
    Ok(hits)
}

fn contains_cjk(s: &str) -> bool {
    s.chars().any(|c| {
        matches!(
            c,
            '\u{4e00}'..='\u{9fff}' | '\u{3400}'..='\u{4dbf}' | '\u{f900}'..='\u{faff}'
        )
    })
}

pub fn like_escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

fn snippet(
    summary: Option<&str>,
    fulltext: Option<&str>,
    content_md: Option<&str>,
    content_html: Option<&str>,
) -> String {
    if let Some(s) = summary.map(str::trim).filter(|s| !s.is_empty()) {
        return truncate_chars(s, LEGACY_SNIPPET_CHARS);
    }
    let raw = fulltext.or(content_md).or(content_html).unwrap_or("");
    truncate_chars(&strip_html(raw), LEGACY_SNIPPET_CHARS)
}

/// ASCII 折叠不改变 UTF-8 字节偏移；其他文字原样匹配。空查询不命中。
pub fn query_match_offset(text: &str, query: &str) -> Option<usize> {
    if query.is_empty() {
        return None;
    }
    text.to_ascii_lowercase().find(&query.to_ascii_lowercase())
}

pub fn contains_query(text: &str, query: &str) -> bool {
    query_match_offset(text, query).is_some()
}

pub fn truncate_search_text(text: &str, max: usize) -> String {
    let end = text.grapheme_indices(true).nth(max).map(|(i, _)| i);
    match end {
        Some(end) => format!("{}…", &text[..end]),
        None => text.to_owned(),
    }
}

/// 整段不超 `max` 个字素簇时原样返回；长段以第一处命中为中心截取，保证命中落在返回文本里。
pub fn hit_excerpt(text: &str, query: &str, max: usize) -> String {
    if text.graphemes(true).nth(max).is_none() {
        return text.to_owned();
    }
    match_window(text, query, max / 2).unwrap_or_else(|| truncate_search_text(text, max))
}

pub fn hit_snippet(body: &str, query: &str) -> String {
    match_window(body, query, SNIPPET_CONTEXT_GRAPHEMES)
        .unwrap_or_else(|| truncate_search_text(body, SNIPPET_OPENING_GRAPHEMES))
}

/// 第一处命中连同前后各 `context` 个字素簇；被截断的一侧加 `…`。无命中返回 `None`。
pub fn match_window(body: &str, query: &str, context: usize) -> Option<String> {
    let offset = query_match_offset(body, query)?;
    // 扩展字素簇保证组合字符、肤色、ZWJ emoji 不被截开。
    let mut boundaries: Vec<usize> = body.grapheme_indices(true).map(|(i, _)| i).collect();
    boundaries.push(body.len());
    let count = boundaries.len() - 1;
    let first = boundaries
        .partition_point(|&i| i <= offset)
        .saturating_sub(1);
    let after = boundaries.partition_point(|&i| i < offset + query.len());
    let start = first.saturating_sub(context);
    let end = (after + context).min(count);
    Some(format!(
        "{}{}{}",
        if start > 0 { "…" } else { "" },
        &body[boundaries[start]..boundaries[end]],
        if end < count { "…" } else { "" }
    ))
}

pub fn fts_phrase(query: &str) -> Option<String> {
    let q = query.trim();
    if q.is_empty() || query_char_len(q) < TRIGRAM_MIN_CHARS {
        return None;
    }
    // FTS5 短语内的 `"` 写成 `""`，整段再包一层引号，避免 MATCH 语法错误。
    let escaped = q.replace('"', "\"\"");
    Some(format!("\"{escaped}\""))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::articles::DEFAULT_HIDE_THRESHOLD;
    use crate::db::{PooledConnection, open_at};
    use crate::feeds::{SubscribeRequest, subscribe};
    use crate::parse::ParsedEntry;
    use crate::rules::{Dimension, NewRule, Polarity, insert_rule};
    use tempfile::tempdir;

    fn open_temp() -> (tempfile::TempDir, PooledConnection) {
        let dir = tempdir().unwrap();
        let conn = open_at(&dir.path().join("rrss.db")).unwrap();
        (dir, conn)
    }

    fn seed(conn: &Connection) {
        let feed = subscribe(
            conn,
            SubscribeRequest {
                url: "https://news.example.com/rss.xml".into(),
                folder_id: None,
                as_rsshub: false,
                rsshub_instance_id: None,
            },
        )
        .unwrap();
        insert_rule(
            conn,
            NewRule::manual_global(Polarity::Dislike, Dimension::TitleKw, "hidden"),
        )
        .unwrap();
        let entries = [
            ParsedEntry {
                guid: "keep".into(),
                canonical_url: Some("https://news.example.com/library".into()),
                title: "City opens new library".into(),
                author: Some("Ada".into()),
                published_at: Some(1),
                content_html: Some(
                    "<p>The new library opened downtown with long hours.</p>".into(),
                ),
                hash: "h1".into(),
                tags: vec![],
            },
            ParsedEntry {
                guid: "hide".into(),
                canonical_url: Some("https://news.example.com/hidden".into()),
                title: "hidden library gossip".into(),
                author: None,
                published_at: Some(2),
                content_html: Some("<p>should not appear</p>".into()),
                hash: "h2".into(),
                tags: vec![],
            },
        ];
        crate::ingest::upsert_entries(conn, feed, &entries, 1).unwrap();
    }

    #[test]
    fn structured_filters_apply_to_fts_and_like_without_changing_default_order() {
        let (_dir, conn) = open_temp();
        seed_cjk_mix(&conn);
        let first_feed: i64 = conn
            .query_row("SELECT id FROM feeds LIMIT 1", [], |r| r.get(0))
            .unwrap();
        let other = subscribe(
            &conn,
            SubscribeRequest {
                url: "https://other.test/rss".into(),
                folder_id: None,
                as_rsshub: false,
                rsshub_instance_id: None,
            },
        )
        .unwrap();
        crate::ingest::upsert_entries(
            &conn,
            other,
            &[ParsedEntry {
                guid: "other".into(),
                title: "图书馆 library AI other".into(),
                author: None,
                canonical_url: None,
                published_at: Some(5),
                content_html: Some("<p>library AI</p>".into()),
                hash: "other".into(),
                tags: vec![],
            }],
            1,
        )
        .unwrap();
        conn.execute(
            "UPDATE articles SET title = title || ' library AI' WHERE feed_id = ?1",
            [first_feed],
        )
        .unwrap();
        conn.execute(
            "UPDATE articles SET read = 1, starred = 1 WHERE guid = 'exact'",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE articles SET score = ?1 WHERE guid = 'hide'",
            [DEFAULT_HIDE_THRESHOLD],
        )
        .unwrap();
        for word in ["library", "AI"] {
            let options = SearchQuery {
                query: word.into(),
                limit: 10,
                ..Default::default()
            };
            let ids = |rows: Vec<SearchHit>| rows.into_iter().map(|h| h.id).collect::<Vec<_>>();
            assert_eq!(
                ids(search_articles_with_query(&conn, &options).unwrap()),
                ids(search_articles(&conn, word, 10).unwrap())
            );
            let all = search_articles_with_query(&conn, &options).unwrap();
            assert_eq!(all.len(), 3);
            assert!(all.iter().all(|h| !h.title.contains("hidden")));
            let unread = search_articles_with_query(
                &conn,
                &SearchQuery {
                    scope: SearchScope::Unread,
                    ..options.clone()
                },
            )
            .unwrap();
            assert_eq!(unread.len(), 2);
            assert!(unread.iter().all(|h| !h.read));
            let starred = search_articles_with_query(
                &conn,
                &SearchQuery {
                    scope: SearchScope::Starred,
                    ..options.clone()
                },
            )
            .unwrap();
            assert_eq!(starred.len(), 1);
            assert!(starred[0].starred && starred[0].read);
            let feed = search_articles_with_query(
                &conn,
                &SearchQuery {
                    feed_ids: vec![first_feed],
                    ..options.clone()
                },
            )
            .unwrap();
            assert_eq!(feed.len(), 2);
            let time = search_articles_with_query(
                &conn,
                &SearchQuery {
                    published_since: Some(3),
                    ..options.clone()
                },
            )
            .unwrap();
            assert_eq!(time.len(), 2);
            assert!(time.iter().all(|h| h.published_at.unwrap() >= 3));
            let combined = search_articles_with_query(
                &conn,
                &SearchQuery {
                    feed_ids: vec![first_feed, other],
                    published_since: Some(3),
                    scope: SearchScope::Unread,
                    ..options
                },
            )
            .unwrap();
            assert_eq!(combined.len(), 1);
            assert!(combined[0].title.contains("other"));
        }
        // 明确验证中文补齐在 FTS 已返回一部分时仍应用全部筛选。
        let options = SearchQuery {
            query: "图书馆".into(),
            scope: SearchScope::Unread,
            feed_ids: vec![first_feed],
            published_since: Some(2),
            limit: 10,
        };
        assert_eq!(
            search_articles_with_query(&conn, &options).unwrap().len(),
            1
        );
        let options = SearchQuery {
            scope: SearchScope::Starred,
            ..options
        };
        assert_eq!(
            search_articles_with_query(&conn, &options).unwrap().len(),
            1
        );
    }

    #[test]
    fn hit_snippets_preserve_graphemes_and_center_first_literal_match() {
        for unit in ["字", "a", "😀", "👩🏽‍💻", "e\u{301}"] {
            let pad = unit.repeat(80);
            assert_eq!(
                hit_snippet(&format!("KEY{pad}"), "key"),
                format!("KEY{}…", unit.repeat(60))
            );
            assert_eq!(
                hit_snippet(&format!("{pad}KEY{pad}"), "key"),
                format!("…{}KEY{}…", unit.repeat(60), unit.repeat(60))
            );
            assert_eq!(
                hit_snippet(&format!("{pad}KEY"), "key"),
                format!("…{}KEY", unit.repeat(60))
            );
        }
        assert_eq!(hit_snippet("中文图书馆正文", "图书馆"), "中文图书馆正文");
        assert_eq!(
            hit_snippet("first KEY then KEY", "key"),
            "first KEY then KEY"
        );
        assert!(!contains_query("École", "école"));
        assert!(!contains_query("body", ""));
        assert_eq!(
            hit_snippet(&"👩🏽‍💻".repeat(121), "absent"),
            format!("{}…", "👩🏽‍💻".repeat(120))
        );
        // 命中段：短段原样，长段以命中为中心，命中在段尾也不被截掉。
        assert_eq!(hit_excerpt("短段 KEY", "key", 400), "短段 KEY");
        let long = format!("{}KEY{}", "字".repeat(500), "尾".repeat(500));
        assert_eq!(
            hit_excerpt(&long, "key", 400),
            format!("…{}KEY{}…", "字".repeat(200), "尾".repeat(200))
        );
        assert_eq!(
            hit_excerpt(&"字".repeat(500), "key", 400),
            format!("{}…", "字".repeat(400))
        );
        // 命中本身落在字素簇内部时，也必须包含完整字素簇。
        assert_eq!(
            hit_snippet(
                &format!("{}e\u{301}{}", "中".repeat(80), "尾".repeat(80)),
                "\u{301}"
            ),
            format!("…{}e\u{301}{}…", "中".repeat(60), "尾".repeat(60))
        );
    }

    #[test]
    fn structured_search_uses_body_not_summary_and_reports_match_location() {
        let (_dir, conn) = open_temp();
        seed(&conn);
        conn.execute("UPDATE articles SET summary = 'AI summary library', fulltext_md = ?1, content_md = 'wrong body' WHERE guid = 'keep'", [format!("{}LiBrArY{}", "中".repeat(80), "😀".repeat(80))]).unwrap();
        let hits = search_articles_with_query(
            &conn,
            &SearchQuery {
                query: "library".into(),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(hits[0].title_match && hits[0].body_match);
        assert_eq!(
            hits[0].snippet,
            format!("…{}LiBrArY{}…", "中".repeat(60), "😀".repeat(60))
        );
        conn.execute("UPDATE articles SET fulltext_md = NULL, content_md = NULL, content_html = '<p>Body beginning.</p>' WHERE guid = 'keep'", []).unwrap();
        let hits = search_articles_with_query(
            &conn,
            &SearchQuery {
                query: "library".into(),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(hits[0].title_match && !hits[0].body_match);
        assert_eq!(hits[0].snippet, "Body beginning.");
        let summary_only = search_articles_with_query(
            &conn,
            &SearchQuery {
                query: "AI".into(),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            summary_only.len(),
            1,
            "legacy LIKE candidate semantics remain unchanged"
        );
        assert!(!summary_only[0].title_match && !summary_only[0].body_match);
        assert_eq!(summary_only[0].snippet, "Body beginning.");
        conn.execute(
            "UPDATE articles SET author = 'Only Author', summary = NULL WHERE guid = 'keep'",
            [],
        )
        .unwrap();
        let author = search_articles_with_query(
            &conn,
            &SearchQuery {
                query: "Only Author".into(),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(author.len(), 1);
        assert!(!author[0].title_match && !author[0].body_match);
        assert_eq!(author[0].snippet, "Body beginning.");
    }

    #[test]
    fn fts_finds_visible_and_skips_hidden() {
        let (_dir, conn) = open_temp();
        seed(&conn);
        let hits = search_articles(&conn, "library", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "City opens new library");
        assert!(hits[0].snippet.contains("library"));
        assert!(hits[0].score > DEFAULT_HIDE_THRESHOLD);
        let quoted = search_articles(&conn, "\"library\"", 10).unwrap();
        assert_eq!(quoted.len(), 1);
        assert_eq!(quoted[0].title, "City opens new library");
        assert!(search_articles(&conn, "", 10).unwrap().is_empty());
    }

    #[test]
    fn cjk_keyword_matches_title_substring() {
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
        let entries = [ParsedEntry {
            guid: "lib-cn".into(),
            canonical_url: Some("https://news.example.com/lib".into()),
            title: "城市图书馆延长开放时间".into(),
            author: None,
            published_at: Some(1),
            content_html: Some("<p>公告</p>".into()),
            hash: "hcn".into(),
            tags: vec![],
        }];
        crate::ingest::upsert_entries(&conn, feed, &entries, 1).unwrap();
        assert!(
            search_articles(&conn, "图书馆", 10)
                .unwrap()
                .iter()
                .any(|h| h.title.contains("图书馆")),
            "CJK keyword should match inside title"
        );
        assert_eq!(search_articles(&conn, "library", 10).unwrap().len(), 0);
        let two = search_articles(&conn, "图书", 10).unwrap();
        assert!(
            two.iter().any(|h| h.title.contains("图书馆")),
            "2-char CJK still matches via LIKE fallback"
        );
    }

    fn seed_cjk_mix(conn: &Connection) {
        let feed = subscribe(
            conn,
            SubscribeRequest {
                url: "https://news.example.com/rss.xml".into(),
                folder_id: None,
                as_rsshub: false,
                rsshub_instance_id: None,
            },
        )
        .unwrap();
        insert_rule(
            conn,
            NewRule::manual_global(Polarity::Dislike, Dimension::TitleKw, "hidden"),
        )
        .unwrap();
        crate::ingest::upsert_entries(
            conn,
            feed,
            &[
                ParsedEntry {
                    guid: "exact".into(),
                    canonical_url: Some("https://news.example.com/lib-exact".into()),
                    title: "图书馆".into(),
                    author: None,
                    published_at: Some(3),
                    content_html: Some("<p>独立词</p>".into()),
                    hash: "hexact".into(),
                    tags: vec![],
                },
                ParsedEntry {
                    guid: "sub".into(),
                    canonical_url: Some("https://news.example.com/lib-sub".into()),
                    title: "城市图书馆延长开放时间".into(),
                    author: None,
                    published_at: Some(2),
                    content_html: Some("<p>公告</p>".into()),
                    hash: "hsub".into(),
                    tags: vec![],
                },
                ParsedEntry {
                    guid: "hide".into(),
                    canonical_url: Some("https://news.example.com/lib-hide".into()),
                    title: "hidden 图书馆内参".into(),
                    author: None,
                    published_at: Some(4),
                    content_html: Some("<p>不应出现</p>".into()),
                    hash: "hhide".into(),
                    tags: vec![],
                },
            ],
            1,
        )
        .unwrap();
    }

    #[test]
    fn cjk_merges_fts_and_substring_skips_hidden() {
        let (_dir, conn) = open_temp();
        seed_cjk_mix(&conn);
        let hits = search_articles(&conn, "图书馆", 10).unwrap();
        let titles: Vec<_> = hits.iter().map(|h| h.title.as_str()).collect();
        assert_eq!(titles, vec!["图书馆", "城市图书馆延长开放时间"]);
        assert!(!titles.iter().any(|t| t.contains("hidden")));
    }

    #[test]
    fn cjk_respects_limit_zero_and_negative() {
        let (_dir, conn) = open_temp();
        seed_cjk_mix(&conn);
        assert!(search_articles(&conn, "图书馆", 0).unwrap().is_empty());
        let one = search_articles(&conn, "图书馆", 1).unwrap();
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].title, "图书馆");
        let two = search_articles(&conn, "图书馆", 2).unwrap();
        assert_eq!(two.len(), 2);
        let unbounded = search_articles(&conn, "图书馆", -1).unwrap();
        assert_eq!(unbounded.len(), 2);
    }

    #[test]
    fn cjk_like_escapes_percent_and_underscore() {
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
        crate::ingest::upsert_entries(
            &conn,
            feed,
            &[
                ParsedEntry {
                    guid: "pct".into(),
                    canonical_url: Some("https://news.example.com/pct".into()),
                    title: "图书馆%延长".into(),
                    author: None,
                    published_at: Some(1),
                    content_html: Some("<p>x</p>".into()),
                    hash: "hpct".into(),
                    tags: vec![],
                },
                ParsedEntry {
                    guid: "us".into(),
                    canonical_url: Some("https://news.example.com/us".into()),
                    title: "图_馆通知".into(),
                    author: None,
                    published_at: Some(2),
                    content_html: Some("<p>y</p>".into()),
                    hash: "hus".into(),
                    tags: vec![],
                },
                ParsedEntry {
                    guid: "plain".into(),
                    canonical_url: Some("https://news.example.com/plain".into()),
                    title: "城市图书馆延长开放时间".into(),
                    author: None,
                    published_at: Some(3),
                    content_html: Some("<p>z</p>".into()),
                    hash: "hplain".into(),
                    tags: vec![],
                },
            ],
            1,
        )
        .unwrap();
        let pct = search_articles(&conn, "图书馆%", 10).unwrap();
        assert_eq!(pct.len(), 1);
        assert_eq!(pct[0].title, "图书馆%延长");
        let us = search_articles(&conn, "图_馆", 10).unwrap();
        assert_eq!(us.len(), 1);
        assert_eq!(us[0].title, "图_馆通知");
        let plain = search_articles(&conn, "图书馆", 10).unwrap();
        assert!(plain.iter().any(|h| h.title == "城市图书馆延长开放时间"));
        assert!(plain.iter().any(|h| h.title == "图书馆%延长"));
        assert!(!plain.iter().any(|h| h.title == "图_馆通知"));
    }

    #[test]
    fn snippet_ignores_summary_but_retrieval_retains_it() {
        let (_dir, conn) = open_temp();
        seed(&conn);
        let id: i64 = conn
            .query_row(
                "SELECT id FROM articles WHERE title = 'City opens new library'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        crate::articles::set_summary(&conn, id, "Short canned summary about the branch.").unwrap();
        let hits = search_articles(&conn, "library", 10).unwrap();
        assert_eq!(
            hits[0].snippet,
            "The new library opened downtown with long hours."
        );
        let retrieval = search_articles_for_retrieval(&conn, "library", 10).unwrap();
        assert_eq!(
            retrieval[0].snippet,
            "Short canned summary about the branch."
        );
    }

    fn seed_compat(conn: &Connection) {
        let feed = subscribe(
            conn,
            SubscribeRequest {
                url: "https://news.example.com/rss.xml".into(),
                folder_id: None,
                as_rsshub: false,
                rsshub_instance_id: None,
            },
        )
        .unwrap();
        insert_rule(
            conn,
            NewRule::manual_global(Polarity::Dislike, Dimension::TitleKw, "hidden"),
        )
        .unwrap();
        crate::ingest::upsert_entries(
            conn,
            feed,
            &[
                ParsedEntry {
                    guid: "ai".into(),
                    canonical_url: Some("https://news.example.com/ai".into()),
                    title: "Why AI changes everything".into(),
                    author: None,
                    published_at: Some(1),
                    content_html: Some("<p>Short token in title.</p>".into()),
                    hash: "hai".into(),
                    tags: vec![],
                },
                ParsedEntry {
                    guid: "go".into(),
                    canonical_url: Some("https://news.example.com/go".into()),
                    title: "Go 1.22 released".into(),
                    author: None,
                    published_at: Some(2),
                    content_html: Some("<p>Language notes.</p>".into()),
                    hash: "hgo".into(),
                    tags: vec![],
                },
                ParsedEntry {
                    guid: "cpp".into(),
                    canonical_url: Some("https://news.example.com/cpp".into()),
                    title: "Notes on C++ templates".into(),
                    author: None,
                    published_at: Some(3),
                    content_html: Some("<p>Punctuation in title.</p>".into()),
                    hash: "hcpp".into(),
                    tags: vec![],
                },
                ParsedEntry {
                    guid: "rare".into(),
                    canonical_url: Some("https://news.example.com/rare".into()),
                    title: "Only zyxxyrareword appears here".into(),
                    author: None,
                    published_at: Some(4),
                    content_html: Some("<p>unique marker</p>".into()),
                    hash: "hrare".into(),
                    tags: vec![],
                },
                ParsedEntry {
                    guid: "hide-ai".into(),
                    canonical_url: Some("https://news.example.com/hide-ai".into()),
                    title: "hidden AI gossip".into(),
                    author: None,
                    published_at: Some(5),
                    content_html: Some("<p>should not appear</p>".into()),
                    hash: "hhideai".into(),
                    tags: vec![],
                },
                ParsedEntry {
                    guid: "sum-cn".into(),
                    canonical_url: Some("https://news.example.com/sum-cn".into()),
                    title: "Weather brief".into(),
                    author: None,
                    published_at: Some(6),
                    content_html: Some("<p>no keyword in body</p>".into()),
                    hash: "hsumcn".into(),
                    tags: vec![],
                },
            ],
            1,
        )
        .unwrap();
        let summary_id: i64 = conn
            .query_row(
                "SELECT id FROM articles WHERE title = 'Weather brief'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        crate::articles::set_summary(conn, summary_id, "今日量子计算进展综述").unwrap();
    }

    #[test]
    fn fts_phrase_skips_short_terms_and_escapes_quotes() {
        assert_eq!(fts_phrase(""), None);
        assert_eq!(fts_phrase("AI"), None);
        assert_eq!(fts_phrase("Go"), None);
        assert_eq!(fts_phrase("图书馆"), Some("\"图书馆\"".into()));
        assert_eq!(fts_phrase("library"), Some("\"library\"".into()));
        assert_eq!(
            fts_phrase("say \"hi\" there"),
            Some("\"say \"\"hi\"\" there\"".into())
        );
    }

    #[test]
    fn normalize_search_query_strips_only_wrapping_quotes() {
        assert_eq!(normalize_search_query("AI"), "AI");
        assert_eq!(normalize_search_query("  \"AI\"  "), "AI");
        assert_eq!(normalize_search_query("\"library\""), "library");
        assert_eq!(
            normalize_search_query("say \"hi\" there"),
            "say \"hi\" there"
        );
        assert_eq!(
            normalize_search_query("\"say \"hi\" there\""),
            "say \"hi\" there"
        );
        assert_eq!(normalize_search_query("\"AI"), "\"AI");
        assert_eq!(normalize_search_query("AI\""), "AI\"");
        assert_eq!(normalize_search_query("\"\""), "");
        assert_eq!(normalize_search_query("  "), "");
    }

    #[test]
    fn short_english_tokens_match_ai_and_go() {
        let (_dir, conn) = open_temp();
        seed_compat(&conn);
        let ai = search_articles(&conn, "AI", 10).unwrap();
        assert_eq!(
            ai.iter().map(|h| h.title.as_str()).collect::<Vec<_>>(),
            vec!["Why AI changes everything"]
        );
        let quoted = search_articles(&conn, "\"AI\"", 10).unwrap();
        assert_eq!(quoted.len(), 1);
        assert_eq!(quoted[0].title, "Why AI changes everything");
        let go = search_articles(&conn, "Go", 10).unwrap();
        assert_eq!(go.len(), 1);
        assert_eq!(go[0].title, "Go 1.22 released");
        assert!(search_articles(&conn, "AI", 0).unwrap().is_empty());
    }

    #[test]
    fn zero_hits_for_unknown_term() {
        let (_dir, conn) = open_temp();
        seed_compat(&conn);
        assert!(
            search_articles(&conn, "zzzxqqqnotfound", 10)
                .unwrap()
                .is_empty()
        );
        assert!(search_articles(&conn, "\"\"", 10).unwrap().is_empty());
    }

    #[test]
    fn rare_word_finds_single_article() {
        let (_dir, conn) = open_temp();
        seed_compat(&conn);
        let hits = search_articles(&conn, "zyxxyrareword", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "Only zyxxyrareword appears here");
    }

    #[test]
    fn punctuation_query_matches_c_plus_plus() {
        let (_dir, conn) = open_temp();
        seed_compat(&conn);
        let hits = search_articles(&conn, "C++", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "Notes on C++ templates");
    }

    #[test]
    fn internal_quotes_match_literal_and_wrapping_still_finds() {
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
        crate::ingest::upsert_entries(
            &conn,
            feed,
            &[
                ParsedEntry {
                    guid: "quoted".into(),
                    canonical_url: Some("https://news.example.com/quoted".into()),
                    title: "He said \"hello\" today".into(),
                    author: None,
                    published_at: Some(1),
                    content_html: Some("<p>with quotes</p>".into()),
                    hash: "hquoted".into(),
                    tags: vec![],
                },
                ParsedEntry {
                    guid: "plain".into(),
                    canonical_url: Some("https://news.example.com/plain-hello".into()),
                    title: "He said hello today".into(),
                    author: None,
                    published_at: Some(2),
                    content_html: Some("<p>no quotes</p>".into()),
                    hash: "hplainhello".into(),
                    tags: vec![],
                },
            ],
            1,
        )
        .unwrap();
        let literal = search_articles(&conn, "said \"hello\"", 10).unwrap();
        assert_eq!(
            literal.iter().map(|h| h.title.as_str()).collect::<Vec<_>>(),
            vec!["He said \"hello\" today"]
        );
        let wrapping = search_articles(&conn, "\"hello\"", 10).unwrap();
        let titles: Vec<_> = wrapping.iter().map(|h| h.title.as_str()).collect();
        assert!(titles.contains(&"He said \"hello\" today"));
        assert!(titles.contains(&"He said hello today"));
        let wrapped_literal = search_articles(&conn, "\"said \"hello\"\"", 10).unwrap();
        assert_eq!(
            wrapped_literal
                .iter()
                .map(|h| h.title.as_str())
                .collect::<Vec<_>>(),
            vec!["He said \"hello\" today"]
        );
    }

    #[test]
    fn cjk_summary_only_still_matches() {
        let (_dir, conn) = open_temp();
        seed_compat(&conn);
        let hits = search_articles(&conn, "量子计算", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "Weather brief");
        assert_eq!(hits[0].snippet, "no keyword in body");
        assert!(!hits[0].title_match && !hits[0].body_match);
    }
}
