//! Bounded, database-scoped diagnostics. No SQL, paths or input content escapes.
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Fixed logarithmic microsecond buckets; percentiles are bucket upper bounds.
#[derive(Clone, Copy, Debug, Default)]
pub struct Distribution {
    buckets: [u64; 32],
    count: u64,
    total_us: u64,
    max_us: u64,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct TimingSnapshot {
    pub count: u64,
    pub total_ms: f64,
    pub max_ms: f64,
    pub p50_ms: f64,
    pub p95_ms: f64,
}

impl Distribution {
    pub fn record(&mut self, duration: Duration) {
        let us = duration.as_micros().min(u64::MAX as u128) as u64;
        let bucket = (u64::BITS - us.saturating_sub(1).leading_zeros()).min(31) as usize;
        self.buckets[bucket] = self.buckets[bucket].saturating_add(1);
        self.count = self.count.saturating_add(1);
        self.total_us = self.total_us.saturating_add(us);
        self.max_us = self.max_us.max(us);
    }

    pub fn snapshot(&self) -> TimingSnapshot {
        let percentile = |percent: u64| {
            let target = self.count.saturating_mul(percent).div_ceil(100);
            let mut seen: u64 = 0;
            for (i, count) in self.buckets.iter().enumerate() {
                seen = seen.saturating_add(*count);
                if seen >= target {
                    return (if i == 31 { self.max_us } else { 1u64 << i }).min(self.max_us) as f64
                        / 1000.0;
                }
            }
            self.max_us as f64 / 1000.0
        };
        TimingSnapshot {
            count: self.count,
            total_ms: self.total_us as f64 / 1000.0,
            max_ms: self.max_us as f64 / 1000.0,
            p50_ms: percentile(50),
            p95_ms: percentile(95),
        }
    }
}

#[derive(Default)]
pub struct Timing(Mutex<Distribution>);
impl Timing {
    pub fn record(&self, duration: Duration) {
        self.0.lock().unwrap().record(duration);
    }
    pub fn snapshot(&self) -> TimingSnapshot {
        self.0.lock().unwrap().snapshot()
    }
    pub fn start(&self) -> Timer<'_> {
        Timer(self, Instant::now())
    }
}

pub struct Timer<'a>(&'a Timing, Instant);
impl Drop for Timer<'_> {
    fn drop(&mut self) {
        self.0.record(self.1.elapsed());
    }
}

#[derive(Default)]
pub struct DatabaseMetrics {
    pub connection_wait: Timing,
    pub write_wait: Timing,
    pub write_hold: Timing,
    pub feed_parse: Timing,
    pub feed_ingest: Timing,
}

// The workspace uses bundled SQLite (client data is supported since 3.44).
// Keep the pool's metrics on each physical connection: &Connection consumers
// also include migrations and transactions, where the pool wrapper is erased.
// No filename normalization, registry lock or SQL is needed on the hot path.
const CONNECTION_METRICS: &std::ffi::CStr = c"rrss.database_metrics";

pub(super) fn attach(
    conn: &rusqlite::Connection,
    metrics: Arc<DatabaseMetrics>,
) -> rusqlite::Result<()> {
    unsafe extern "C" fn release(data: *mut std::ffi::c_void) {
        // SAFETY: attach transfers exactly one Box<Arc<_>> to SQLite. Its
        // destructor runs once on close/replacement, or registration failure.
        unsafe { drop(Box::from_raw(data.cast::<Arc<DatabaseMetrics>>())) };
    }
    let data = Box::into_raw(Box::new(metrics));
    // SAFETY: the handle is live for conn's borrow; the private static key and
    // boxed Arc outlive this call. SQLite owns data even if registration fails.
    let result = unsafe {
        rusqlite::ffi::sqlite3_set_clientdata(
            conn.handle(),
            CONNECTION_METRICS.as_ptr(),
            data.cast(),
            Some(release),
        )
    };
    if result == rusqlite::ffi::SQLITE_OK {
        Ok(())
    } else {
        Err(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(result),
            None,
        ))
    }
}

pub(crate) fn for_connection(conn: &rusqlite::Connection) -> Option<Arc<DatabaseMetrics>> {
    // SAFETY: only attach writes this key. Connection is !Sync and remains
    // borrowed while cloning the Arc, so its destructor cannot race this read.
    unsafe {
        rusqlite::ffi::sqlite3_get_clientdata(conn.handle(), CONNECTION_METRICS.as_ptr())
            .cast::<Arc<DatabaseMetrics>>()
            .as_ref()
            .cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_histogram_handles_empty_zero_and_long_samples() {
        let mut data = Distribution::default();
        assert_eq!(data.snapshot().p95_ms, 0.0);
        data.record(Duration::ZERO);
        for _ in 0..98 {
            data.record(Duration::from_millis(3));
        }
        data.record(Duration::from_secs(24 * 60 * 60));
        let snapshot = data.snapshot();
        assert_eq!(snapshot.count, 100);
        assert_eq!(snapshot.p50_ms, 4.096);
        assert_eq!(snapshot.p95_ms, 4.096);
        assert_eq!(snapshot.max_ms, 86_400_000.0);
        assert_eq!(snapshot.total_ms, 86_400_294.0);
    }

    #[test]
    fn pool_metrics_track_transactions_without_changing_atomicity() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("metrics.db");
        assert!(crate::db::pool::metrics(&path).is_none());
        assert!(
            !path.exists(),
            "a diagnostic lookup must not initialize storage"
        );
        let conn = crate::db::open_at(&path).unwrap();
        let metrics = crate::db::pool::metrics(&path).unwrap();
        let before = metrics.write_hold.snapshot().count;
        {
            let tx = crate::db::write_transaction(&conn).unwrap();
            crate::db::set_setting(&tx, "metrics_fixture", "rolled-back").unwrap();
        }
        assert_eq!(
            crate::db::get_setting(&conn, "metrics_fixture").unwrap(),
            None
        );
        {
            let tx = crate::db::write_transaction(&conn).unwrap();
            crate::db::set_setting(&tx, "metrics_fixture", "committed").unwrap();
            // Nested atomic writes remain savepoints; the outer write is timed once.
            crate::db::atomic_write(&tx, |conn| {
                crate::db::set_setting(conn, "metrics_nested", "yes")
            })
            .unwrap();
            tx.commit().unwrap();
        }
        assert_eq!(
            crate::db::get_setting(&conn, "metrics_fixture")
                .unwrap()
                .as_deref(),
            Some("committed")
        );
        assert_eq!(metrics.write_hold.snapshot().count, before + 2);
        assert!(metrics.write_wait.snapshot().count >= 2);
        assert_eq!(metrics.connection_wait.snapshot().count, 1);
        let other = dir.path().join("other.db");
        let _other = crate::db::open_at(&other).unwrap();
        assert!(!std::sync::Arc::ptr_eq(
            &metrics,
            &crate::db::pool::metrics(&other).unwrap()
        ));
    }

    #[test]
    fn connection_metrics_ownership_is_released_on_close() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        assert!(for_connection(&conn).is_none());
        let metrics = Arc::new(DatabaseMetrics::default());
        let weak = Arc::downgrade(&metrics);
        attach(&conn, metrics.clone()).unwrap();
        assert!(Arc::ptr_eq(&metrics, &for_connection(&conn).unwrap()));
        // Moving a connection between workers keeps its metrics attached.
        std::thread::spawn(move || {
            assert!(Arc::ptr_eq(&metrics, &for_connection(&conn).unwrap()));
            drop(conn);
            assert_eq!(Arc::strong_count(&metrics), 1);
        })
        .join()
        .unwrap();
        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn external_connections_do_not_inherit_pool_metrics_by_filename() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pool.db");
        let pooled = crate::db::open_at(&path).unwrap();
        assert!(for_connection(&pooled).is_some());
        let external = rusqlite::Connection::open(&path).unwrap();
        assert!(for_connection(&external).is_none());
    }

    #[test]
    fn exhausted_pool_waits_and_reuses_connections() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pool.db");
        let mut held = (0..8)
            .map(|_| crate::db::open_at(&path).unwrap())
            .collect::<Vec<_>>();
        let metrics = crate::db::pool::metrics(&path).unwrap();
        let (started, ready) = std::sync::mpsc::channel();
        let (done, finished) = std::sync::mpsc::channel();
        std::thread::scope(|scope| {
            scope.spawn(|| {
                started.send(()).unwrap();
                let conn = crate::db::open_at(&path).unwrap();
                conn.query_row("SELECT 1", [], |row| row.get::<_, i64>(0))
                    .unwrap();
                done.send(()).unwrap();
            });
            ready.recv().unwrap();
            assert!(finished.recv_timeout(Duration::from_millis(20)).is_err());
            held.pop();
            finished.recv_timeout(Duration::from_secs(5)).unwrap();
        });
        assert_eq!(metrics.connection_wait.snapshot().count, 9);
    }

    fn assert_path_metrics(path: &std::path::Path) {
        let conn = crate::db::open_at(path).unwrap();
        let metrics = crate::db::pool::metrics(path).unwrap();
        // Force a second physical connection, then reuse it after returning it.
        let second = crate::db::open_at(path).unwrap();
        drop(second);
        let second = crate::db::open_at(path).unwrap();
        let before = metrics.write_hold.snapshot().count;
        for conn in [&*conn, &*second] {
            crate::db::write_transaction(conn)
                .unwrap()
                .commit()
                .unwrap();
        }
        assert_eq!(metrics.write_hold.snapshot().count, before + 2);
        conn.execute(
            "INSERT INTO feeds(url,created_at) VALUES('https://metrics.test/rss',0)",
            [],
        )
        .unwrap();
        let feed = crate::feeds::get_feed(&conn, conn.last_insert_rowid()).unwrap();
        let response = crate::ingest::FetchResponse {
            status: 200,
            body: br#"<rss version="2.0"><channel><title>Metrics</title><link>https://metrics.test</link><description>fixture</description><item><guid>one</guid><title>One</title></item></channel></rss>"#.to_vec(),
            etag: None,
            last_modified: None,
            retry_after: None,
        };
        crate::ingest::apply_fetch(&second, &feed, &response, 1).unwrap();
        assert_eq!(metrics.feed_parse.snapshot().count, 1);
        assert_eq!(metrics.feed_ingest.snapshot().count, 1);
    }

    #[test]
    fn metrics_follow_connections_with_noncanonical_paths() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("nested");
        std::fs::create_dir(&nested).unwrap();
        assert_path_metrics(&nested.join("..").join("metrics.db"));
    }

    #[cfg(unix)]
    #[test]
    fn metrics_follow_connections_through_symlinked_directories() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        let alias = dir.path().join("alias");
        std::fs::create_dir(&real).unwrap();
        std::os::unix::fs::symlink(&real, &alias).unwrap();
        assert_path_metrics(&alias.join("metrics.db"));
    }
}
