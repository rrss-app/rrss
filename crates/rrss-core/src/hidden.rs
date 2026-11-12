//! Read-only hidden articles in the current feed/list scope.

use std::collections::BTreeMap;

use rusqlite::{Connection, params_from_iter};

use crate::Result;
use crate::articles::{self, ListScope};
use crate::feed_scope::{self, FeedScope};

#[derive(Debug)]
pub struct HiddenArticle {
    pub id: i64,
    pub title: String,
    pub feed_id: i64,
    pub feed_title: String,
    pub published_at: Option<i64>,
    pub score: i64,
    pub reasons_json: Option<String>,
    pub spot_liked: bool,
}

// Shared by the list, its count, and rule counts, including ordering and the 200-row cap.
fn hidden_sql(scope: FeedScope, list_scope: ListScope) -> String {
    let list_sql = list_scope.filter_sql();
    let (scope_sql, _) = feed_scope::scope_sql(scope);
    let liked = crate::rule_stats::SPOT_EVENT_SQL.replace("$kinds", "'open', 'up'");
    format!(
        "SELECT a.id, a.title, a.feed_id, COALESCE(f.title, f.url),
                a.published_at, a.score, a.reasons_json,
                EXISTS(SELECT 1 FROM exposures x WHERE x.article_id = a.id
                       AND x.variant = 'spot_check' AND {liked})
         FROM articles a JOIN feeds f ON f.id = a.feed_id
         WHERE a.expires_at IS NULL AND a.score <= ? AND a.released_at IS NULL AND a.created_at >= ?
           {list_sql}{scope_sql}
         ORDER BY a.published_at DESC, a.id DESC LIMIT 200"
    )
}

fn hidden_params(conn: &Connection, scope: FeedScope) -> Result<Vec<i64>> {
    let mut bind = vec![
        articles::hide_threshold(conn)?,
        crate::feeds::now_ms() - 7 * 24 * 60 * 60 * 1000,
    ];
    if let Some(id) = feed_scope::scope_sql(scope).1 {
        bind.push(id);
    }
    Ok(bind)
}

pub fn list_hidden(
    conn: &Connection,
    scope: FeedScope,
    list_scope: ListScope,
) -> Result<Vec<HiddenArticle>> {
    let mut stmt = conn.prepare(&hidden_sql(scope, list_scope))?;
    let rows = stmt.query_map(params_from_iter(hidden_params(conn, scope)?), |row| {
        Ok(HiddenArticle {
            id: row.get(0)?,
            title: row.get(1)?,
            feed_id: row.get(2)?,
            feed_title: row.get(3)?,
            published_at: row.get(4)?,
            score: row.get(5)?,
            reasons_json: row.get(6)?,
            spot_liked: row.get(7)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn hidden_count(conn: &Connection, scope: FeedScope, list_scope: ListScope) -> Result<i64> {
    let sql = format!("SELECT COUNT(*) FROM ({})", hidden_sql(scope, list_scope));
    conn.query_row(&sql, params_from_iter(hidden_params(conn, scope)?), |row| {
        row.get(0)
    })
    .map_err(Into::into)
}

// Invalid legacy JSON contributes no reasons; duplicate reasons count once per article.
pub(crate) const REASONS_SQL: &str =
    "json_each(CASE WHEN json_valid(a.reasons_json) THEN a.reasons_json ELSE '[]' END) reason";

pub fn rule_counts(
    conn: &Connection,
    scope: FeedScope,
    list_scope: ListScope,
) -> Result<BTreeMap<i64, i64>> {
    let sql = hidden_sql(scope, list_scope);
    let mut stmt = conn.prepare(&format!(
        "WITH hidden AS ({sql})
         SELECT json_extract(reason.value, '$.rule_id'), COUNT(DISTINCT a.id)
         FROM hidden a, {REASONS_SQL}
         WHERE reason.type = 'object' AND json_extract(reason.value, '$.weight') < 0
         GROUP BY json_extract(reason.value, '$.rule_id')"
    ))?;
    let rows = stmt.query_map(params_from_iter(hidden_params(conn, scope)?), |row| {
        Ok((row.get(0)?, row.get(1)?))
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(include_str!("schema.sql")).unwrap();
        conn
    }

    fn sorted_ids(rows: &[HiddenArticle]) -> Vec<i64> {
        let mut ids: Vec<_> = rows.iter().map(|row| row.id).collect();
        ids.sort_unstable();
        ids
    }

    #[test]
    fn folder_hidden_list_uses_the_passed_scope_and_matches_count() {
        let conn = mem();
        articles::set_hide_threshold(&conn, 0).unwrap();
        let now = crate::feeds::now_ms();
        let old = now - 8 * 24 * 60 * 60 * 1000;
        conn.execute_batch(&format!(
            "INSERT INTO folders (id, parent_id, name, created_at) VALUES
               (1, NULL, 'root', 0), (2, 1, 'child', 0), (9, NULL, 'other', 0);
             INSERT INTO feeds (id, url, folder_id, created_at) VALUES
               (10, 'https://child.example/rss', 2, 0),
               (11, 'https://root.example/rss', 1, 0),
               (12, 'https://out.example/rss', 9, 0);
             INSERT INTO articles
               (id, feed_id, title, published_at, created_at, read, starred, score) VALUES
               (1, 10, 'unread', 1, {now}, 0, 0, -5),
               (2, 10, 'read', 2, {now}, 1, 0, -5),
               (3, 10, 'old', 3, {old}, 0, 0, -5),
               (4, 10, 'visible', 4, {now}, 0, 0, 1),
               (5, 10, 'starred', 5, {now}, 1, 1, -5),
               (6, 11, 'root unread', 6, {now}, 0, 0, -5),
               (7, 12, 'outside', 7, {now}, 0, 0, -5);"
        ))
        .unwrap();
        articles::set_list_scope(&conn, ListScope::Starred).unwrap();
        let unread = list_hidden(&conn, FeedScope::Folder(1), ListScope::Unread).unwrap();
        assert_eq!(sorted_ids(&unread), vec![1, 6]);
        assert_eq!(
            hidden_count(&conn, FeedScope::Folder(1), ListScope::Unread).unwrap(),
            unread.len() as i64
        );
        articles::set_list_scope(&conn, ListScope::Unread).unwrap();
        let starred = list_hidden(&conn, FeedScope::Folder(1), ListScope::Starred).unwrap();
        assert_eq!(sorted_ids(&starred), vec![5]);
        assert_eq!(
            hidden_count(&conn, FeedScope::Folder(1), ListScope::Starred).unwrap(),
            starred.len() as i64
        );
    }
}
