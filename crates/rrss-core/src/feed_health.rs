//! 订阅源健康：长期失败、长期没有新文章、噪声高。只提示，不自动退订，也不改刷新间隔（阶段 20 L4）。
//!
//! 判定按优先级取一种，在查询时算，不落库。用户「忽略」后，同一种状态 30 天内不再提示。

use std::collections::HashMap;

use rusqlite::{Connection, params};

use crate::error::{Error, Result};

const DAY_MS: i64 = 86_400_000;
/// 长期失败：连续失败次数与持续天数都要达到。
pub const FAIL_COUNT: i64 = 5;
pub const FAIL_DAYS: i64 = 7;
/// 订阅满这么多天才判断长期未更新。
pub const STALE_MIN_AGE_DAYS: i64 = 14;
/// 长期未更新的最短天数；平时发文慢的源按平均间隔的 4 倍。
pub const STALE_MIN_DAYS: i64 = 30;
const STALE_FACTOR: f64 = 4.0;
const STALE_WINDOW_DAYS: i64 = 180;
/// 忽略后重新提示的天数。
pub const DISMISS_DAYS: i64 = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthKind {
    Failing,
    Stale,
    Noisy,
}

impl HealthKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Failing => "failing",
            Self::Stale => "stale",
            Self::Noisy => "noisy",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct FeedHealth {
    pub kind: HealthKind,
    pub fail_count: i64,
    /// 连续失败已持续的天数。
    pub failing_days: i64,
    /// 距最近一篇新文章的天数（从未入库时按订阅时间）。
    pub quiet_days: i64,
    /// 近 180 天平均几天一篇；少于 2 篇时为空。
    pub avg_interval_days: Option<f64>,
    pub last_article_at: Option<i64>,
    pub last_success_at: Option<i64>,
    pub error: Option<String>,
}

struct Row {
    id: i64,
    created_at: i64,
    error: Option<String>,
    fail_count: i64,
    failing_since: Option<i64>,
    last_success_at: Option<i64>,
    dismissed_at: Option<i64>,
    dismissed_kind: Option<String>,
    last_article_at: Option<i64>,
    recent: i64,
    recent_first: Option<i64>,
    recent_last: Option<i64>,
}

/// 各源当前的健康提示；没有问题或已忽略的源不在结果里。
pub fn feed_health(conn: &Connection, now_ms: i64) -> Result<HashMap<i64, FeedHealth>> {
    let noisy = crate::feeds::noisy_feed_ids(conn)?;
    let mut stmt = conn.prepare(
        "SELECT f.id, f.created_at, f.error, f.fail_count, f.failing_since, f.last_success_at,
                f.health_dismissed_at, f.health_dismissed_kind,
                (SELECT MAX(a.created_at) FROM articles a WHERE a.feed_id = f.id),
                (SELECT COUNT(*) FROM articles a WHERE a.feed_id = f.id AND a.created_at >= ?1),
                (SELECT MIN(a.created_at) FROM articles a WHERE a.feed_id = f.id AND a.created_at >= ?1),
                (SELECT MAX(a.created_at) FROM articles a WHERE a.feed_id = f.id AND a.created_at >= ?1)
         FROM feeds f WHERE f.kind <> 'manual'",
    )?;
    let rows = stmt.query_map([now_ms - STALE_WINDOW_DAYS * DAY_MS], |r| {
        Ok(Row {
            id: r.get(0)?,
            created_at: r.get(1)?,
            error: r.get(2)?,
            fail_count: r.get(3)?,
            failing_since: r.get(4)?,
            last_success_at: r.get(5)?,
            dismissed_at: r.get(6)?,
            dismissed_kind: r.get(7)?,
            last_article_at: r.get(8)?,
            recent: r.get(9)?,
            recent_first: r.get(10)?,
            recent_last: r.get(11)?,
        })
    })?;
    let mut out = HashMap::new();
    for row in rows {
        let row = row?;
        let failing_days = row.failing_since.map_or(0, |t| (now_ms - t) / DAY_MS);
        let quiet_days = (now_ms - row.last_article_at.unwrap_or(row.created_at)) / DAY_MS;
        let avg_interval_days = match (row.recent, row.recent_first, row.recent_last) {
            (n, Some(first), Some(last)) if n >= 2 => {
                Some((last - first) as f64 / DAY_MS as f64 / (n - 1) as f64)
            }
            _ => None,
        };
        let stale_after = avg_interval_days.map_or(STALE_MIN_DAYS as f64, |avg| {
            (avg * STALE_FACTOR).max(STALE_MIN_DAYS as f64)
        });
        let kind = if row.fail_count >= FAIL_COUNT && failing_days >= FAIL_DAYS {
            HealthKind::Failing
        } else if now_ms - row.created_at >= STALE_MIN_AGE_DAYS * DAY_MS
            && quiet_days as f64 >= stale_after
        {
            HealthKind::Stale
        } else if noisy.contains(&row.id) {
            HealthKind::Noisy
        } else {
            continue;
        };
        let dismissed = row.dismissed_kind.as_deref() == Some(kind.as_str())
            && row
                .dismissed_at
                .is_some_and(|t| now_ms - t < DISMISS_DAYS * DAY_MS);
        if dismissed {
            continue;
        }
        out.insert(
            row.id,
            FeedHealth {
                kind,
                fail_count: row.fail_count,
                failing_days,
                quiet_days,
                avg_interval_days,
                last_article_at: row.last_article_at,
                last_success_at: row.last_success_at,
                error: row.error,
            },
        );
    }
    Ok(out)
}

/// 忽略当前提示：记下时间和状态种类，同一种状态 30 天内不再提示。
pub fn dismiss(conn: &Connection, feed_id: i64, now_ms: i64) -> Result<()> {
    let Some(health) = feed_health(conn, now_ms)?.remove(&feed_id) else {
        return Ok(());
    };
    let n = conn.execute(
        "UPDATE feeds SET health_dismissed_at = ?1, health_dismissed_kind = ?2 WHERE id = ?3",
        params![now_ms, health.kind.as_str(), feed_id],
    )?;
    if n == 0 {
        return Err(Error::FeedNotFound);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_at;
    use tempfile::tempdir;

    const NOW: i64 = 1000 * DAY_MS;

    fn feed(conn: &Connection, id: i64, created_days_ago: i64) {
        conn.execute(
            "INSERT INTO feeds (id, url, created_at) VALUES (?1, ?2, ?3)",
            params![
                id,
                format!("https://f{id}/rss"),
                NOW - created_days_ago * DAY_MS
            ],
        )
        .unwrap();
    }

    fn article(conn: &Connection, feed: i64, days_ago: i64) {
        conn.execute(
            "INSERT INTO articles (feed_id, title, created_at) VALUES (?1, 't', ?2)",
            params![feed, NOW - days_ago * DAY_MS],
        )
        .unwrap();
    }

    #[test]
    fn failing_needs_count_and_duration_and_outranks_stale() {
        let dir = tempdir().unwrap();
        let conn = open_at(&dir.path().join("h.db")).unwrap();
        feed(&conn, 1, 100);
        conn.execute(
            "UPDATE feeds SET fail_count = 12, failing_since = ?1, error = 'HTTP 404', last_success_at = ?2 WHERE id = 1",
            params![NOW - 9 * DAY_MS, NOW - 9 * DAY_MS - 3_600_000],
        )
        .unwrap();
        feed(&conn, 2, 100);
        conn.execute(
            "UPDATE feeds SET fail_count = 12, failing_since = ?1 WHERE id = 2",
            [NOW - 3 * DAY_MS],
        )
        .unwrap();
        article(&conn, 2, 1);
        let h = feed_health(&conn, NOW).unwrap();
        assert_eq!(h[&1].kind, HealthKind::Failing);
        assert_eq!((h[&1].fail_count, h[&1].failing_days), (12, 9));
        assert!(!h.contains_key(&2), "only 3 days of failures");
    }

    #[test]
    fn stale_uses_average_interval_and_minimum() {
        let dir = tempdir().unwrap();
        let conn = open_at(&dir.path().join("h.db")).unwrap();
        // 平时 6 天一篇，47 天没有新文章。
        feed(&conn, 1, 300);
        for d in [47, 53, 59, 65] {
            article(&conn, 1, d);
        }
        // 平时 20 天一篇，50 天没更新：阈值 80 天，不提示。
        feed(&conn, 2, 300);
        for d in [50, 70, 90, 110] {
            article(&conn, 2, d);
        }
        // 新订阅、从未入库：未满 14 天不提示。
        feed(&conn, 3, 5);
        // 订阅 40 天、从未入库：按订阅时间算。
        feed(&conn, 4, 40);
        let h = feed_health(&conn, NOW).unwrap();
        assert_eq!(h[&1].kind, HealthKind::Stale);
        assert_eq!(h[&1].quiet_days, 47);
        assert_eq!(h[&1].avg_interval_days, Some(6.0));
        assert!(!h.contains_key(&2) && !h.contains_key(&3));
        assert_eq!(h[&4].kind, HealthKind::Stale);
        assert_eq!(h[&4].avg_interval_days, None);
    }

    #[test]
    fn dismiss_hides_same_kind_for_thirty_days_only() {
        let dir = tempdir().unwrap();
        let conn = open_at(&dir.path().join("h.db")).unwrap();
        feed(&conn, 1, 100);
        article(&conn, 1, 60);
        assert_eq!(feed_health(&conn, NOW).unwrap()[&1].kind, HealthKind::Stale);
        dismiss(&conn, 1, NOW).unwrap();
        assert!(feed_health(&conn, NOW).unwrap().is_empty());
        assert!(feed_health(&conn, NOW + 29 * DAY_MS).unwrap().is_empty());
        assert_eq!(
            feed_health(&conn, NOW + 31 * DAY_MS).unwrap()[&1].kind,
            HealthKind::Stale
        );
        // 种类变化后立即提示。
        conn.execute(
            "UPDATE feeds SET fail_count = 6, failing_since = ?1 WHERE id = 1",
            [NOW - 8 * DAY_MS],
        )
        .unwrap();
        assert_eq!(
            feed_health(&conn, NOW).unwrap()[&1].kind,
            HealthKind::Failing
        );
    }

    #[test]
    fn fetch_results_update_failure_streak() {
        use crate::ingest::{FetchResponse, apply_fetch};
        let dir = tempdir().unwrap();
        let conn = open_at(&dir.path().join("h.db")).unwrap();
        feed(&conn, 1, 1);
        let row = crate::feeds::get_feed(&conn, 1).unwrap();
        let resp = |status| FetchResponse {
            status,
            body: Vec::new(),
            etag: None,
            last_modified: None,
            retry_after: None,
        };
        let state = || -> (i64, Option<i64>, Option<i64>) {
            conn.query_row(
                "SELECT fail_count, failing_since, last_success_at FROM feeds WHERE id = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap()
        };
        apply_fetch(&conn, &row, &resp(500), 10).unwrap();
        apply_fetch(&conn, &row, &resp(404), 20).unwrap();
        assert_eq!(state(), (2, Some(10), None));
        apply_fetch(&conn, &row, &resp(304), 30).unwrap();
        assert_eq!(state(), (0, None, Some(30)));
        apply_fetch(&conn, &row, &resp(503), 40).unwrap();
        assert_eq!(state(), (1, Some(40), Some(30)));
    }
}
