//! Feed refresh interval: a global default in minutes plus optional per-feed overrides.
//!
//! `feeds.interval_secs` always holds the effective interval the scheduler reads;
//! `feeds.interval_custom` marks an override. Changing the global default rewrites
//! every non-custom feed, so the fetch path never has to consult the setting.

use rusqlite::{Connection, params};

use crate::db;
use crate::error::settings::SettingProblem;
use crate::error::{Error, Result};

pub const SETTING: &str = "feed_fetch_interval_minutes";
pub const DEFAULT_MINUTES: i64 = 60;
/// One minute up to one year. The scheduler wakes for the earliest due feed,
/// so short intervals are honoured without a fixed polling floor.
pub const RANGE_MINUTES: (i64, i64) = (1, 525_600);

pub fn check_minutes(minutes: i64) -> Result<i64> {
    let (min, max) = RANGE_MINUTES;
    if (min..=max).contains(&minutes) {
        Ok(minutes)
    } else {
        Err(Error::InvalidSetting(SettingProblem::OutOfRange {
            min,
            max,
        }))
    }
}

/// Stored default; missing or unreadable values fall back to the default, out-of-range ones are clamped.
pub fn default_minutes(conn: &Connection) -> Result<i64> {
    let (min, max) = RANGE_MINUTES;
    Ok(db::get_setting(conn, SETTING)?
        .and_then(|value| value.trim().parse::<i64>().ok())
        .unwrap_or(DEFAULT_MINUTES)
        .clamp(min, max))
}

/// New feeds start on the current default.
pub fn default_secs(conn: &Connection) -> Result<i64> {
    Ok(default_minutes(conn)? * 60)
}

// A changed interval moves an already fetched feed's deadline to its last fetch
// plus the new interval, in either direction. Never fetched feeds stay due;
// feeds in slow-failure backoff keep their backoff deadline, and server
// cooldowns are enforced at admission, independent of this column.
const RESCHEDULE: &str = "UPDATE feeds SET interval_secs = ?1,
    next_fetch_at = CASE
        WHEN last_fetched_at IS NULL OR next_fetch_at = 0 OR slow_fail_count > 0 THEN next_fetch_at
        ELSE last_fetched_at + ?1 * 1000 END";

/// Saves the global default and applies it to every feed without an override.
pub fn set_default_minutes(conn: &Connection, minutes: i64) -> Result<()> {
    let minutes = check_minutes(minutes)?;
    db::atomic_write(conn, |conn| {
        db::set_setting(conn, SETTING, &minutes.to_string())?;
        conn.execute(
            &format!(
                "{RESCHEDULE} WHERE interval_custom = 0 AND kind != 'manual' AND interval_secs != ?1"
            ),
            [minutes * 60],
        )?;
        Ok(())
    })
}

/// `None` follows the global default; `Some(minutes)` overrides it for this feed.
pub fn set_feed_minutes(conn: &Connection, feed_id: i64, minutes: Option<i64>) -> Result<()> {
    if let Some(minutes) = minutes {
        check_minutes(minutes)?;
    }
    db::atomic_write(conn, |conn| {
        let secs = match minutes {
            Some(minutes) => minutes * 60,
            None => default_secs(conn)?,
        };
        let n = conn.execute(
            "UPDATE feeds SET interval_custom = ?1 WHERE id = ?2",
            params![minutes.is_some(), feed_id],
        )?;
        if n == 0 {
            return Err(Error::FeedNotFound);
        }
        conn.execute(
            &format!("{RESCHEDULE} WHERE id = ?2 AND interval_secs != ?1"),
            params![secs, feed_id],
        )?;
        Ok(())
    })
}

/// Per-feed override in minutes; `None` when the feed follows the default.
pub fn feed_overrides(conn: &Connection) -> Result<std::collections::HashMap<i64, i64>> {
    let mut stmt = conn.prepare("SELECT id, interval_secs FROM feeds WHERE interval_custom = 1")?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)? / 60)))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Earliest persisted deadline among fetchable feeds, for the scheduler's next wake.
pub fn earliest_due(conn: &Connection) -> Result<Option<i64>> {
    Ok(conn.query_row(
        "SELECT MIN(next_fetch_at) FROM feeds WHERE kind != 'manual'",
        [],
        |r| r.get(0),
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> (tempfile::TempDir, db::PooledConnection) {
        let dir = tempfile::tempdir().unwrap();
        let conn = db::open_at(&dir.path().join("interval.db")).unwrap();
        conn.execute_batch(
            "INSERT INTO feeds(id,url,created_at,last_fetched_at,next_fetch_at) VALUES
               (1,'https://a.example/feed',0,1000000,1000000+3600000),
               (2,'https://b.example/feed',0,NULL,0),
               (3,'https://c.example/feed',0,1000000,1000000+3600000);",
        )
        .unwrap();
        (dir, conn)
    }

    fn row(conn: &Connection, id: i64) -> (i64, bool, i64) {
        conn.query_row(
            "SELECT interval_secs, interval_custom, next_fetch_at FROM feeds WHERE id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap()
    }

    #[test]
    fn default_applies_to_following_feeds_and_moves_their_deadline() {
        let (_dir, conn) = conn();
        assert_eq!(default_minutes(&conn).unwrap(), DEFAULT_MINUTES);
        set_feed_minutes(&conn, 3, Some(5)).unwrap();
        assert_eq!(row(&conn, 3), (300, true, 1_000_000 + 300_000));

        set_default_minutes(&conn, 30).unwrap();
        assert_eq!(default_minutes(&conn).unwrap(), 30);
        assert_eq!(row(&conn, 1), (1800, false, 1_000_000 + 1_800_000));
        // Never fetched stays due; the override keeps its own interval.
        assert_eq!(row(&conn, 2), (1800, false, 0));
        assert_eq!(row(&conn, 3), (300, true, 1_000_000 + 300_000));
        assert_eq!(feed_overrides(&conn).unwrap().get(&3), Some(&5));
        assert_eq!(earliest_due(&conn).unwrap(), Some(0));

        // Lengthening pushes the deadline later.
        set_default_minutes(&conn, 24 * 60).unwrap();
        assert_eq!(row(&conn, 1), (86_400, false, 1_000_000 + 86_400_000));
    }

    #[test]
    fn following_again_takes_the_default_and_slow_backoff_keeps_its_deadline() {
        let (_dir, conn) = conn();
        set_default_minutes(&conn, 120).unwrap();
        set_feed_minutes(&conn, 1, Some(1)).unwrap();
        assert_eq!(row(&conn, 1), (60, true, 1_060_000));
        set_feed_minutes(&conn, 1, None).unwrap();
        assert_eq!(row(&conn, 1), (7200, false, 1_000_000 + 7_200_000));

        conn.execute(
            "UPDATE feeds SET slow_fail_count=2, next_fetch_at=99 WHERE id=3",
            [],
        )
        .unwrap();
        set_feed_minutes(&conn, 3, Some(10)).unwrap();
        assert_eq!(row(&conn, 3), (600, true, 99));
    }

    #[test]
    fn rejects_out_of_range_and_missing_feeds_without_writing() {
        let (_dir, conn) = conn();
        for minutes in [0, -5, RANGE_MINUTES.1 + 1] {
            assert!(matches!(
                set_feed_minutes(&conn, 1, Some(minutes)),
                Err(Error::InvalidSetting(SettingProblem::OutOfRange { .. }))
            ));
            assert!(set_default_minutes(&conn, minutes).is_err());
        }
        assert!(matches!(
            set_feed_minutes(&conn, 99, Some(5)),
            Err(Error::FeedNotFound)
        ));
        assert_eq!(row(&conn, 1), (3600, false, 1_000_000 + 3_600_000));
        assert_eq!(db::get_setting(&conn, SETTING).unwrap(), None);
    }

    #[test]
    fn new_subscriptions_start_on_the_default() {
        let (_dir, conn) = conn();
        set_default_minutes(&conn, 15).unwrap();
        let id = crate::feeds::subscribe(
            &conn,
            crate::feeds::SubscribeRequest {
                url: "https://new.example/feed".into(),
                folder_id: None,
                as_rsshub: false,
                rsshub_instance_id: None,
            },
        )
        .unwrap();
        assert_eq!(row(&conn, id), (900, false, 0));
    }
}
