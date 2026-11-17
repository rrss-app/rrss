use super::*;
use std::sync::atomic::{AtomicU32, Ordering};

fn sqlite_error(code: i32) -> rrss_core::Error {
    rrss_core::Error::Sqlite(rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error::new(code),
        None,
    ))
}

#[tokio::test(start_paused = true)]
async fn lock_variants_exhaust_the_original_attempt_and_backoff_budget() {
    for code in [
        rusqlite::ffi::SQLITE_BUSY,
        rusqlite::ffi::SQLITE_BUSY_RECOVERY,
        rusqlite::ffi::SQLITE_BUSY_SNAPSHOT,
        rusqlite::ffi::SQLITE_BUSY_TIMEOUT,
        rusqlite::ffi::SQLITE_LOCKED,
        rusqlite::ffi::SQLITE_LOCKED_SHAREDCACHE,
        rusqlite::ffi::SQLITE_LOCKED_VTAB,
    ] {
        let (_dir, path) = fixture();
        let capacity = model_capacity::Capacity::for_database(&path);
        let attempts = Arc::new(AtomicU32::new(0));
        let calls = attempts.clone();
        let started = tokio::time::Instant::now();
        let error = write_db::<(), _>(path.clone(), move |conn| {
            calls.fetch_add(1, Ordering::Relaxed);
            assert!(conn.is_autocommit());
            db::atomic_write(conn, |conn| {
                db::set_setting(conn, "retry_fixture", "rolled back")?;
                Err(sqlite_error(code))
            })
        })
        .await
        .unwrap_err();
        assert!(matches!(error.downcast_ref::<rrss_core::Error>(),
            Some(rrss_core::Error::Sqlite(rusqlite::Error::SqliteFailure(error, _)))
                if error.extended_code == code));
        assert_eq!(attempts.load(Ordering::Relaxed), 4);
        assert_eq!(started.elapsed(), Duration::from_millis(200 + 400 + 600));
        assert!(
            db::get_setting(&db::open_at(&path).unwrap(), "retry_fixture")
                .unwrap()
                .is_none()
        );
        assert_eq!(*capacity.active_work.borrow(), 0);
        assert_eq!(
            capacity.database.available_permits(),
            db::pool::BACKGROUND_CONNECTIONS
        );
        assert_eq!(capacity.metrics.db_queue.snapshot().count, 4);
        assert_eq!(capacity.metrics.db_work.snapshot().count, 4);
    }
}

#[tokio::test(start_paused = true)]
async fn retry_restarts_the_transaction_and_returns_the_first_success() {
    let (_dir, path) = fixture();
    let attempts = Arc::new(AtomicU32::new(0));
    let calls = attempts.clone();
    let started = tokio::time::Instant::now();
    let result = write_db(path.clone(), move |conn| {
        assert!(conn.is_autocommit());
        let attempt = calls.fetch_add(1, Ordering::Relaxed);
        db::atomic_write(conn, |conn| {
            assert!(db::get_setting(conn, "retry_fixture")?.is_none());
            db::set_setting(conn, "retry_fixture", "committed")?;
            if attempt == 0 {
                Err(sqlite_error(rusqlite::ffi::SQLITE_BUSY_SNAPSHOT))
            } else {
                Ok(17)
            }
        })
    })
    .await
    .unwrap();
    assert_eq!(result, 17);
    assert_eq!(attempts.load(Ordering::Relaxed), 2);
    assert_eq!(started.elapsed(), Duration::from_millis(200));
    assert_eq!(
        db::get_setting(&db::open_at(&path).unwrap(), "retry_fixture")
            .unwrap()
            .as_deref(),
        Some("committed")
    );
}

#[tokio::test(start_paused = true)]
async fn non_lock_errors_and_worker_panics_are_reported_without_retrying() {
    for case in 0..3 {
        let (_dir, path) = fixture();
        let capacity = model_capacity::Capacity::for_database(&path);
        let attempts = Arc::new(AtomicU32::new(0));
        let calls = attempts.clone();
        let started = tokio::time::Instant::now();
        let error = write_db::<(), _>(path, move |_| {
            calls.fetch_add(1, Ordering::Relaxed);
            match case {
                0 => Err(sqlite_error(rusqlite::ffi::SQLITE_IOERR_LOCK)),
                1 => Err(rrss_core::Error::Parse("database is locked".into())),
                _ => panic!("fixture refresh worker panic"),
            }
        })
        .await
        .unwrap_err();
        match case {
            0 => assert!(matches!(error.downcast_ref::<rrss_core::Error>(),
                Some(rrss_core::Error::Sqlite(rusqlite::Error::SqliteFailure(error, _)))
                    if error.extended_code == rusqlite::ffi::SQLITE_IOERR_LOCK)),
            1 => assert!(matches!(
                error.downcast_ref::<rrss_core::Error>(),
                Some(rrss_core::Error::Parse(_))
            )),
            _ => assert!(
                error
                    .downcast_ref::<tokio::task::JoinError>()
                    .is_some_and(|error| error.is_panic())
            ),
        }
        assert_eq!(attempts.load(Ordering::Relaxed), 1);
        assert_eq!(started.elapsed(), Duration::ZERO);
        assert_eq!(*capacity.active_work.borrow(), 0);
        assert_eq!(
            capacity.database.available_permits(),
            db::pool::BACKGROUND_CONNECTIONS
        );
        assert!(capacity.feed_writes.try_lock().is_ok());
        assert_eq!(capacity.metrics.db_queue.snapshot().count, 1);
        assert_eq!(capacity.metrics.db_work.snapshot().count, 1);
    }
}
