use super::*;
use crate::test_log::Capture;

fn response(status: u16, body: &[u8]) -> FetchResponse {
    FetchResponse {
        status,
        etag: None,
        last_modified: None,
        retry_after: None,
        body: body.into(),
    }
}

#[test]
fn failures_recovery_and_stale_responses_log_only_committed_health_changes() {
    let dir = tempfile::tempdir().unwrap();
    let conn = crate::db::open_at(&dir.path().join("test.db")).unwrap();
    conn.execute(
        "INSERT INTO feeds(id,url,created_at) VALUES(1,?1,0)",
        ["https://canary-user:canary-password@feed.test/private-feed?token=canary-query"],
    )
    .unwrap();
    let feed = crate::feeds::get_feed(&conn, 1).unwrap();
    let capture = Capture::new();
    capture.run(|| {
        for (kind, status) in [
            (ErrorKind::Dns, None), (ErrorKind::Refused, None), (ErrorKind::TlsCert, None),
            (ErrorKind::Timeout, None), (ErrorKind::ProxyAuth, Some(407)),
        ] {
            let schedule = FetchSchedule::default().with_failure(FailureClass::Slow).with_error(kind, status);
            // Deliberately reuse the original snapshot to verify dedupe reads
            // current DB state, not its stale error field.
            for _ in 0..2 { record_fetch_failure_scheduled(&conn, &feed, "canary-body", 1, schedule).unwrap(); }
            apply_fetch(&conn, &feed, &response(304, b""), 2).unwrap();
            let log = capture.take();
            assert_eq!(log.lines().filter(|line| line.contains(" WARN ")).count(), 1, "{log}");
            assert_eq!(log.matches("feed fetch still failing").count(), 1, "{log}");
            assert_eq!(log.matches("feed fetch recovered").count(), 1, "{log}");
            assert!(log.contains(&format!("error_kind=\"{kind}\"")), "{log}");
            assert!(log.contains("class=\"slow\"") && log.contains("host=\"feed.test\""), "{log}");
            assert!(!log.contains("canary-") && !log.contains("private-feed"), "{log}");
        }
        for _ in 0..2 { apply_fetch(&conn, &feed, &response(200, b"canary-malformed-body"), 3).unwrap(); }
        apply_fetch(&conn, &feed, &response(200, include_bytes!("../../../../testdata/feeds/rss2-sample.xml")), 4).unwrap();
        let log = capture.take();
        assert_eq!(log.matches("feed fetch failing").count(), 1, "{log}");
        assert_eq!(log.matches("feed fetch recovered").count(), 1, "{log}");
        assert!(log.contains("error_kind=\"parse\"") && !log.contains("canary-"), "{log}");

        for _ in 0..2 { apply_fetch(&conn, &feed, &response(503, b""), 4).unwrap(); }
        let log = capture.take();
        assert_eq!(log.matches("feed fetch failing").count(), 1, "{log}");
        assert_eq!(log.matches("feed fetch still failing").count(), 1, "{log}");
        assert!(log.contains("status=503") && log.contains("error_kind=\"http\""), "{log}");
        apply_fetch(&conn, &feed, &response(304, b""), 4).unwrap();
        capture.take();
        record_fetch_deferred(&conn, &feed, Some(100_000), "canary-deferral").unwrap();
        assert!(capture.take().is_empty());
        // A failed inner savepoint is silent; a successful inner savepoint
        // followed by outer rollback is equally silent and preserves health.
        let tx = crate::db::write_transaction(&conn).unwrap();
        let (_, event) = record_fetch_failure_pending(&tx, &feed, "canary-body", 5, FetchSchedule::default()).unwrap();
        drop(event);
        assert!(capture.take().is_empty());
        tx.rollback().unwrap();
        assert!(crate::feeds::get_feed(&conn, 1).unwrap().error.is_none());
        conn.execute_batch("CREATE TRIGGER fail_health BEFORE UPDATE OF error ON feeds BEGIN SELECT RAISE(ABORT,'blocked'); END").unwrap();
        assert!(record_fetch_failure(&conn, &feed, "canary-body").is_err());
        assert!(capture.take().is_empty());
        conn.execute_batch("DROP TRIGGER fail_health; UPDATE feeds SET url='https://changed.test/new' WHERE id=1").unwrap();
        record_fetch_failure(&conn, &feed, "old failure").unwrap();
        apply_fetch(&conn, &feed, &response(503, b""), 6).unwrap();
        assert!(capture.take().is_empty());
        conn.execute("DELETE FROM feeds WHERE id=1", []).unwrap();
        record_fetch_failure(&conn, &feed, "deleted failure").unwrap();
        assert!(capture.take().is_empty());
    });
}
