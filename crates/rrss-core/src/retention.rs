//! 数据保留：清掉过期文章的正文、向量、译文和配图，文章行和阅读记录保留（阶段 20 L3）。
//!
//! 文章行不能删：去重靠 `guid` / `canonical_url` / `hash`，对话与简报引用靠 `article_id`，
//! 删行还会级联删掉赞踩。年龄按入库时间 `created_at` 计算。全局 `retention_days` 为 0 表示关闭；
//! 源上的 `retention_days` 为 NULL 时跟随全局，为 0 时永久保留。

use rusqlite::{Connection, params};

use crate::db;
use crate::error::{Error, Result};

pub const SETTING: &str = "retention_days";
/// 开启时界面预设的天数。
pub const DEFAULT_DAYS: i64 = 90;
/// 每个写事务处理的篇数；批间让出写锁，不阻塞刷新。
pub const BATCH: i64 = 200;
const DAY_MS: i64 = 86_400_000;

pub fn global_days(conn: &Connection) -> Result<i64> {
    Ok(db::get_setting(conn, SETTING)?
        .and_then(|v| v.trim().parse::<i64>().ok())
        .filter(|d| *d > 0)
        .unwrap_or(0))
}

pub fn set_global_days(conn: &Connection, days: i64) -> Result<()> {
    if !(0..=36_500).contains(&days) {
        return Err(Error::InvalidRule(format!(
            "retention days out of range: {days}"
        )));
    }
    db::set_setting(conn, SETTING, &days.to_string())
}

pub fn set_feed_days(conn: &Connection, feed_id: i64, days: Option<i64>) -> Result<()> {
    if let Some(d) = days
        && !(0..=36_500).contains(&d)
    {
        return Err(Error::InvalidRule(format!(
            "retention days out of range: {d}"
        )));
    }
    let n = conn.execute(
        "UPDATE feeds SET retention_days = ?1 WHERE id = ?2",
        params![days, feed_id],
    )?;
    if n == 0 {
        return Err(Error::FeedNotFound);
    }
    Ok(())
}

/// 有任何源会清理（全局开启，或某个源单独设了天数）。
pub fn active(conn: &Connection) -> Result<bool> {
    if global_days(conn)? > 0 {
        return Ok(true);
    }
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM feeds WHERE retention_days > 0",
        [],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// 永久保留的文章：被对话消息引用、出现在简报来源里、作为对话锚点。收藏在主查询里单独判断。
/// JSON 里任意层级的 `article_id` 都算，只看 `json_valid` 的行。
const PROTECTED: &str = "
    SELECT CAST(j.value AS INTEGER) FROM messages m, json_tree(m.citations_json) j
     WHERE m.citations_json IS NOT NULL AND json_valid(m.citations_json)
       AND j.key = 'article_id' AND j.type = 'integer'
    UNION
    SELECT CAST(j.value AS INTEGER) FROM digests d, json_tree(d.items_json) j
     WHERE json_valid(d.items_json) AND j.key = 'article_id' AND j.type = 'integer'
    UNION
    SELECT anchor_id FROM threads WHERE anchor_kind = 'article' AND anchor_id IS NOT NULL";

fn due_sql(select: &str, tail: &str) -> String {
    format!(
        "WITH protected(id) AS ({PROTECTED})
         {select}
         FROM articles a JOIN feeds f ON f.id = a.feed_id
         WHERE a.expires_at IS NULL AND a.pruned_at IS NULL AND a.starred = 0
           AND COALESCE(f.retention_days, ?1) > 0
           AND a.created_at < ?2 - COALESCE(f.retention_days, ?1) * {DAY_MS}
           AND a.id NOT IN (SELECT id FROM protected WHERE id IS NOT NULL)
         {tail}"
    )
}

/// 预览：将清理的篇数和估计释放的字节数（被清列的长度加上向量、译文）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Preview {
    pub articles: i64,
    pub bytes: i64,
}

pub fn preview(conn: &Connection, now_ms: i64) -> Result<Preview> {
    let global = global_days(conn)?;
    let sql = due_sql(
        "SELECT COUNT(*),
                COALESCE(SUM(COALESCE(length(a.content_html), 0) + COALESCE(length(a.content_md), 0)
                  + COALESCE(length(a.fulltext_md), 0) + COALESCE(length(a.blocks_json), 0)
                  + COALESCE(length(a.summary_json), 0)
                  + (SELECT COALESCE(SUM(length(c.text) + COALESCE(length(c.embedding), 0)), 0) FROM chunks c WHERE c.article_id = a.id)
                  + (SELECT COALESCE(SUM(length(t.blocks_json)), 0) FROM article_translations t WHERE t.article_id = a.id)), 0)",
        "",
    );
    Ok(conn.query_row(&sql, params![global, now_ms], |r| {
        Ok(Preview {
            articles: r.get(0)?,
            bytes: r.get(1)?,
        })
    })?)
}

/// 下一批要清理的文章 id。
pub fn due_batch(conn: &Connection, now_ms: i64, limit: i64) -> Result<Vec<i64>> {
    let global = global_days(conn)?;
    let sql = due_sql("SELECT a.id", "ORDER BY a.id LIMIT ?3");
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![global, now_ms, limit], |r| r.get(0))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// 清掉一批文章的正文、向量、译文和本地媒体。调用方负责事务。
pub fn prune_articles(conn: &Connection, ids: &[i64], now_ms: i64) -> Result<i64> {
    let mut n = 0;
    for id in ids {
        let media: Vec<(i64, Option<String>)> = {
            let mut stmt =
                conn.prepare("SELECT id, local_path FROM media WHERE article_id = ?1")?;
            stmt.query_map([id], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?
        };
        for (media_id, path) in media {
            match path.filter(|p| !p.is_empty()) {
                Some(path) => crate::media::remove_cached(conn, media_id, &path)?,
                None => {
                    conn.execute("DELETE FROM media WHERE id = ?1", [media_id])?;
                }
            }
        }
        conn.execute("DELETE FROM chunks WHERE article_id = ?1", [id])?;
        conn.execute(
            "DELETE FROM article_translations WHERE article_id = ?1",
            [id],
        )?;
        // 触发器把全文索引的正文列更新为空，只剩标题和作者可搜。
        n += conn.execute(
            "UPDATE articles SET content_html = NULL, content_md = NULL, fulltext_md = NULL,
                    blocks_json = NULL, summary_json = NULL, pruned_at = ?2, embed_tier = 0
             WHERE id = ?1 AND pruned_at IS NULL",
            params![id, now_ms],
        )? as i64;
    }
    Ok(n)
}

/// Temporary pages expire independently of the optional body retention policy.
/// Recheck expiry while holding the writer lock so a concurrent collect/reopen wins safely.
pub fn delete_expired(conn: &Connection, now_ms: i64) -> Result<i64> {
    let mut total = 0;
    loop {
        let tx = db::write_transaction(conn)?;
        let ids: Vec<i64> = tx
            .prepare("SELECT id FROM articles WHERE expires_at <= ?1 ORDER BY expires_at LIMIT ?2")?
            .query_map(params![now_ms, BATCH], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        if ids.is_empty() {
            break;
        }
        // Remove files through the existing cache path before FK cascades remove their rows.
        prune_articles(&tx, &ids, now_ms)?;
        for id in ids {
            total += tx.execute(
                "DELETE FROM articles WHERE id=?1 AND expires_at <= ?2",
                params![id, now_ms],
            )? as i64;
        }
        tx.commit()?;
    }
    Ok(total)
}

/// 手动重新抓取正文后调用：清掉标记，之后照常参与保留判断。
#[cfg(test)]
pub fn clear_pruned(conn: &Connection, article_id: i64) -> Result<()> {
    conn.execute(
        "UPDATE articles SET pruned_at = NULL WHERE id = ?1",
        [article_id],
    )?;
    Ok(())
}

/// 清理过至少一篇文章（决定第一次清理前是否要自动备份）。
pub fn ever_pruned(conn: &Connection) -> Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM articles WHERE pruned_at IS NOT NULL)",
        [],
        |r| r.get(0),
    )?;
    Ok(n != 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_at;
    use tempfile::tempdir;

    const NOW: i64 = 400 * DAY_MS;

    fn seed(conn: &Connection) {
        conn.execute_batch(&format!(
            "INSERT INTO feeds (id, url, created_at) VALUES (1, 'https://a/rss', 0), (2, 'https://b/rss', 0);
             INSERT INTO articles (id, feed_id, guid, title, content_html, blocks_json, summary, summary_json, hash, starred, created_at) VALUES
               (1, 1, 'g1', 'old plain', '<p>body one</p>', '[1]', 'short', '{{}}', 'h1', 0, {old}),
               (2, 1, 'g2', 'old starred', '<p>body</p>', '[1]', NULL, NULL, 'h2', 1, {old}),
               (3, 1, 'g3', 'old cited', '<p>body</p>', '[1]', NULL, NULL, 'h3', 0, {old}),
               (4, 1, 'g4', 'old digest', '<p>body</p>', '[1]', NULL, NULL, 'h4', 0, {old}),
               (5, 1, 'g5', 'old anchor', '<p>body</p>', '[1]', NULL, NULL, 'h5', 0, {old}),
               (6, 1, 'g6', 'fresh', '<p>body</p>', '[1]', NULL, NULL, 'h6', 0, {fresh}),
               (7, 2, 'g7', 'old other feed', '<p>body</p>', '[1]', NULL, NULL, 'h7', 0, {old});
             INSERT INTO threads (id, title, anchor_kind, anchor_id, created_at, updated_at) VALUES (1, 't', 'article', 5, 0, 0);
             INSERT INTO messages (thread_id, role, content, citations_json, created_at) VALUES
               (1, 'user', 'q', '[{{\"kind\":\"article\",\"article_id\":3}}]', 0),
               (1, 'user', 'bad', 'not json', 0);
             INSERT INTO digests (period, period_start, body, items_json, created_at) VALUES
               ('daily', 0, 'b', '[{{\"title\":\"x\",\"sources\":[{{\"article_id\":4}}]}}]', 0);
             INSERT INTO chunks (article_id, ordinal, text, embedding, model) VALUES (1, 0, 'chunk', x'0000', 'm');
             INSERT INTO article_translations (article_id, target_lang, source_hash, blocks_json, created_at) VALUES (1, 'zh', 'h1', '[]', 0);
             INSERT INTO feedback (article_id, kind, created_at) VALUES (1, 'up', 0);",
            old = NOW - 100 * DAY_MS,
            fresh = NOW - 10 * DAY_MS,
        ))
        .unwrap();
    }

    #[test]
    fn off_by_default_and_protects_starred_cited_digest_and_anchor() {
        let dir = tempdir().unwrap();
        let conn = open_at(&dir.path().join("r.db")).unwrap();
        seed(&conn);
        assert_eq!(global_days(&conn).unwrap(), 0);
        assert!(!active(&conn).unwrap());
        assert!(due_batch(&conn, NOW, 100).unwrap().is_empty());
        assert_eq!(preview(&conn, NOW).unwrap(), Preview::default());

        set_global_days(&conn, 90).unwrap();
        assert_eq!(due_batch(&conn, NOW, 100).unwrap(), vec![1, 7]);
        let p = preview(&conn, NOW).unwrap();
        assert_eq!(p.articles, 2);
        assert!(p.bytes > 0);

        // 源单独设为永久保留；另一个源单独设更短的天数也会生效。
        set_feed_days(&conn, 2, Some(0)).unwrap();
        assert_eq!(due_batch(&conn, NOW, 100).unwrap(), vec![1]);
        set_global_days(&conn, 0).unwrap();
        set_feed_days(&conn, 1, Some(30)).unwrap();
        assert!(active(&conn).unwrap());
        assert_eq!(due_batch(&conn, NOW, 100).unwrap(), vec![1]);
        assert!(set_feed_days(&conn, 99, None).is_err());
        assert!(set_global_days(&conn, -1).is_err());
    }

    #[test]
    fn prune_clears_content_keeps_row_feedback_and_title_search() {
        let dir = tempdir().unwrap();
        let conn = open_at(&dir.path().join("r.db")).unwrap();
        seed(&conn);
        assert!(!ever_pruned(&conn).unwrap());
        assert_eq!(prune_articles(&conn, &[1], NOW).unwrap(), 1);
        assert!(ever_pruned(&conn).unwrap());
        let row: (Option<String>, Option<String>, Option<String>, String, String, Option<i64>) = conn
            .query_row(
                "SELECT content_html, blocks_json, summary_json, summary, guid, pruned_at FROM articles WHERE id = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
            )
            .unwrap();
        assert_eq!(
            row,
            (None, None, None, "short".into(), "g1".into(), Some(NOW))
        );
        let count = |sql: &str| -> i64 { conn.query_row(sql, [], |r| r.get(0)).unwrap() };
        assert_eq!(count("SELECT COUNT(*) FROM chunks WHERE article_id = 1"), 0);
        assert_eq!(
            count("SELECT COUNT(*) FROM article_translations WHERE article_id = 1"),
            0
        );
        assert_eq!(
            count("SELECT COUNT(*) FROM feedback WHERE article_id = 1"),
            1
        );
        assert_eq!(
            count("SELECT COUNT(*) FROM articles_fts WHERE articles_fts MATCH 'body one'"),
            0
        );
        assert_eq!(
            count("SELECT COUNT(*) FROM articles_fts WHERE articles_fts MATCH 'old plain'"),
            1
        );
        // 再清一次不重复计数；清掉标记后重新参与判断。
        assert_eq!(prune_articles(&conn, &[1], NOW).unwrap(), 0);
        clear_pruned(&conn, 1).unwrap();
        assert!(!ever_pruned(&conn).unwrap());
    }
}
