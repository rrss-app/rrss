//! 阶段 23「范围浏览 S1」：按发布时间倒序的订阅时间线。

use std::collections::{BTreeMap, HashMap, HashSet};

use rusqlite::{Connection, params_from_iter};

use crate::articles::{self, ArticleCard, ListScope};
use crate::error::Result;
use crate::feed_scope::{self, FeedScope};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimelineCursor {
    pub at: i64,
    pub id: i64,
}

const SORT_AT: &str = "COALESCE(a.published_at, a.created_at)";

/// 可见条件与焦点相同，不读焦点的 `focus_filter`。多取一条来判断下一页游标。
pub fn list_feeds_page(
    conn: &Connection,
    scope: FeedScope,
    list_scope: ListScope,
    cursor: Option<TimelineCursor>,
    limit: i64,
) -> Result<(Vec<ArticleCard>, Option<TimelineCursor>)> {
    let limit = limit.max(1);
    let threshold = articles::hide_threshold(conn)?;
    let list_sql = list_scope.filter_sql();
    let (scope_sql, scope_id) = feed_scope::scope_sql(scope);
    let cursor_sql = if cursor.is_some() {
        format!(" AND ({SORT_AT}, a.id) < (?, ?)")
    } else {
        String::new()
    };
    let kept = visible_kept_sql();
    let sql = format!(
        "SELECT a.id, a.title, COALESCE(f.title, f.url), a.published_at, a.canonical_url,
                a.read, a.starred, a.score, a.interest_boost, COALESCE(a.summary, ''),
                a.feed_id, a.cluster_id, a.interest_reasons_json, 0, NULL, {SORT_AT}
         FROM articles a
         JOIN feeds f ON f.id = a.feed_id
         WHERE {kept}{list_sql}{scope_sql}{cursor_sql}
         ORDER BY {SORT_AT} DESC, a.id DESC
         LIMIT ?"
    );
    let mut bind = Vec::with_capacity(6);
    bind.push(threshold);
    if let Some(id) = scope_id {
        bind.push(id);
    }
    if let Some(cursor) = &cursor {
        bind.push(cursor.at);
        bind.push(cursor.id);
    }
    bind.push(limit + 1);
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(bind), |row| {
        let mut card = articles::map_card(row)?;
        card.feed_title = crate::feeds::display_title(&card.feed_title, "");
        Ok((card, row.get::<_, i64>(15)?))
    })?;
    let mut pairs: Vec<(ArticleCard, i64)> = rows.collect::<rusqlite::Result<_>>()?;
    let take = usize::try_from(limit).unwrap_or(usize::MAX);
    let next = if pairs.len() > take {
        pairs.truncate(take);
        pairs.last().map(|(card, at)| TimelineCursor {
            at: *at,
            id: card.id,
        })
    } else {
        None
    };
    let cards = pairs.into_iter().map(|(card, _)| card).collect();
    Ok((cards, next))
}

/// 列表与计数共用：规则可见且未清理。含一个匿名 `?`（隐藏阈值），不含未读条件。
fn visible_kept_sql() -> String {
    format!(
        "{} AND a.pruned_at IS NULL",
        articles::FOCUS_VISIBLE_SQL.replace("$threshold", "?")
    )
}

fn id_parent_map(conn: &Connection, sql: &str) -> Result<HashMap<i64, Option<i64>>> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, Option<i64>>(1)?))
    })?;
    rows.collect::<rusqlite::Result<_>>().map_err(Into::into)
}

#[derive(Debug, PartialEq, Eq)]
pub struct FeedCounts {
    pub total: i64,
    pub feeds: BTreeMap<i64, i64>,
    pub folders: BTreeMap<i64, i64>,
}

/// 每个源的可见未读数，以及文件夹（含子文件夹、沿祖先向上）的合计。噪声源计入。
pub fn feed_counts(conn: &Connection) -> Result<FeedCounts> {
    let sql = format!(
        "SELECT a.feed_id, COUNT(*) FROM articles a
         WHERE a.read = 0 AND {}
         GROUP BY a.feed_id",
        visible_kept_sql()
    );
    let mut stmt = conn.prepare(&sql)?;
    let feeds = stmt
        .query_map([articles::hide_threshold(conn)?], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
        })?
        .collect::<rusqlite::Result<BTreeMap<_, _>>>()?;
    let parents = id_parent_map(conn, "SELECT id, parent_id FROM folders")?;
    let homes = id_parent_map(conn, "SELECT id, folder_id FROM feeds")?;
    let mut folders: BTreeMap<i64, i64> = BTreeMap::new();
    for (&feed_id, &n) in &feeds {
        let mut seen = HashSet::new();
        let mut current = homes.get(&feed_id).copied().flatten();
        while let Some(id) = current {
            if !seen.insert(id) {
                break;
            }
            *folders.entry(id).or_default() += n;
            current = parents.get(&id).copied().flatten();
        }
    }
    let total = feeds.values().sum();
    Ok(FeedCounts {
        total,
        feeds,
        folders,
    })
}

/// 把范围内规则可见的未读标为已读，返回改动的 id。被规则隐藏的不碰；已清理正文的照标。
pub fn mark_scope_read(conn: &Connection, scope: FeedScope) -> Result<Vec<i64>> {
    let visible = articles::FOCUS_VISIBLE_SQL.replace("$threshold", "?");
    let (scope_sql, scope_id) = feed_scope::scope_sql(scope);
    let sql = format!(
        "UPDATE articles SET read = 1 WHERE id IN (
            SELECT a.id FROM articles a WHERE a.read = 0 AND {visible}{scope_sql}
         ) RETURNING id"
    );
    let mut bind = vec![articles::hide_threshold(conn)?];
    if let Some(id) = scope_id {
        bind.push(id);
    }
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(bind), |row| row.get(0))?;
    rows.collect::<rusqlite::Result<_>>().map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::articles::FocusFilter;

    fn mem() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(include_str!("schema.sql")).unwrap();
        conn
    }

    fn ids(cards: &[ArticleCard]) -> Vec<i64> {
        cards.iter().map(|c| c.id).collect()
    }

    #[test]
    fn same_timestamp_pages_cover_every_row_once() {
        let conn = mem();
        conn.execute_batch(
            "INSERT INTO feeds (id, url, created_at) VALUES (1, 'https://www.solidot.org/index.rss', 0);
             INSERT INTO articles (id, feed_id, title, published_at, created_at) VALUES
               (1, 1, 'a', 1000, 1000),
               (2, 1, 'b', 1000, 1000),
               (3, 1, 'c', 1000, 1000),
               (4, 1, 'd', 1000, 1000),
               (5, 1, 'e', 1000, 1000);
             INSERT INTO article_rank (article_id, boost, reasons_json, algo_version, computed_at)
               VALUES (1, 999, '[]', 1, 0);",
        )
        .unwrap();
        let (p1, c1) = list_feeds_page(&conn, FeedScope::All, ListScope::All, None, 2).unwrap();
        let (p2, c2) = list_feeds_page(&conn, FeedScope::All, ListScope::All, c1, 2).unwrap();
        let (p3, c3) = list_feeds_page(&conn, FeedScope::All, ListScope::All, c2, 2).unwrap();
        assert_eq!(ids(&p1), vec![5, 4]);
        assert_eq!(ids(&p2), vec![3, 2]);
        assert_eq!(ids(&p3), vec![1]);
        assert!(c3.is_none());
        let ranked = p3.iter().find(|c| c.id == 1).unwrap();
        assert_eq!(ranked.rank_boost, 0);
        assert!(ranked.rank_reasons.is_empty());
        assert_eq!(p1[0].feed_title, "solidot.org");
    }

    #[test]
    fn insert_between_pages_does_not_shift_the_next_page() {
        let conn = mem();
        conn.execute_batch(
            "INSERT INTO feeds (id, url, created_at) VALUES (1, 'https://a.example/rss', 0);
             INSERT INTO articles (id, feed_id, title, published_at, created_at) VALUES
               (1, 1, 'a', 100, 100),
               (2, 1, 'b', 90, 90),
               (3, 1, 'c', 80, 80);",
        )
        .unwrap();
        let (page1, cursor) =
            list_feeds_page(&conn, FeedScope::All, ListScope::All, None, 2).unwrap();
        assert_eq!(ids(&page1), vec![1, 2]);
        let cursor = cursor.unwrap();
        assert_eq!((cursor.at, cursor.id), (90, 2));
        conn.execute_batch(
            "INSERT INTO articles (id, feed_id, title, published_at, created_at) VALUES
               (4, 1, 'between', 95, 95),
               (5, 1, 'newer', 200, 200);",
        )
        .unwrap();
        let (page2, next) =
            list_feeds_page(&conn, FeedScope::All, ListScope::All, Some(cursor), 2).unwrap();
        assert_eq!(ids(&page2), vec![3]);
        assert!(next.is_none());
    }

    #[test]
    fn folder_scope_includes_descendants_and_feed_scope_is_exact() {
        let conn = mem();
        conn.execute_batch(
            "INSERT INTO folders (id, parent_id, name, created_at) VALUES
               (1, NULL, 'root', 0),
               (2, 1, 'child', 0),
               (3, 2, 'grand', 0),
               (4, 1, 'sibling', 0),
               (9, NULL, 'other', 0),
               (5, 6, 'cycle-a', 0),
               (6, 5, 'cycle-b', 0);
             INSERT INTO feeds (id, url, folder_id, created_at) VALUES
               (10, 'https://grand.example/rss', 3, 0),
               (11, 'https://root.example/rss', 1, 0),
               (14, 'https://sib.example/rss', 4, 0),
               (12, 'https://other.example/rss', 9, 0),
               (13, 'https://loose.example/rss', NULL, 0),
               (16, 'https://cycle.example/rss', 6, 0);
             INSERT INTO articles (id, feed_id, title, published_at, created_at) VALUES
               (1, 10, 'grand', 50, 50),
               (2, 11, 'root', 40, 40),
               (3, 14, 'sibling', 30, 30),
               (4, 12, 'other', 20, 20),
               (5, 13, 'loose', 10, 10),
               (6, 16, 'cycle', 5, 5);",
        )
        .unwrap();
        let ids_of = |scope| {
            ids(&list_feeds_page(&conn, scope, ListScope::All, None, 20)
                .unwrap()
                .0)
        };
        assert_eq!(ids_of(FeedScope::Folder(1)), vec![1, 2, 3]);
        assert_eq!(ids_of(FeedScope::Folder(2)), vec![1]);
        assert_eq!(ids_of(FeedScope::Feed(10)), vec![1]);
        assert_eq!(ids_of(FeedScope::Folder(5)), vec![6]);
    }

    #[test]
    fn hidden_rows_stay_out_and_released_rows_stay_in() {
        let conn = mem();
        articles::set_hide_threshold(&conn, 0).unwrap();
        articles::set_focus_filter(&conn, FocusFilter::FocusOnly).unwrap();
        conn.execute_batch(
            "INSERT INTO feeds (id, url, created_at) VALUES (1, 'https://a.example/rss', 0);
             INSERT INTO articles (id, feed_id, title, published_at, created_at, score, released_at) VALUES
               (1, 1, 'visible', 30, 30, 1, NULL),
               (2, 1, 'hidden', 20, 20, -5, NULL),
               (3, 1, 'released', 10, 10, -5, 1);",
        )
        .unwrap();
        let (page, _) = list_feeds_page(&conn, FeedScope::All, ListScope::All, None, 10).unwrap();
        assert_eq!(ids(&page), vec![1, 3]);
    }

    #[test]
    fn list_scope_keeps_unread_or_starred() {
        let conn = mem();
        conn.execute_batch(
            "INSERT INTO feeds (id, url, created_at) VALUES (1, 'https://a.example/rss', 0);
             INSERT INTO articles (id, feed_id, title, published_at, created_at, read, starred) VALUES
               (1, 1, 'unread', 30, 30, 0, 0),
               (2, 1, 'read star', 20, 20, 1, 1),
               (3, 1, 'read', 10, 10, 1, 0);",
        )
        .unwrap();
        let unread = list_feeds_page(&conn, FeedScope::All, ListScope::Unread, None, 10)
            .unwrap()
            .0;
        let starred = list_feeds_page(&conn, FeedScope::All, ListScope::Starred, None, 10)
            .unwrap()
            .0;
        assert_eq!(ids(&unread), vec![1]);
        assert_eq!(ids(&starred), vec![2]);
    }

    #[test]
    fn missing_published_at_sorts_by_created_at() {
        let conn = mem();
        conn.execute_batch(
            "INSERT INTO feeds (id, url, created_at) VALUES (1, 'https://a.example/rss', 0);
             INSERT INTO articles (id, feed_id, title, published_at, created_at) VALUES
               (1, 1, 'pub', 300, 1),
               (2, 1, 'created later', NULL, 250),
               (4, 1, 'pub mid', 200, 999),
               (3, 1, 'created early', NULL, 100);",
        )
        .unwrap();
        let (page, next) = list_feeds_page(&conn, FeedScope::All, ListScope::All, None, 2).unwrap();
        assert_eq!(ids(&page), vec![1, 2]);
        let (page2, next2) =
            list_feeds_page(&conn, FeedScope::All, ListScope::All, next, 2).unwrap();
        assert_eq!(ids(&page2), vec![4, 3]);
        assert!(next2.is_none());
    }

    #[test]
    fn feed_counts_roll_up_ancestors_and_skip_hidden() {
        let conn = mem();
        articles::set_hide_threshold(&conn, 0).unwrap();
        conn.execute_batch(
            "INSERT INTO folders (id, parent_id, name, created_at) VALUES
               (1, NULL, 'root', 0), (2, 1, 'child', 0), (3, NULL, 'other', 0),
               (4, NULL, 'empty', 0), (8, 9, 'cycle-a', 0), (9, 8, 'cycle-b', 0);
             INSERT INTO feeds (id, url, folder_id, created_at) VALUES
               (10, 'https://child.example/rss', 2, 0),
               (11, 'https://root.example/rss', 1, 0),
               (12, 'https://other.example/rss', 3, 0),
               (13, 'https://loose.example/rss', NULL, 0),
               (14, 'https://cycle.example/rss', 8, 0),
               (15, 'https://read.example/rss', 4, 0);
             INSERT INTO articles
               (id, feed_id, title, published_at, created_at, read, score, released_at, pruned_at)
             VALUES
               (1, 10, 'visible', 1, 1, 0, 1, NULL, NULL),
               (2, 10, 'read', 1, 1, 1, 1, NULL, NULL),
               (3, 10, 'pruned', 1, 1, 0, 1, NULL, 1),
               (4, 10, 'hidden', 1, 1, 0, -5, NULL, NULL),
               (5, 10, 'released', 1, 1, 0, -5, 1, NULL),
               (6, 11, 'root', 1, 1, 0, 1, NULL, NULL),
               (7, 12, 'other', 1, 1, 0, 1, NULL, NULL),
               (8, 13, 'loose', 1, 1, 0, 1, NULL, NULL),
               (9, 14, 'cycle', 1, 1, 0, 1, NULL, NULL),
               (10, 15, 'only read', 1, 1, 1, 1, NULL, NULL);",
        )
        .unwrap();
        let counts = feed_counts(&conn).unwrap();
        assert_eq!(
            counts.feeds,
            BTreeMap::from([(10, 2), (11, 1), (12, 1), (13, 1), (14, 1)])
        );
        assert_eq!(
            counts.folders,
            BTreeMap::from([(1, 3), (2, 2), (3, 1), (8, 1), (9, 1)])
        );
        assert_eq!(counts.total, counts.feeds.values().sum::<i64>());
    }

    #[test]
    fn unread_page_length_matches_feed_counts() {
        let conn = mem();
        articles::set_hide_threshold(&conn, 0).unwrap();
        conn.execute_batch(
            "INSERT INTO folders (id, parent_id, name, created_at) VALUES
               (1, NULL, 'root', 0), (2, 1, 'child', 0);
             INSERT INTO feeds (id, url, folder_id, created_at) VALUES
               (10, 'https://child.example/rss', 2, 0),
               (11, 'https://root.example/rss', 1, 0),
               (12, 'https://out.example/rss', NULL, 0);
             INSERT INTO articles
               (id, feed_id, title, published_at, created_at, read, score, pruned_at)
             VALUES
               (1, 10, 'a', 5, 5, 0, 1, NULL),
               (2, 10, 'b', 4, 4, 0, 1, NULL),
               (3, 10, 'pruned', 3, 3, 0, 1, 1),
               (4, 10, 'read', 2, 2, 1, 1, NULL),
               (5, 11, 'root', 1, 1, 0, 1, NULL),
               (6, 12, 'out', 1, 1, 0, 1, NULL);",
        )
        .unwrap();
        let counts = feed_counts(&conn).unwrap();
        let page_len = |scope| {
            list_feeds_page(&conn, scope, ListScope::Unread, None, 20)
                .unwrap()
                .0
                .len() as i64
        };
        assert_eq!(counts.feeds[&10], page_len(FeedScope::Feed(10)));
        assert_eq!(counts.folders[&1], page_len(FeedScope::Folder(1)));
    }

    #[test]
    fn mark_scope_read_marks_visible_unread_in_the_subtree() {
        let conn = mem();
        articles::set_hide_threshold(&conn, 0).unwrap();
        conn.execute_batch(
            "INSERT INTO folders (id, parent_id, name, created_at) VALUES
               (1, NULL, 'root', 0), (2, 1, 'child', 0), (9, NULL, 'other', 0);
             INSERT INTO feeds (id, url, folder_id, created_at) VALUES
               (10, 'https://child.example/rss', 2, 0),
               (11, 'https://root.example/rss', 1, 0),
               (12, 'https://out.example/rss', 9, 0);
             INSERT INTO articles
               (id, feed_id, title, created_at, read, score, released_at, pruned_at)
             VALUES
               (1, 10, 'visible', 1, 0, 1, NULL, NULL),
               (2, 10, 'hidden', 1, 0, -5, NULL, NULL),
               (3, 10, 'already', 1, 1, 1, NULL, NULL),
               (4, 11, 'root', 1, 0, 1, NULL, NULL),
               (5, 12, 'outside', 1, 0, 1, NULL, NULL),
               (6, 10, 'pruned', 1, 0, 1, NULL, 1);",
        )
        .unwrap();
        let mut changed = mark_scope_read(&conn, FeedScope::Folder(1)).unwrap();
        changed.sort_unstable();
        assert_eq!(changed, vec![1, 4, 6]);
        let still: Vec<i64> = conn
            .prepare("SELECT id FROM articles WHERE read = 0 ORDER BY id")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(|id| id.unwrap())
            .collect();
        assert_eq!(still, vec![2, 5]);
    }
}
