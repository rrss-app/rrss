use super::super::cooldown::{Clock, SETTING, Time};
use super::*;
use std::sync::atomic::{AtomicI64, AtomicU64, AtomicUsize, Ordering};
use wiremock::matchers::path as request_path;

struct TestClock {
    utc: AtomicI64,
    elapsed: AtomicU64,
    origin: tokio::time::Instant,
}
impl TestClock {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            utc: AtomicI64::new(now_ms()),
            elapsed: AtomicU64::new(0),
            origin: tokio::time::Instant::now(),
        })
    }
    fn advance(&self, millis: u64) {
        self.utc.fetch_add(millis as i64, Ordering::AcqRel);
        self.elapsed.fetch_add(millis, Ordering::AcqRel);
    }
    fn jump(&self, millis: i64) {
        self.utc.fetch_add(millis, Ordering::AcqRel);
    }
}
impl Clock for TestClock {
    fn now(&self) -> Time {
        Time {
            utc: self.utc.load(Ordering::Acquire),
            mono: self.origin + Duration::from_millis(self.elapsed.load(Ordering::Acquire)),
        }
    }
}

async fn wait_until(mut condition: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(20), async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

fn seed(conn: &rusqlite::Connection, id: i64, url: String) {
    conn.execute(
        "INSERT INTO feeds(id,url,created_at) VALUES(?1,?2,0)",
        rusqlite::params![id, url],
    )
    .unwrap();
}
fn snapshot(conn: &rusqlite::Connection) -> serde_json::Value {
    serde_json::from_str(&db::get_setting(conn, SETTING).unwrap().unwrap()).unwrap()
}
async fn fetch(scheduler: &Scheduler, id: i64) -> RefreshResult {
    tokio::time::timeout(
        Duration::from_secs(10),
        scheduler.submit(id).unwrap().wait(),
    )
    .await
    .unwrap()
    .unwrap()
}
async fn operation(scheduler: &Scheduler) -> views::RefreshOperation {
    let started = scheduler.start_refresh().await.unwrap();
    wait_until(|| {
        scheduler
            .snapshot()
            .operations
            .iter()
            .any(|op| op.id == started.operation_id && op.done)
    })
    .await;
    scheduler
        .snapshot()
        .operations
        .into_iter()
        .find(|op| op.id == started.operation_id)
        .unwrap()
}

#[tokio::test]
async fn newly_received_cooldown_settles_already_parked_manual_and_due_claims() {
    const SOURCES: usize = 80;
    for due in [false, true] {
        let (url, mut arrived, release, server) = gated_response(
            b"HTTP/1.1 429 Too Many Requests\r\nRetry-After: 7200\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
        ).await;
        let (_dir, path) = fixture();
        let conn = db::open_at(&path).unwrap();
        for id in 1..=SOURCES {
            seed(&conn, id as i64, format!("{url}/{id}"));
        }
        let clock = TestClock::new();
        let scheduler = Scheduler::with_clock(
            path.clone(),
            NetworkClients::new(&path),
            |_| {},
            runtime::Limits {
                host: 1,
                ..Default::default()
            },
            clock.clone(),
        );
        if due {
            assert_eq!(scheduler.enqueue_due().await.unwrap(), SOURCES);
        }
        let operation = scheduler.start_refresh().await.unwrap();
        arrived.recv().await.unwrap();
        wait_until(|| scheduler.diagnostics().feeds.parked == SOURCES - 1).await;
        release.add_permits(1);
        wait_until(|| {
            scheduler
                .snapshot()
                .operations
                .iter()
                .any(|op| op.id == operation.operation_id && op.done)
        })
        .await;
        let result = scheduler
            .snapshot()
            .operations
            .into_iter()
            .find(|op| op.id == operation.operation_id)
            .unwrap();
        assert_eq!(
            (result.completed, result.skipped, result.failed),
            (SOURCES, SOURCES, 0)
        );
        assert!(
            arrived.try_recv().is_err(),
            "parked peers must not make HTTP requests during cooldown"
        );
        let (persisted, failures): (usize, i64) = conn
            .query_row(
                "SELECT COUNT(*),SUM(fail_count) FROM feeds WHERE next_fetch_at>=?1",
                [clock.now().utc + 7_200_000],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((persisted, failures), (SOURCES, 1));
        let lookups = scheduler.lookups();
        for _ in 0..3 {
            scheduler.shared.wake.notify_one();
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(
            scheduler.lookups(),
            lookups,
            "recorded deferrals must stay parked without rescanning"
        );
        assert!(scheduler.shutdown().await);
        server.abort();
    }
}

#[tokio::test]
async fn rate_limit_spreads_to_peers_is_restored_before_dispatch_and_manual_reports_delay() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(request_path("/rate"))
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "86400"))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(request_path("/peer"))
        .respond_with(ResponseTemplate::new(304))
        .mount(&server)
        .await;
    let other = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(304))
        .mount(&other)
        .await;
    let (_dir, path) = fixture();
    let conn = db::open_at(&path).unwrap();
    seed(&conn, 1, format!("{}/rate", server.uri()));
    seed(&conn, 2, format!("{}/peer", server.uri()));
    seed(&conn, 3, other.uri());
    let clock = TestClock::new();
    let limits = runtime::Limits {
        host: 1,
        ..Default::default()
    };
    let scheduler = Scheduler::with_clock(
        path.clone(),
        NetworkClients::new(&path),
        |_| {},
        limits,
        clock.clone(),
    );
    assert!(matches!(
        fetch(&scheduler, 1).await.status,
        RefreshStatus::Skipped {
            retry_at: Some(_),
            ..
        }
    ));
    let saved = snapshot(&conn);
    let until = saved["blocks"][0]["blocked_until"].as_i64().unwrap();
    assert_eq!(saved["version"], 2);
    assert!(until >= clock.now().utc + 86_400_000);
    assert!(rrss_core::feeds::get_feed(&conn, 1).unwrap().next_fetch_at >= until);
    assert_eq!(
        conn.query_row(
            "SELECT fail_count,slow_fail_count FROM feeds WHERE id=1",
            [],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
        )
        .unwrap(),
        (1, 0)
    );
    let view = operation(&scheduler).await;
    assert_eq!(
        (view.target, view.completed, view.skipped, view.failed),
        (3, 3, 2, 0)
    );
    assert!(view.error.unwrap().contains("延期"));
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
    assert!(!saved.to_string().contains("http://") && !saved.to_string().contains("/rate"));
    assert!(scheduler.shutdown().await);
    let restarted = Scheduler::with_clock(
        path.clone(),
        NetworkClients::new(&path),
        |_| {},
        limits,
        clock.clone(),
    );
    assert!(matches!(
        fetch(&restarted, 2).await.status,
        RefreshStatus::Skipped { .. }
    ));
    let restored = snapshot(&conn);
    assert!(restored["blocks"][0]["blocked_until"].as_i64().unwrap() >= until);
    let view = operation(&restarted).await;
    assert_eq!((view.skipped, view.failed), (2, 0));
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
    assert!(restarted.shutdown().await);
}

#[tokio::test]
async fn deferred_deadline_writes_do_not_block_other_hosts_from_dispatching() {
    let healthy = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(304))
        .expect(1)
        .mount(&healthy)
        .await;
    let (_dir, path) = fixture();
    let conn = db::open_at(&path).unwrap();
    for id in 1..=64 {
        seed(&conn, id, format!("https://cooled.invalid/{id}"));
    }
    seed(&conn, 65, healthy.uri());
    let scheduler = Scheduler::start(path.clone(), NetworkClients::new(&path), |_| {});
    scheduler.shared.await_cooldown_ready().await.unwrap();
    let feed = rrss_core::feeds::get_feed(&conn, 1).unwrap();
    let url = reqwest::Url::parse(&feed.url).unwrap();
    scheduler
        .shared
        .response_head(
            feed,
            1,
            rrss_core::ingest::feed_http::ResponseHead {
                url: url.clone(),
                visited: vec![url],
                status: 429,
                retry_after: Some("600".into()),
            },
        )
        .await
        .unwrap();
    // Hold the SQLite writer after the authoritative cooldown has committed.
    // Its derived per-feed deadlines may wait; an unrelated GET must not.
    let writer = db::write_transaction(&conn).unwrap();
    let tickets = (1..=65)
        .map(|id| scheduler.submit(id).unwrap())
        .collect::<Vec<_>>();
    let dispatched = tokio::time::timeout(Duration::from_secs(5), async {
        while healthy.received_requests().await.unwrap().is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .is_ok();
    drop(writer);
    for ticket in tickets {
        tokio::time::timeout(Duration::from_secs(20), ticket.wait())
            .await
            .unwrap()
            .unwrap();
    }
    assert!(scheduler.shutdown().await);
    assert!(
        dispatched,
        "deadline bookkeeping blocked an unrelated download"
    );
}

struct StatusReply(Arc<AtomicUsize>);
impl wiremock::Respond for StatusReply {
    fn respond(&self, _: &wiremock::Request) -> ResponseTemplate {
        ResponseTemplate::new(self.0.load(Ordering::Acquire) as u16)
    }
}

#[tokio::test]
async fn missing_retry_after_uses_host_staircase_and_fast_response_resets_the_tier() {
    let server = MockServer::start().await;
    let status = Arc::new(AtomicUsize::new(429));
    Mock::given(method("GET"))
        .respond_with(StatusReply(status.clone()))
        .mount(&server)
        .await;
    let (_dir, path) = fixture();
    let conn = db::open_at(&path).unwrap();
    seed(&conn, 1, server.uri());
    let clock = TestClock::new();
    let scheduler = Scheduler::with_clock(
        path.clone(),
        NetworkClients::new(&path),
        |_| {},
        runtime::Limits::default(),
        clock.clone(),
    );
    for (code, hours) in [(429, 1), (503, 2), (429, 4), (503, 4)] {
        status.store(code, Ordering::Release);
        let result = fetch(&scheduler, 1).await;
        let RefreshStatus::Skipped {
            retry_at: Some(until),
            ..
        } = result.status
        else {
            panic!("expected cooldown")
        };
        assert_eq!(until - clock.now().utc, hours * 3_600_000);
        clock.advance((hours * 3_600_000 + 1000) as u64);
        scheduler.shared.wake.notify_one();
        wait_until(|| scheduler.shared.deferred(1, &server.uri()).is_none()).await;
    }
    status.store(500, Ordering::Release);
    assert!(matches!(
        fetch(&scheduler, 1).await.status,
        RefreshStatus::Failed { .. }
    ));
    status.store(429, Ordering::Release);
    let RefreshStatus::Skipped {
        retry_at: Some(until),
        ..
    } = fetch(&scheduler, 1).await.status
    else {
        panic!("expected cooldown")
    };
    assert_eq!(until - clock.now().utc, 3_600_000);
    assert!(scheduler.shutdown().await);
}

#[tokio::test]
async fn cooled_due_sources_stay_parked_in_memory_then_resume_without_rescanning() {
    const PEERS: i64 = 16;
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(request_path("/rate"))
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "7200"))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(wiremock::matchers::path_regex("^/peer/"))
        .respond_with(ResponseTemplate::new(304))
        .mount(&server)
        .await;
    let (_dir, path) = fixture();
    let conn = db::open_at(&path).unwrap();
    seed(&conn, 1, format!("{}/rate", server.uri()));
    for id in 2..=PEERS + 1 {
        seed(&conn, id, format!("{}/peer/{id}", server.uri()));
    }
    let clock = TestClock::new();
    let scheduler = Scheduler::with_clock(
        path.clone(),
        NetworkClients::new(&path),
        |_| {},
        runtime::Limits {
            global: 4,
            host: 1,
            ..Default::default()
        },
        clock.clone(),
    );
    fetch(&scheduler, 1).await;
    let mut tickets = Vec::new();
    for id in 2..=PEERS + 1 {
        tickets.push(scheduler.submit_wait(id, true).await.unwrap());
    }
    for ticket in tickets {
        assert!(matches!(
            ticket.wait().await.unwrap().status,
            RefreshStatus::Skipped { .. }
        ));
    }
    let lookups = scheduler.lookups();
    for _ in 0..10 {
        scheduler.shared.wake.notify_one();
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(
        scheduler.lookups(),
        lookups,
        "cooled hosts must not re-read all parked rows on wake"
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
    assert_eq!(conn.query_row("SELECT count(*) FROM feeds WHERE id>1 AND next_fetch_at>=?1 AND fail_count=0 AND slow_fail_count=0 AND last_fetched_at IS NULL",[clock.now().utc+7_200_000],|r|r.get::<_,i64>(0)).unwrap(),PEERS);
    let view = operation(&scheduler).await;
    assert_eq!((view.skipped, view.failed), (PEERS as usize + 1, 0));
    clock.advance(7_201_000);
    scheduler.shared.wake.notify_one();
    wait_until(|| {
        conn.query_row(
            "SELECT count(*) FROM feeds WHERE id>1 AND last_success_at IS NOT NULL",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
            == PEERS
    })
    .await;
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        PEERS as usize + 1
    );
    assert!(
        scheduler.lookups() <= 3 * (PEERS as usize + 1),
        "too many metadata rows read after cooldown expiry: {}",
        scheduler.lookups()
    );
    assert!(scheduler.shutdown().await);
}

#[tokio::test]
async fn cooled_backlog_larger_than_queue_does_not_block_other_due_hosts() {
    let limited = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "86400"))
        .mount(&limited)
        .await;
    let healthy = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(304))
        .mount(&healthy)
        .await;
    let (_dir, path) = fixture();
    let conn = db::open_at(&path).unwrap();
    for id in 1..=5 {
        seed(&conn, id, format!("{}/{id}", limited.uri()));
    }
    seed(&conn, 6, healthy.uri());
    let scheduler = Scheduler::with_limits(
        path.clone(),
        NetworkClients::new(&path),
        |_| {},
        runtime::Limits {
            pending: 2,
            host: 1,
            ..Default::default()
        },
    );
    fetch(&scheduler, 1).await;
    let results = tokio::time::timeout(Duration::from_secs(5), scheduler.enqueue_due())
        .await
        .expect("cooled sources must leave room for later pages")
        .unwrap();
    assert_eq!(results, 5);
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let feed = rrss_core::feeds::get_feed(&conn, 6).unwrap();
            if feed.last_fetched_at.is_some() && feed.error.is_none() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("healthy host must finish despite cooled backlog");
    assert_eq!(limited.received_requests().await.unwrap().len(), 1);
    assert_eq!(healthy.received_requests().await.unwrap().len(), 1);
    assert!(scheduler.shutdown().await);
}

#[tokio::test]
async fn clock_rollback_and_large_forward_jumps_do_not_shorten_saved_cooldowns() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "86400"))
        .mount(&server)
        .await;
    let (_dir, path) = fixture();
    let conn = db::open_at(&path).unwrap();
    seed(&conn, 1, server.uri());
    let clock = TestClock::new();
    let scheduler = Scheduler::with_clock(
        path.clone(),
        NetworkClients::new(&path),
        |_| {},
        runtime::Limits::default(),
        clock.clone(),
    );
    fetch(&scheduler, 1).await;
    let original = snapshot(&conn)["blocks"][0]["blocked_until"]
        .as_i64()
        .unwrap();
    clock.jump(-86_400_000);
    super::super::cooldown::checkpoint(scheduler.shared.clone()).await;
    assert!(
        snapshot(&conn)["blocks"][0]["blocked_until"]
            .as_i64()
            .unwrap()
            >= original
    );
    assert_eq!(operation(&scheduler).await.skipped, 1);
    clock.jump(10 * 86_400_000);
    super::super::cooldown::checkpoint(scheduler.shared.clone()).await;
    let advanced = snapshot(&conn)["blocks"][0]["blocked_until"]
        .as_i64()
        .unwrap();
    assert!(advanced >= clock.now().utc + 86_400_000 - 1000 && advanced >= original);
    assert!(scheduler.shutdown().await);
    let restarted = Scheduler::with_clock(
        path.clone(),
        NetworkClients::new(&path),
        |_| {},
        runtime::Limits::default(),
        clock.clone(),
    );
    assert_eq!(operation(&restarted).await.skipped, 1);
    assert!(
        snapshot(&conn)["blocks"][0]["blocked_until"]
            .as_i64()
            .unwrap()
            >= advanced
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
    assert!(restarted.shutdown().await);
}

#[tokio::test]
async fn redirected_target_cooldown_is_persisted_for_known_sources_and_address_changes() {
    let origin = MockServer::start().await;
    let target = MockServer::start().await;
    Mock::given(method("GET"))
        .and(request_path("/rate"))
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "3600"))
        .mount(&target)
        .await;
    Mock::given(method("GET"))
        .and(request_path("/peer"))
        .respond_with(ResponseTemplate::new(304))
        .mount(&target)
        .await;
    for (source, destination) in [("/first", "/rate"), ("/second", "/peer")] {
        Mock::given(method("GET"))
            .and(request_path(source))
            .respond_with(
                ResponseTemplate::new(302)
                    .insert_header("Location", format!("{}{destination}", target.uri())),
            )
            .mount(&origin)
            .await;
    }
    Mock::given(method("GET"))
        .and(request_path("/independent"))
        .respond_with(ResponseTemplate::new(304))
        .mount(&origin)
        .await;
    let (_dir, path) = fixture();
    let conn = db::open_at(&path).unwrap();
    seed(&conn, 1, format!("{}/first", origin.uri()));
    seed(&conn, 2, format!("{}/second", origin.uri()));
    seed(&conn, 3, format!("{}/direct", target.uri()));
    let clock = TestClock::new();
    let scheduler = Scheduler::with_clock(
        path.clone(),
        NetworkClients::new(&path),
        |_| {},
        runtime::Limits::default(),
        clock.clone(),
    );
    assert_eq!(
        fetch(&scheduler, 2).await.status,
        RefreshStatus::NotModified
    );
    assert!(matches!(
        fetch(&scheduler, 1).await.status,
        RefreshStatus::Skipped { .. }
    ));
    assert_eq!(snapshot(&conn)["routes"].as_array().unwrap().len(), 2);
    assert_eq!(operation(&scheduler).await.skipped, 3);
    assert_eq!(origin.received_requests().await.unwrap().len(), 2);
    assert!(scheduler.shutdown().await);
    let restarted = Scheduler::with_clock(
        path.clone(),
        NetworkClients::new(&path),
        |_| {},
        runtime::Limits::default(),
        clock,
    );
    assert_eq!(operation(&restarted).await.skipped, 3);
    assert_eq!(origin.received_requests().await.unwrap().len(), 2);
    conn.execute(
        "UPDATE feeds SET url=?1 WHERE id=2",
        [format!("{}/independent", origin.uri())],
    )
    .unwrap();
    assert_eq!(
        fetch(&restarted, 2).await.status,
        RefreshStatus::NotModified
    );
    assert_eq!(target.received_requests().await.unwrap().len(), 2);
    assert!(restarted.shutdown().await);
}

#[tokio::test]
async fn cancelled_header_wait_still_commits_before_a_restarted_runtime_can_dispatch() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(request_path("/rate"))
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "86400"))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(request_path("/peer"))
        .respond_with(ResponseTemplate::new(304))
        .mount(&server)
        .await;
    let (_dir, path) = fixture();
    let conn = db::open_at(&path).unwrap();
    seed(&conn, 1, format!("{}/rate", server.uri()));
    seed(&conn, 2, format!("{}/peer", server.uri()));
    let mut limits = runtime::Limits {
        host: 1,
        ..Default::default()
    };
    limits.policy.timeout = Duration::from_secs(1);
    let scheduler =
        Scheduler::with_limits(path.clone(), NetworkClients::new(&path), |_| {}, limits);
    scheduler.shared.await_cooldown_ready().await.unwrap();
    scheduler.shared.network.feed_client().await.unwrap();
    conn.execute_batch("BEGIN IMMEDIATE").unwrap();
    let ticket = scheduler.submit(1).unwrap();
    wait_until(|| {
        scheduler
            .shared
            .cooldown_store
            .writes
            .load(Ordering::Acquire)
            > 0
    })
    .await;
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(10), ticket.wait())
            .await
            .unwrap()
            .unwrap()
            .status,
        RefreshStatus::Skipped { .. }
    ));
    scheduler.stop();
    let restarted =
        Scheduler::with_limits(path.clone(), NetworkClients::new(&path), |_| {}, limits);
    let peer = restarted.submit(2).unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
    conn.execute_batch("COMMIT").unwrap();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(10), peer.wait())
            .await
            .unwrap()
            .unwrap()
            .status,
        RefreshStatus::Skipped { .. }
    ));
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
    assert_eq!(snapshot(&conn)["blocks"].as_array().unwrap().len(), 1);
    assert_eq!(
        conn.query_row("SELECT fail_count FROM feeds WHERE id=1", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert!(restarted.shutdown().await);
}

#[tokio::test]
async fn damaged_snapshot_is_visible_and_persisted_recovery_admits_only_one_request_per_gap() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(304))
        .mount(&server)
        .await;
    let (_dir, path) = fixture();
    let conn = db::open_at(&path).unwrap();
    for id in 1..=3 {
        seed(&conn, id, format!("{}/{id}", server.uri()));
    }
    db::set_setting(&conn, SETTING, "{broken").unwrap();
    let clock = TestClock::new();
    let scheduler = Scheduler::with_clock(
        path.clone(),
        NetworkClients::new(&path),
        |_| {},
        runtime::Limits::default(),
        clock.clone(),
    );
    scheduler.shared.await_cooldown_ready().await.unwrap();
    assert!(
        scheduler
            .snapshot()
            .feed_pause
            .unwrap()
            .contains("限流记录")
    );
    assert_eq!(operation(&scheduler).await.skipped, 3);
    assert_eq!(snapshot(&conn)["recovery"], true);
    for id in 1..=3 {
        scheduler.submit_wait(id, true).await.unwrap();
    }
    wait_until(|| {
        conn.query_row(
            "SELECT count(*) FROM feeds WHERE last_fetched_at IS NOT NULL",
            [],
            |r| r.get::<_, usize>(0),
        )
        .unwrap()
            == 1
    })
    .await;
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
    clock.advance(11_000);
    scheduler.shared.wake.notify_one();
    wait_until(|| {
        conn.query_row(
            "SELECT count(*) FROM feeds WHERE last_fetched_at IS NOT NULL",
            [],
            |r| r.get::<_, usize>(0),
        )
        .unwrap()
            == 2
    })
    .await;
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
    clock.advance(11_000);
    scheduler.shared.wake.notify_one();
    wait_until(|| {
        conn.query_row(
            "SELECT count(*) FROM feeds WHERE last_fetched_at IS NOT NULL",
            [],
            |r| r.get::<_, usize>(0),
        )
        .unwrap()
            == 3
    })
    .await;
    assert_eq!(server.received_requests().await.unwrap().len(), 3);
    assert!(scheduler.shutdown().await);
}

#[tokio::test]
async fn recovery_keeps_serial_admission_across_a_slow_redirect() {
    let (target, mut target_arrived, release_target, target_task) = gated_server().await;
    let (peer, mut peer_arrived, release_peer, peer_task) = gated_server().await;
    let origin = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(302).insert_header("Location", target))
        .mount(&origin)
        .await;
    let (_dir, path) = fixture();
    let conn = db::open_at(&path).unwrap();
    seed(&conn, 1, origin.uri());
    seed(&conn, 2, peer);
    db::set_setting(&conn, SETTING, "{broken").unwrap();
    let clock = TestClock::new();
    let scheduler = Scheduler::with_clock(
        path.clone(),
        NetworkClients::new(&path),
        |_| {},
        runtime::Limits::default(),
        clock.clone(),
    );
    let first = scheduler.submit_wait(1, true).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), target_arrived.recv())
        .await
        .unwrap()
        .unwrap();
    clock.advance(11_000);
    let second = scheduler.submit_wait(2, true).await.unwrap();
    tokio::select! {
        result = second.wait() => assert!(matches!(result.unwrap().status, RefreshStatus::Skipped { .. })),
        _ = peer_arrived.recv() => panic!("recovery must remain serial while a redirected body is in flight"),
        _ = tokio::time::sleep(Duration::from_secs(5)) => panic!("recovery did not report the deferred source"),
    }
    release_target.add_permits(1);
    first.wait().await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), peer_arrived.recv())
        .await
        .unwrap()
        .unwrap();
    let second = scheduler.submit(2).unwrap();
    release_peer.add_permits(1);
    second.wait().await.unwrap();
    assert!(scheduler.shutdown().await);
    target_task.abort();
    peer_task.abort();
}

#[tokio::test]
async fn failed_cooldown_snapshot_write_is_visible_and_prevents_peer_dispatch() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(503).insert_header("Retry-After", "3600"))
        .mount(&server)
        .await;
    let (_dir, path) = fixture();
    let conn = db::open_at(&path).unwrap();
    seed(&conn, 1, format!("{}/rate", server.uri()));
    seed(&conn, 2, format!("{}/peer", server.uri()));
    let scheduler = Scheduler::start(path.clone(), NetworkClients::new(&path), |_| {});
    scheduler.shared.await_cooldown_ready().await.unwrap();
    conn.execute_batch(&format!("CREATE TRIGGER reject_cooldown BEFORE INSERT ON settings WHEN NEW.key='{SETTING}' BEGIN SELECT RAISE(ABORT,'injected'); END;")).unwrap();
    assert!(matches!(
        fetch(&scheduler, 1).await.status,
        RefreshStatus::Skipped { .. }
    ));
    assert!(
        scheduler
            .snapshot()
            .feed_pause
            .unwrap()
            .contains("暂停抓取")
    );
    assert_eq!(operation(&scheduler).await.skipped, 2);
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
    conn.execute_batch("DROP TRIGGER reject_cooldown").unwrap();
    super::super::cooldown::checkpoint(scheduler.shared.clone()).await;
    assert!(scheduler.snapshot().feed_pause.is_none());
    assert_eq!(operation(&scheduler).await.skipped, 2);
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
    assert!(scheduler.shutdown().await);
}

#[tokio::test]
async fn restart_resumes_from_the_saved_deadline_so_downtime_counts_toward_the_wait() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "3600"))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(304))
        .mount(&server)
        .await;
    let (_dir, path) = fixture();
    let conn = db::open_at(&path).unwrap();
    seed(&conn, 1, server.uri());
    let clock = TestClock::new();
    let start = |clock: &Arc<TestClock>| {
        Scheduler::with_clock(
            path.clone(),
            NetworkClients::new(&path),
            |_| {},
            runtime::Limits::default(),
            clock.clone(),
        )
    };
    let scheduler = start(&clock);
    assert!(matches!(
        fetch(&scheduler, 1).await.status,
        RefreshStatus::Skipped { .. }
    ));
    assert!(scheduler.shutdown().await);
    // Stopped for longer than the server asked: no extra wait after restart.
    clock.advance(3_601_000);
    let restarted = start(&clock);
    assert_eq!(
        fetch(&restarted, 1).await.status,
        RefreshStatus::NotModified
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
    assert!(restarted.shutdown().await);
}

#[tokio::test]
async fn damaged_snapshot_recovery_ends_after_its_limit_and_stays_ended_after_restart() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(304))
        .mount(&server)
        .await;
    let (_dir, path) = fixture();
    let conn = db::open_at(&path).unwrap();
    for id in 1..=3 {
        seed(&conn, id, format!("{}/{id}", server.uri()));
    }
    db::set_setting(&conn, SETTING, "{broken").unwrap();
    let clock = TestClock::new();
    let start = |clock: &Arc<TestClock>| {
        Scheduler::with_clock(
            path.clone(),
            NetworkClients::new(&path),
            |_| {},
            runtime::Limits::default(),
            clock.clone(),
        )
    };
    let scheduler = start(&clock);
    scheduler.shared.await_cooldown_ready().await.unwrap();
    assert_eq!(snapshot(&conn)["recovery"], true);
    // Halfway through, a restart keeps the remaining throttling window.
    clock.advance(2 * 3_600_000);
    assert!(scheduler.shutdown().await);
    let scheduler = start(&clock);
    scheduler.shared.await_cooldown_ready().await.unwrap();
    assert!(scheduler.snapshot().feed_pause.is_some());
    clock.advance(2 * 3_600_000 + 1000);
    scheduler.shared.wake.notify_one();
    wait_until(|| snapshot(&conn)["recovery"] == false).await;
    wait_until(|| scheduler.snapshot().feed_pause.is_none()).await;
    // Unthrottled: all sources go out without advancing the clock.
    for id in 1..=3 {
        scheduler.submit_wait(id, true).await.unwrap();
    }
    wait_until(|| {
        conn.query_row(
            "SELECT count(*) FROM feeds WHERE last_fetched_at IS NOT NULL",
            [],
            |r| r.get::<_, usize>(0),
        )
        .unwrap()
            == 3
    })
    .await;
    assert!(scheduler.shutdown().await);
    let restarted = start(&clock);
    restarted.shared.await_cooldown_ready().await.unwrap();
    assert!(restarted.snapshot().feed_pause.is_none());
    assert!(restarted.shutdown().await);
}
