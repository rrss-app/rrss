use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

struct FeedReply(Arc<AtomicUsize>);
impl wiremock::Respond for FeedReply {
    fn respond(&self, _: &wiremock::Request) -> ResponseTemplate {
        match self.0.load(Ordering::Acquire) {
            0 => ResponseTemplate::new(200)
                .set_body_string(RSS)
                .set_delay(Duration::from_secs(10)),
            status => ResponseTemplate::new(status as u16),
        }
    }
}

#[tokio::test]
async fn quick_failures_keep_base_interval_slow_failures_back_off_and_manual_fetch_resets() {
    let server = MockServer::start().await;
    let reply = Arc::new(AtomicUsize::new(304));
    Mock::given(method("GET"))
        .respond_with(FeedReply(reply.clone()))
        .mount(&server)
        .await;
    let (_dir, path) = fixture();
    let conn = db::open_at(&path).unwrap();
    conn.execute(
        "INSERT INTO feeds(id,url,created_at) VALUES(1,?1,0)",
        [server.uri()],
    )
    .unwrap();
    let mut limits = runtime::Limits::default();
    limits.policy.timeout = Duration::from_secs(1);
    let scheduler =
        Scheduler::with_limits(path.clone(), NetworkClients::new(&path), |_| {}, limits);
    let fetch = || async {
        tokio::time::timeout(Duration::from_secs(10), scheduler.submit(1).unwrap().wait())
            .await
            .unwrap()
            .unwrap()
    };
    assert_eq!(fetch().await.status, RefreshStatus::NotModified);
    let interval = |hours: i64, slow: i64| {
        let feed = rrss_core::feeds::get_feed(&conn, 1).unwrap();
        assert_eq!(feed.slow_fail_count, slow);
        let delay = feed.next_fetch_at - feed.last_fetched_at.unwrap();
        let base = hours * 3_600_000;
        assert!(
            (base..=(base + base / 10).min(14_400_000)).contains(&delay),
            "delay {delay}, expected {hours} hours"
        );
        assert!(feed.next_fetch_at > now_ms());
    };
    reply.store(500, Ordering::Release);
    for _ in 0..2 {
        assert!(matches!(fetch().await.status, RefreshStatus::Failed { .. }));
        interval(1, 0);
    }
    reply.store(0, Ordering::Release);
    for (slow, hours) in [(1, 1), (2, 2), (3, 4)] {
        assert!(matches!(fetch().await.status, RefreshStatus::Failed { .. }));
        interval(hours, slow);
    }
    assert_eq!(
        conn.query_row("SELECT fail_count FROM feeds WHERE id=1", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        5
    );
    // The source is due hours from now. Explicit submit must still start and
    // complete a new fetch immediately, clearing only the slow streak on 500.
    reply.store(500, Ordering::Release);
    assert!(matches!(fetch().await.status, RefreshStatus::Failed { .. }));
    interval(1, 0);
    assert_eq!(
        conn.query_row("SELECT fail_count FROM feeds WHERE id=1", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        6
    );
    reply.store(0, Ordering::Release);
    for (slow, hours) in [(1, 1), (2, 2)] {
        fetch().await;
        interval(hours, slow);
    }
    reply.store(304, Ordering::Release);
    assert_eq!(fetch().await.status, RefreshStatus::NotModified);
    interval(1, 0);
    assert_eq!(
        conn.query_row("SELECT fail_count FROM feeds WHERE id=1", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert!(scheduler.shutdown().await);
}
