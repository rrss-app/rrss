//! SQLite：WAL、外键、一次性 schema 与版本迁移。

pub mod metrics;
pub mod migrations;
pub mod pool;
pub mod settings;
pub mod site_rules;

pub use migrations::{MIN_UPGRADE_VERSION, SCHEMA_VERSION};
pub use pool::{PooledConnection, atomic_write, open, open_at, read_snapshot, write_transaction};
pub use settings::*;

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use tempfile::tempdir;

    const CORE_TABLES: &[&str] = &[
        "settings",
        "folders",
        "rsshub_instances",
        "feeds",
        "articles",
        "articles_fts",
        "media",
        "article_tags",
        "rules",
        "site_rules",
        "feedback",
        "interests",
        "ai_providers",
        "prompt_templates",
        "task_bindings",
        "ai_cache",
        "ai_usage",
        "threads",
        "messages",
        "memories",
        "chunks",
    ];

    #[test]
    fn per_host_fetch_limit_defaults_and_clamps() {
        let (_dir, conn) = open_temp();
        assert_eq!(
            feed_fetch_per_host(&conn).unwrap(),
            FEED_FETCH_PER_HOST_DEFAULT
        );
        for (stored, read) in [("1", 1), ("6", 6), ("0", 1), ("99", 8), ("x", 4)] {
            set_setting(&conn, FEED_FETCH_PER_HOST_KEY, stored).unwrap();
            assert_eq!(feed_fetch_per_host(&conn).unwrap(), read, "{stored}");
        }
    }

    fn open_temp() -> (tempfile::TempDir, PooledConnection) {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("rrss.db");
        let conn = open_at(&path).expect("open_at");
        (dir, conn)
    }

    #[test]
    fn open_creates_wal_and_schema() {
        let (_dir, conn) = open_temp();
        let mode: String = conn
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .unwrap();
        assert_eq!(mode.to_ascii_lowercase(), "wal");
        // WAL 下 NORMAL（1）：提交不逐次 fsync，落盘交给检查点。
        let synchronous: i64 = conn
            .pragma_query_value(None, "synchronous", |row| row.get(0))
            .unwrap();
        assert_eq!(synchronous, 1);

        let version: i32 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);

        for table in CORE_TABLES {
            let exists: i32 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE name = ?1",
                    [*table],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(exists, 1, "missing table {table}");
        }
    }

    #[test]
    fn reopen_is_idempotent() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("rrss.db");
        drop(open_at(&path).expect("first open"));
        let conn = open_at(&path).expect("second open");
        let version: i32 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
        assert_eq!(theme_preference(&conn).unwrap(), "system");
    }

    #[test]
    fn settings_roundtrip() {
        let (_dir, conn) = open_temp();
        assert_eq!(theme_preference(&conn).unwrap(), "system");
        set_theme_preference(&conn, "dark").unwrap();
        assert_eq!(theme_preference(&conn).unwrap(), "dark");
        set_setting(&conn, "hide_threshold", "-10").unwrap();
        assert_eq!(
            get_setting(&conn, "hide_threshold").unwrap().as_deref(),
            Some("-10")
        );
        assert_eq!(get_setting(&conn, "missing").unwrap(), None);
    }

    #[test]
    fn unsupported_schema_is_error() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("rrss.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.pragma_update(None, "user_version", 99).unwrap();
        }
        match open_at(&path) {
            Err(crate::error::Error::UnsupportedSchema {
                found: 99,
                supported: SCHEMA_VERSION,
            }) => {}
            Err(other) => panic!("unexpected error: {other}"),
            Ok(_) => panic!("expected unsupported schema error"),
        }
    }

    #[test]
    fn migrate_ensures_digest_period_unique_index() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("rrss.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(include_str!("../schema.sql")).unwrap();
            conn.execute("DROP INDEX IF EXISTS idx_digests_period_start", [])
                .unwrap();
            conn.execute(
                "INSERT INTO digests (period, period_start, body, created_at)
                 VALUES ('daily', 1, 'old', 1), ('daily', 1, 'newer', 2)",
                [],
            )
            .unwrap();
            conn.pragma_update(None, "user_version", SCHEMA_VERSION)
                .unwrap();
        }
        let conn = open_at(&path).expect("migrate");
        let idx: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name = 'idx_digests_period_start'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(idx, 1);
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM digests", [], |row| row.get(0))
            .unwrap();
        assert_eq!(n, 1);
        let id = crate::ai_store::insert_digest(&conn, "daily", 1, "upserted", "[]").unwrap();
        let body: String = conn
            .query_row("SELECT body FROM digests WHERE id = ?1", [id], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(body, "upserted");
    }
}
