use rusqlite::Connection;

use rrss_core::articles::{self, ListScope};
use rrss_core::feed_scope::{self, FeedScope};
use rrss_core::timeline::{self, TimelineCursor};

use crate::views::*;

pub fn feeds_page(
    conn: &Connection,
    scope: &str,
    list_scope: &str,
    cursor: Option<FeedsCursor>,
) -> rrss_core::Result<FeedsPage> {
    let resolved = feed_scope::resolve(conn, FeedScope::parse_str(scope))?;
    let list_scope = ListScope::parse_str(list_scope);
    let (cards, next) = timeline::list_feeds_page(
        conn,
        resolved,
        list_scope,
        cursor.map(|cursor| TimelineCursor {
            at: cursor.at,
            id: cursor.id,
        }),
        articles::FOCUS_PAGE_SIZE,
    )?;
    let icons = super::feeds::feed_icon_hrefs(conn)?;
    let articles = cards
        .into_iter()
        .map(|card| {
            let mut article = article_from_card(card);
            article.feed_icon_href = icons.get(&article.feed_id).cloned().unwrap_or_default();
            article
        })
        .collect();
    Ok(FeedsPage {
        articles,
        has_more: next.is_some(),
        next_cursor: next.map(|cursor| FeedsCursor {
            at: cursor.at,
            id: cursor.id,
        }),
        hidden_count: rrss_core::hidden::hidden_count(conn, resolved, list_scope)?,
        scope: resolved.as_string(),
    })
}

pub fn feed_counts(conn: &Connection) -> rrss_core::Result<FeedCountsView> {
    let counts = timeline::feed_counts(conn)?;
    let scope_counts = |rows: std::collections::BTreeMap<i64, i64>| {
        rows.into_iter()
            .map(|(id, unread)| ScopeCount { id, unread })
            .collect()
    };
    Ok(FeedCountsView {
        total: counts.total,
        feeds: scope_counts(counts.feeds),
        folders: scope_counts(counts.folders),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actions;
    use rrss_core::feeds;

    fn open() -> (tempfile::TempDir, rrss_core::db::PooledConnection) {
        let dir = tempfile::tempdir().unwrap();
        let conn = rrss_core::db::open_at(&dir.path().join("feeds.db")).unwrap();
        articles::set_hide_threshold(&conn, -10).unwrap();
        (dir, conn)
    }

    fn ids(page: &FeedsPage) -> Vec<i64> {
        page.articles.iter().map(|article| article.id).collect()
    }

    #[test]
    fn deleted_folder_scope_returns_all_and_lists_visible_articles() {
        let (_dir, conn) = open();
        conn.execute_batch(
            "INSERT INTO folders (id, name, created_at) VALUES (7, 'gone', 0);
             INSERT INTO feeds (id, url, folder_id, created_at) VALUES
               (1, 'https://in.example/rss', 7, 0),
               (2, 'https://out.example/rss', NULL, 0);
             INSERT INTO articles (id, feed_id, title, published_at, created_at, score) VALUES
               (1, 1, 'in', 30, 30, 0),
               (2, 2, 'out', 20, 20, 0),
               (3, 2, 'hidden', 10, 10, -11);",
        )
        .unwrap();
        feeds::delete_folder(&conn, 7).unwrap();
        let page = feeds_page(&conn, "folder:7", "all", None).unwrap();
        assert_eq!(page.scope, "all");
        assert_eq!(ids(&page), vec![1, 2]);
    }

    #[test]
    fn next_cursor_pages_do_not_overlap_or_skip() {
        let (_dir, conn) = open();
        conn.execute(
            "INSERT INTO feeds (id, url, created_at) VALUES (1, 'https://a.example/rss', 0)",
            [],
        )
        .unwrap();
        let n = articles::FOCUS_PAGE_SIZE;
        conn.execute(
            "WITH RECURSIVE ids(id) AS (SELECT 1 UNION ALL SELECT id + 1 FROM ids WHERE id < ?1)
             INSERT INTO articles (id, feed_id, title, published_at, created_at)
             SELECT id, 1, id, id, id FROM ids",
            [n + 1],
        )
        .unwrap();
        let first = feeds_page(&conn, "all", "all", None).unwrap();
        assert!(first.has_more);
        assert_eq!(first.articles.len() as i64, n);
        let second = feeds_page(&conn, "all", "all", first.next_cursor).unwrap();
        assert!(!second.has_more);
        assert!(second.next_cursor.is_none());
        let mut seen = ids(&first);
        seen.extend(ids(&second));
        seen.sort_unstable();
        assert_eq!(seen, (1..=n + 1).collect::<Vec<_>>());
    }

    #[test]
    fn hidden_count_matches_hidden_page_for_the_same_scope() {
        let (_dir, conn) = open();
        let now = feeds::now_ms();
        conn.execute_batch(&format!(
            "INSERT INTO feeds (id, url, created_at) VALUES
               (1, 'https://a.example/rss', 0),
               (2, 'https://b.example/rss', 0);
             INSERT INTO articles
               (id, feed_id, title, published_at, created_at, score, read, starred) VALUES
               (1, 1, 'visible', 4, {now}, 0, 0, 0),
               (2, 1, 'hidden starred', 3, {now}, -11, 0, 1),
               (3, 1, 'hidden plain', 2, {now}, -11, 0, 0),
               (4, 2, 'other hidden starred', 1, {now}, -11, 0, 1);"
        ))
        .unwrap();
        let page = feeds_page(&conn, "feed:1", "starred", None).unwrap();
        let hidden = crate::queries::hidden_page(&conn, Some("feed:1"), Some("starred")).unwrap();
        assert_eq!(page.hidden_count, hidden.articles.len() as i64);
        assert_eq!(page.hidden_count, 1);
    }

    #[test]
    fn mark_scope_read_honors_folder_and_treats_invalid_text_as_all() {
        let (_dir, conn) = open();
        conn.execute_batch(
            "INSERT INTO folders (id, name, created_at) VALUES (1, 'news', 0);
             INSERT INTO feeds (id, url, folder_id, created_at) VALUES
               (10, 'https://in.example/rss', 1, 0),
               (11, 'https://out.example/rss', NULL, 0);
             INSERT INTO articles (id, feed_id, title, created_at, read, score) VALUES
               (1, 10, 'visible', 1, 0, 0),
               (2, 10, 'hidden', 1, 0, -11),
               (3, 11, 'outside', 1, 0, 0);",
        )
        .unwrap();
        let mut folder = actions::mark_scope_read(&conn, "folder:1").unwrap();
        folder.sort_unstable();
        assert_eq!(folder, vec![1]);
        let mut all = actions::mark_scope_read(&conn, "nope").unwrap();
        all.sort_unstable();
        assert_eq!(all, vec![3]);
        let still: Vec<i64> = conn
            .prepare("SELECT id FROM articles WHERE read = 0 ORDER BY id")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(|id| id.unwrap())
            .collect();
        assert_eq!(still, vec![2]);
    }
}
