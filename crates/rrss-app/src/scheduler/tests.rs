use super::*;
use tokio::sync::mpsc;
use wiremock::{Mock, MockServer, ResponseTemplate, matchers::method};

mod cooldown;
mod offline;
mod polling;
mod scale;

pub(super) fn capture_logs(run: impl FnOnce()) -> String {
    let dir = tempfile::tempdir().unwrap();
    let (subscriber, handle) = crate::log::subscriber(
        crate::log::Mode::Desktop {
            log_dir: dir.path().to_owned(),
        },
        "rrss=debug",
    );
    tracing::subscriber::with_default(subscriber, run);
    drop(handle);
    std::fs::read_to_string(dir.path().join("rrss.log")).unwrap()
}

const RSS: &str = "<rss version='2.0'><channel><title>Mock</title><link>https://f.test</link><description>mock</description><item><guid>a</guid><title>Article</title><description>body</description></item></channel></rss>";
fn rss_response() -> Vec<u8> {
    format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nETag: buffered-version\r\nConnection: close\r\n\r\n{RSS}",
        RSS.len()
    ).into_bytes()
}
pub(super) fn fixture() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.db");
    let conn = db::open_at(&path).unwrap();
    db::set_setting(&conn, rrss_core::feed_icon::SETTING_KEY, "0").unwrap();
    rrss_core::ai_store::set_daily_budget_tokens(&conn, 1).unwrap();
    rrss_core::ai_store::add_usage(&conn, &rrss_core::ai_store::local_day(), 1, 0).unwrap();
    (dir, path)
}

#[tokio::test]
async fn add_returns_before_fetch_and_duplicate_never_requests_and_failure_keeps_feed() {
    for status in [200, 304, 500] {
        let reply = format!(
            "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{RSS}",
            RSS.len()
        )
        .into_bytes();
        let (url, mut arrived, release, task) = gated_response(reply).await;
        let (_dir, path) = fixture();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let scheduler = Scheduler::start(path.clone(), NetworkClients::new(&path), move |event| {
            tx.send(event).ok();
        });
        let added = tokio::time::timeout(Duration::from_secs(10), scheduler.add_feed(&url))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(added.message, "已添加，正在后台抓取");
        assert!(
            scheduler
                .add_feed(&url)
                .await
                .unwrap_err()
                .downcast_ref::<rrss_core::Error>()
                .is_some_and(|e| matches!(e, rrss_core::Error::DuplicateFeed(_)))
        );
        let first = scheduler.submit(added.id).unwrap();
        let second = scheduler.submit(added.id).unwrap();
        arrived.recv().await.unwrap();
        release.add_permits(1);
        let first = first.wait().await.unwrap();
        assert_eq!(first.status, second.wait().await.unwrap().status);
        let event = loop {
            let event = rx.recv().await.unwrap();
            if event.feed_id == Some(added.id) {
                break event;
            }
        };
        assert_eq!(event.feed_id, Some(added.id));
        assert_eq!(event.run_id, scheduler.snapshot().run_id);
        assert!(event.generation.is_some_and(|generation| generation > 0));
        assert_eq!(
            event.status,
            match status {
                200 => "updated",
                304 => "not_modified",
                _ => "failed",
            }
        );
        assert!(!event.derived_degraded);
        assert_eq!(event.error.is_some(), status == 500);
        let conn = db::open_at(&path).unwrap();
        assert!(rrss_core::feeds::get_feed(&conn, added.id).is_ok());
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM articles", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            if status == 200 { 1 } else { 0 }
        );
        assert!(arrived.try_recv().is_err());
        task.abort();
    }
}

#[tokio::test]
async fn timeout_releases_slot_single_add_has_reserved_admission_and_stop_is_explicit() {
    let slow = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(RSS)
                .set_delay(Duration::from_secs(5)),
        )
        .mount(&slow)
        .await;
    let (fast, mut arrived, release, server) = gated_response(rss_response()).await;
    let (_dir, path) = fixture();
    let mut limits = runtime::Limits::default();
    limits.pending = 1;
    limits.policy.timeout = Duration::from_secs(1);
    let scheduler =
        Scheduler::with_limits(path.clone(), NetworkClients::new(&path), |_| {}, limits);
    let a = scheduler.add_feed(&slow.uri()).await.unwrap();
    let ticket = scheduler.submit(a.id).unwrap();
    let b = scheduler.add_feed(&fast).await.unwrap();
    assert_eq!(b.message, "已添加，正在后台抓取");
    let b_ticket = scheduler.submit(b.id).unwrap();
    tokio::time::timeout(Duration::from_secs(10), arrived.recv())
        .await
        .unwrap()
        .unwrap();
    release.add_permits(1);
    assert!(matches!(
        b_ticket.wait().await.unwrap().status,
        RefreshStatus::Updated { inserted: 1, .. }
    ));
    assert!(matches!(
        ticket.wait().await.unwrap().status,
        RefreshStatus::Failed { .. }
    ));
    scheduler.stop();
    let c = scheduler.add_feed(&format!("{fast}/next")).await.unwrap();
    assert!(c.message.contains("运行时已停止"));
    assert!(rrss_core::feeds::get_feed(&db::open_at(&path).unwrap(), c.id).is_ok());
    server.abort();
}

#[test]
fn normalized_host_identity_includes_default_port_and_ipv6() {
    let key = |s| runtime::host_key(&reqwest::Url::parse(s).unwrap());
    assert_eq!(
        key("http://EXAMPLE.test:80/a"),
        key("http://example.test/b")
    );
    assert_ne!(key("http://example.test"), key("https://example.test"));
    assert_eq!(key("http://[::1]:80/a"), key("http://[::1]/b"));
}

async fn gated_server() -> (
    String,
    mpsc::UnboundedReceiver<()>,
    Arc<tokio::sync::Semaphore>,
    tokio::task::JoinHandle<()>,
) {
    gated_response(
        b"HTTP/1.1 304 Not Modified\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
    )
    .await
}
async fn gated_response(
    reply: Vec<u8>,
) -> (
    String,
    mpsc::UnboundedReceiver<()>,
    Arc<tokio::sync::Semaphore>,
    tokio::task::JoinHandle<()>,
) {
    controlled_response(Vec::new(), reply).await
}
async fn controlled_response(
    initial: Vec<u8>,
    reply: Vec<u8>,
) -> (
    String,
    mpsc::UnboundedReceiver<()>,
    Arc<tokio::sync::Semaphore>,
    tokio::task::JoinHandle<()>,
) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (tx, rx) = mpsc::unbounded_channel();
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let release = gate.clone();
    let task = tokio::spawn(async move {
        let mut requests = tokio::task::JoinSet::new();
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let tx = tx.clone();
            let gate = gate.clone();
            let initial = initial.clone();
            let reply = reply.clone();
            requests.spawn(async move {
                let mut head = Vec::new();
                while !head.windows(4).any(|tail| tail == b"\r\n\r\n") {
                    let mut chunk = [0; 1024];
                    let read = socket.read(&mut chunk).await.unwrap();
                    if read == 0 {
                        return;
                    }
                    head.extend_from_slice(&chunk[..read]);
                    assert!(head.len() <= 16 * 1024, "fixture request headers too large");
                }
                tx.send(()).unwrap();
                socket.write_all(&initial).await.ok();
                gate.acquire().await.unwrap().forget();
                socket.write_all(&reply).await.ok();
            });
        }
    });
    (url, rx, release, task)
}

#[tokio::test]
async fn overlapping_entries_share_limits_and_busy_host_does_not_hide_other_hosts() {
    let (a, mut got_a, release_a, task_a) = gated_server().await;
    let (b, mut got_b, release_b, task_b) = gated_server().await;
    let (_dir, path) = fixture();
    let limits = runtime::Limits {
        global: 2,
        host: 1,
        parsing: 2,
        ..runtime::Limits::default()
    };
    let scheduler =
        Scheduler::with_limits(path.clone(), NetworkClients::new(&path), |_| {}, limits);
    let one = scheduler.add_feed(&format!("{a}/1")).await.unwrap();
    got_a.recv().await.unwrap();
    let two = scheduler.add_feed(&format!("{a}/2")).await.unwrap();
    let three = scheduler.add_feed(&format!("{b}/1")).await.unwrap();
    tokio::time::timeout(Duration::from_secs(10), got_b.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        got_a.try_recv().is_err(),
        "second same-host request must wait"
    );
    let one_ticket = scheduler.submit(one.id).unwrap();
    let two_ticket = scheduler.submit(two.id).unwrap();
    let three_ticket = scheduler.submit(three.id).unwrap();
    let operation = scheduler.start_refresh().await.unwrap();
    release_a.add_permits(2);
    release_b.add_permits(1);
    one_ticket.wait().await.unwrap();
    two_ticket.wait().await.unwrap();
    three_ticket.wait().await.unwrap();
    let result = wait_operation(&scheduler, operation.operation_id).await;
    assert_eq!(result.completed, 3);
    assert!(
        got_b.try_recv().is_err(),
        "overlapping refresh reuses the running request"
    );
    assert!(got_a.try_recv().is_ok());
    assert!(got_a.try_recv().is_err());
    task_a.abort();
    task_b.abort();
}

#[tokio::test]
async fn slow_downloads_leave_parse_slots_free_and_rss_capacity_is_separate_from_media() {
    let (slow_a, mut arrived_a, _release_a, server_a) = gated_server().await;
    let (slow_b, mut arrived_b, _release_b, server_b) = gated_server().await;
    let (fast, mut arrived_fast, release_fast, server_fast) = gated_response(rss_response()).await;
    let (_dir, path) = fixture();
    let scheduler = Scheduler::with_limits(
        path.clone(),
        NetworkClients::new(&path),
        |_| {},
        runtime::Limits {
            global: 3,
            host: 1,
            parsing: 2,
            ..runtime::Limits::default()
        },
    );
    scheduler.add_feed(&slow_a).await.unwrap();
    scheduler.add_feed(&slow_b).await.unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        arrived_a.recv().await.unwrap();
        arrived_b.recv().await.unwrap();
    })
    .await
    .unwrap();

    // RSS must leave the full media allowance available, and media using that
    // allowance must not prevent another RSS request from downloading/committing.
    let media = tokio::time::timeout(Duration::from_secs(10), async {
        let mut media = Vec::new();
        for _ in 0..8 {
            media.push(
                scheduler
                    .shared
                    .capacity
                    .content
                    .clone()
                    .acquire(model_capacity::Priority::Background)
                    .await
                    .unwrap(),
            );
        }
        media
    })
    .await
    .expect("RSS must not consume media capacity");
    let added = scheduler.add_feed(&fast).await.unwrap();
    let ticket = scheduler.submit(added.id).unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        arrived_fast.recv().await.unwrap();
        release_fast.add_permits(1);
        assert!(matches!(
            ticket.wait().await.unwrap().status,
            RefreshStatus::Updated { inserted: 1, .. }
        ));
    })
    .await
    .unwrap();
    assert_eq!(
        db::open_at(&path)
            .unwrap()
            .query_row("SELECT COUNT(*) FROM articles", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    drop(media);
    assert!(scheduler.shutdown().await);
    server_a.abort();
    server_b.abort();
    server_fast.abort();
}

#[tokio::test]
async fn per_host_download_limit_follows_the_setting() {
    let (host, mut got, release, server) = gated_server().await;
    let (_dir, path) = fixture();
    let conn = db::open_at(&path).unwrap();
    db::set_setting(&conn, db::FEED_FETCH_PER_HOST_KEY, "3").unwrap();
    for id in 1..=12 {
        conn.execute(
            "INSERT INTO feeds(id,url,created_at) VALUES(?1,?2,0)",
            rusqlite::params![id, format!("{host}/{id}")],
        )
        .unwrap();
    }
    let scheduler = Scheduler::start(path.clone(), NetworkClients::new(&path), |_| {});
    scheduler.start_refresh().await.unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        for _ in 0..3 {
            got.recv().await.unwrap();
        }
    })
    .await
    .unwrap();
    // 三个请求都挂着时，同站第四个不会发出。
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        got.try_recv().is_err(),
        "the fourth same-host download waits"
    );
    assert_eq!(scheduler.diagnostics().feeds.host_limit, 3);
    release.add_permits(12);
    assert!(scheduler.shutdown().await);
    server.abort();
}

#[tokio::test]
async fn many_slow_sources_do_not_hide_other_hosts_across_metadata_pages() {
    let (slow_a, mut got_a, _release_a, server_a) = gated_server().await;
    let (slow_b, mut got_b, _release_b, server_b) = gated_server().await;
    let (fast, mut got_fast, release_fast, server_fast) = gated_response(rss_response()).await;
    let (_dir, path) = fixture();
    let conn = db::open_at(&path).unwrap();
    for id in 1..=256 {
        let host = if id <= 128 { &slow_a } else { &slow_b };
        conn.execute(
            "INSERT INTO feeds(id,url,created_at) VALUES(?1,?2,0)",
            rusqlite::params![id, format!("{host}/{id}")],
        )
        .unwrap();
    }
    conn.execute(
        "INSERT INTO feeds(id,url,created_at) VALUES(257,?1,0)",
        [fast],
    )
    .unwrap();
    let scheduler = Scheduler::start(path.clone(), NetworkClients::new(&path), |_| {});
    scheduler.start_refresh().await.unwrap();
    let ticket = scheduler.submit(257).unwrap();
    tokio::time::timeout(Duration::from_secs(20), async {
        // These requests stay in flight. Their queued same-host peers must not
        // prevent metadata pages from reaching the last, independent source.
        for _ in 0..scheduler.shared.host_limit() {
            got_a.recv().await.unwrap();
            got_b.recv().await.unwrap();
        }
        got_fast.recv().await.unwrap();
        release_fast.add_permits(1);
        assert!(matches!(
            ticket.wait().await.unwrap().status,
            RefreshStatus::Updated { inserted: 1, .. }
        ));
    })
    .await
    .unwrap();
    let operation = scheduler.snapshot().operations.remove(0);
    assert_eq!(
        (operation.target, operation.completed, operation.inserted),
        (257, 1, 1)
    );
    assert!(
        got_a.try_recv().is_err(),
        "same-host downloads stay bounded"
    );
    assert!(
        got_b.try_recv().is_err(),
        "same-host downloads stay bounded"
    );
    assert!(scheduler.shutdown().await);
    server_a.abort();
    server_b.abort();
    server_fast.abort();
}

#[tokio::test]
async fn same_host_backlog_waits_in_memory_instead_of_rereading_metadata() {
    const FEEDS: i64 = 48;
    let (host, _got, release, server) = gated_server().await;
    let (_dir, path) = fixture();
    let conn = db::open_at(&path).unwrap();
    for id in 1..=FEEDS {
        conn.execute(
            "INSERT INTO feeds(id,url,created_at) VALUES(?1,?2,0)",
            rusqlite::params![id, format!("{host}/{id}")],
        )
        .unwrap();
    }
    let scheduler = Scheduler::with_limits(
        path.clone(),
        NetworkClients::new(&path),
        |_| {},
        runtime::Limits {
            global: 4,
            host: 1,
            ..runtime::Limits::default()
        },
    );
    let tickets: Vec<_> = (1..=FEEDS)
        .map(|id| scheduler.submit(id).unwrap())
        .collect();
    release.add_permits(FEEDS as usize);
    tokio::time::timeout(Duration::from_secs(30), async {
        for ticket in tickets {
            assert_eq!(
                ticket.wait().await.unwrap().status,
                RefreshStatus::NotModified
            );
        }
    })
    .await
    .unwrap();
    // Rescanning the backlog after every same-host completion would read about
    // FEEDS² / 2 rows; parked claims are read about once per dispatch attempt.
    let lookups = scheduler.lookups();
    assert!(
        lookups <= 3 * FEEDS as usize,
        "same-host backlog re-read {lookups} feed rows"
    );
    assert!(scheduler.shutdown().await);
    server.abort();
}

#[tokio::test]
async fn slow_sources_on_distinct_hosts_can_fill_global_download_capacity() {
    // S2 step 1 boundary: only per-host limits isolate slow sources. Slow sources
    // on more distinct hosts than the global limit still delay later sources until
    // one finishes; slow-failure backoff (step 2) keeps them from repeating.
    let (slow_a, mut got_a, release_a, server_a) = gated_server().await;
    let (slow_b, mut got_b, _release_b, server_b) = gated_server().await;
    let (fast, mut got_fast, release_fast, server_fast) = gated_response(rss_response()).await;
    let (_dir, path) = fixture();
    let conn = db::open_at(&path).unwrap();
    for (id, url) in [(1, &slow_a), (2, &slow_b), (3, &fast)] {
        conn.execute(
            "INSERT INTO feeds(id,url,created_at) VALUES(?1,?2,0)",
            rusqlite::params![id, url],
        )
        .unwrap();
    }
    let scheduler = Scheduler::with_limits(
        path.clone(),
        NetworkClients::new(&path),
        |_| {},
        runtime::Limits {
            global: 2,
            host: 2,
            ..runtime::Limits::default()
        },
    );
    let _slow = [scheduler.submit(1).unwrap(), scheduler.submit(2).unwrap()];
    tokio::time::timeout(Duration::from_secs(10), async {
        got_a.recv().await.unwrap();
        got_b.recv().await.unwrap();
    })
    .await
    .unwrap();
    let ticket = scheduler.submit(3).unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        got_fast.try_recv().is_err(),
        "distinct slow hosts hold every global download permit"
    );
    release_a.add_permits(1);
    tokio::time::timeout(Duration::from_secs(10), async {
        got_fast.recv().await.unwrap();
        release_fast.add_permits(1);
        assert!(matches!(
            ticket.wait().await.unwrap().status,
            RefreshStatus::Updated { inserted: 1, .. }
        ));
    })
    .await
    .unwrap();
    assert!(scheduler.shutdown().await);
    server_a.abort();
    server_b.abort();
    server_fast.abort();
}

#[tokio::test]
async fn buffered_responses_share_generations_and_recheck_deleted_or_changed_sources() {
    let (a, mut got_a, release_a, server_a) = gated_response(rss_response()).await;
    let (b, mut got_b, release_b, server_b) = gated_response(rss_response()).await;
    let (c, mut got_c, release_c, server_c) = gated_response(rss_response()).await;
    let (_dir, path) = fixture();
    let scheduler = Scheduler::with_limits(
        path.clone(),
        NetworkClients::new(&path),
        |_| {},
        runtime::Limits {
            global: 3,
            parsing: 1,
            ..runtime::Limits::default()
        },
    );
    let one = scheduler.add_feed(&a).await.unwrap();
    let two = scheduler.add_feed(&b).await.unwrap();
    let three = scheduler.add_feed(&c).await.unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        got_a.recv().await.unwrap();
        got_b.recv().await.unwrap();
        got_c.recv().await.unwrap();
    })
    .await
    .unwrap();
    scheduler.start_refresh().await.unwrap();
    let first = scheduler.submit(one.id).unwrap();
    let second = scheduler.submit(two.id).unwrap();
    let duplicate = scheduler.submit(two.id).unwrap();
    let third = scheduler.submit(three.id).unwrap();
    let conn = db::open_at(&path).unwrap();
    let transaction = db::write_transaction(&conn).unwrap();
    release_a.add_permits(1);
    wait_for_buffered_bytes(&scheduler, RSS.len()).await;
    release_b.add_permits(1);
    wait_for_buffered_bytes(&scheduler, RSS.len() * 2).await;
    release_c.add_permits(1);
    wait_for_buffered_bytes(&scheduler, RSS.len() * 3).await;
    assert_eq!(scheduler.snapshot().operations[0].completed, 0);
    assert_eq!(
        transaction
            .query_row("SELECT COUNT(*) FROM articles", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0,
        "a completed download does not complete a refresh before its write"
    );
    transaction
        .execute("DELETE FROM feeds WHERE id=?1", [two.id])
        .unwrap();
    transaction
        .execute(
            "UPDATE feeds SET url='https://changed.invalid/new' WHERE id=?1",
            [three.id],
        )
        .unwrap();
    transaction.commit().unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        assert!(matches!(
            first.wait().await.unwrap().status,
            RefreshStatus::Updated { inserted: 1, .. }
        ));
        let result = second.wait().await.unwrap();
        assert_eq!(result.status, RefreshStatus::Deleted);
        assert_eq!(duplicate.wait().await.unwrap().status, result.status);
        assert!(matches!(
            third.wait().await.unwrap().status,
            RefreshStatus::Failed { .. }
        ));
    })
    .await
    .unwrap();
    let operation = scheduler.snapshot().operations.remove(0);
    assert!(operation.done);
    assert_eq!(
        (
            operation.completed,
            operation.inserted,
            operation.deleted,
            operation.failed
        ),
        (3, 1, 1, 1)
    );
    let changed = rrss_core::feeds::get_feed(&conn, three.id).unwrap();
    assert!(changed.last_fetched_at.is_none());
    assert!(changed.etag.is_none());
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM articles WHERE feed_id=?1",
            [three.id],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    assert!(
        got_b.try_recv().is_err(),
        "a buffered source is not downloaded again for another waiter"
    );
    assert!(scheduler.shutdown().await);
    assert_eq!(scheduler.buffered_bytes(), 0);
    server_a.abort();
    server_b.abort();
    server_c.abort();
}

async fn wait_for_buffered_bytes(scheduler: &Scheduler, minimum: usize) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while scheduler.buffered_bytes() < minimum {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn single_added_feed_bypasses_backpressure_and_import_queue_with_one_body_lease() {
    let (first, mut got_first, release_first, server_first) = gated_response(rss_response()).await;
    let (added, mut got_added, release_added, server_added) = gated_response(rss_response()).await;
    let (_dir, path) = fixture();
    let conn = db::open_at(&path).unwrap();
    conn.execute(
        "INSERT INTO feeds(id,url,created_at) VALUES(1,?1,0)",
        [first],
    )
    .unwrap();
    let scheduler = Scheduler::with_limits(
        path.clone(),
        NetworkClients::new(&path),
        |_| {},
        runtime::Limits {
            pending: 1,
            parsing: 0,
            buffer_bytes: RSS.len(),
            ..runtime::Limits::default()
        },
    );
    let first = scheduler.submit(1).unwrap();
    got_first.recv().await.unwrap();
    release_first.add_permits(1);
    wait_for_buffered_bytes(&scheduler, RSS.len()).await;
    let added = scheduler.add_feed(&added).await.unwrap();
    let ticket = scheduler.submit(added.id).unwrap();
    tokio::time::timeout(Duration::from_secs(10), got_added.recv())
        .await
        .unwrap()
        .unwrap();
    release_added.add_permits(1);
    wait_for_buffered_bytes(&scheduler, RSS.len() * 2).await;
    assert_eq!(scheduler.bypass_available(), 0);
    // A second addition cannot accumulate another exceptional body while the
    // first one is still waiting for parsing/commit.
    conn.execute(
        "INSERT INTO feeds(url,created_at) VALUES(?1,0)",
        [format!(
            "{}/ordinary",
            rrss_core::feeds::get_feed(&conn, 1).unwrap().url
        )],
    )
    .unwrap();
    assert!(scheduler.submit(conn.last_insert_rowid()).is_err());
    assert_eq!(scheduler.buffered_bytes(), RSS.len() * 2);
    assert!(scheduler.shutdown().await);
    assert!(first.wait().await.is_err());
    assert!(ticket.wait().await.is_err());
    server_first.abort();
    server_added.abort();
}

#[tokio::test]
async fn import_larger_than_pending_limit_is_fully_admitted_in_due_pages() {
    const SOURCES: usize = 1100;
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(304))
        .mount(&server)
        .await;
    let (_dir, path) = fixture();
    let conn = db::open_at(&path).unwrap();
    let outlines = (0..SOURCES)
        .map(|id| format!("<outline text='source' xmlUrl='{}/{id}'/>", server.uri()))
        .collect::<String>();
    let xml = format!("<opml version='2.0'><head/><body>{outlines}</body></opml>");
    assert_eq!(
        crate::actions::import_opml(&conn, &xml).unwrap().imported,
        SOURCES as u32
    );
    assert!(SOURCES > runtime::Limits::default().pending);
    let scheduler = Scheduler::with_limits(
        path.clone(),
        NetworkClients::new(&path),
        |_| {},
        runtime::Limits {
            global: 4,
            pending: 8,
            ..runtime::Limits::default()
        },
    );
    let scan = scheduler.enqueue_due();
    tokio::pin!(scan);
    let results = tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            tokio::select! {
                result = &mut scan => break result.unwrap(),
                _ = tokio::task::yield_now() => {
                    assert!(scheduler.shared.queue.lock().unwrap().entries.len() <= 8);
                    assert!(scheduler.buffered_bytes() <= scheduler.shared.limits.buffer_bytes
                        + scheduler.shared.limits.global * scheduler.shared.limits.policy.max_body_bytes);
                }
            }
        }
    }).await.unwrap();
    assert_eq!(results, SOURCES);
    tokio::time::timeout(Duration::from_secs(60), async {
        while !scheduler.shared.queue.lock().unwrap().entries.is_empty() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        scheduler
            .diagnostics()
            .counters
            .iter()
            .find(|c| c.name == "feeds_failed")
            .unwrap()
            .count,
        0
    );
    assert_eq!(server.received_requests().await.unwrap().len(), SOURCES);
    assert_eq!(conn.query_row("SELECT count(*) FROM feeds WHERE last_fetched_at IS NOT NULL AND next_fetch_at>last_fetched_at", [], |r| r.get::<_,usize>(0)).unwrap(), SOURCES);
    assert!(scheduler.shutdown().await);
}

#[tokio::test]
async fn body_free_results_bypass_a_paused_parse_channel() {
    let (body_url, mut got_body, release_body, body_server) = gated_response(rss_response()).await;
    let (_dir, path) = fixture();
    let scheduler = Scheduler::with_limits(
        path.clone(),
        NetworkClients::new(&path),
        |_| {},
        // Pause parsing without holding SQLite's write lock. Body-free results
        // must still finish, even while an earlier 200 response cannot be parsed.
        runtime::Limits {
            parsing: 0,
            ..runtime::Limits::default()
        },
    );
    let body = scheduler.add_feed(&body_url).await.unwrap();
    let pending = scheduler.submit(body.id).unwrap();
    got_body.recv().await.unwrap();
    release_body.add_permits(1);
    wait_for_buffered_bytes(&scheduler, RSS.len()).await;
    let mut servers = Vec::new();
    for status in [304, 500] {
        let head = format!(
            "HTTP/1.1 {status} Test\r\nContent-Length: 999999\r\nConnection: close\r\n\r\n"
        );
        let (url, mut arrived, _close, server) =
            controlled_response(head.into_bytes(), Vec::new()).await;
        servers.push(server);
        let added = scheduler.add_feed(&url).await.unwrap();
        let ticket = scheduler.submit(added.id).unwrap();
        let result = tokio::time::timeout(Duration::from_secs(10), async {
            arrived.recv().await.unwrap();
            ticket.wait().await.unwrap()
        })
        .await
        .unwrap();
        assert!(match status {
            304 => result.status == RefreshStatus::NotModified,
            _ => matches!(result.status, RefreshStatus::Failed { .. }),
        });
        assert_eq!(
            scheduler.buffered_bytes(),
            RSS.len(),
            "body-free responses do not enter the byte buffer"
        );
    }
    assert!(scheduler.shutdown().await);
    assert!(pending.wait().await.is_err());
    let unparsed = rrss_core::feeds::get_feed(&db::open_at(&path).unwrap(), body.id).unwrap();
    assert!(unparsed.last_fetched_at.is_none());
    assert!(unparsed.etag.is_none());
    body_server.abort();
    for server in servers {
        server.abort();
    }
}

#[tokio::test]
async fn partial_download_bytes_backpressure_new_dispatch_and_failure_releases_them() {
    let prefix = &RSS[..RSS.len() / 2];
    let initial = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{prefix}",
        RSS.len()
    );
    let (partial, mut got_partial, close_partial, server_partial) =
        controlled_response(initial.into_bytes(), Vec::new()).await;
    let (fast, mut got_fast, release_fast, server_fast) = gated_response(rss_response()).await;
    let (_dir, path) = fixture();
    let scheduler = Scheduler::with_limits(
        path.clone(),
        NetworkClients::new(&path),
        |_| {},
        runtime::Limits {
            global: 2,
            buffer_bytes: prefix.len(),
            ..runtime::Limits::default()
        },
    );
    let partial = scheduler.add_feed(&partial).await.unwrap();
    let partial_ticket = scheduler.submit(partial.id).unwrap();
    got_partial.recv().await.unwrap();
    wait_for_buffered_bytes(&scheduler, prefix.len()).await;
    assert_eq!(
        scheduler.buffered_bytes(),
        prefix.len(),
        "in-flight bytes count before EOF"
    );
    let conn = db::open_at(&path).unwrap();
    conn.execute("INSERT INTO feeds(url,created_at) VALUES(?1,0)", [&fast])
        .unwrap();
    let fast_ticket = scheduler.submit(conn.last_insert_rowid()).unwrap();
    scheduler.start_refresh().await.unwrap();
    assert!(
        got_fast.try_recv().is_err(),
        "dispatch must wait for byte pressure to clear"
    );
    let conn = db::open_at(&path).unwrap();
    conn.execute(
        "UPDATE feeds SET url='https://changed.invalid/partial' WHERE id=?1",
        [partial.id],
    )
    .unwrap();
    close_partial.add_permits(1);
    tokio::time::timeout(Duration::from_secs(10), async {
        assert!(matches!(
            partial_ticket.wait().await.unwrap().status,
            RefreshStatus::Failed { .. }
        ));
        got_fast.recv().await.unwrap();
        release_fast.add_permits(1);
        assert!(matches!(
            fast_ticket.wait().await.unwrap().status,
            RefreshStatus::Updated { inserted: 1, .. }
        ));
    })
    .await
    .unwrap();
    let changed = rrss_core::feeds::get_feed(&conn, partial.id).unwrap();
    assert!(
        changed.last_fetched_at.is_none(),
        "an old network failure must not update a changed source"
    );
    assert!(changed.etag.is_none());
    assert!(scheduler.shutdown().await);
    assert_eq!(scheduler.buffered_bytes(), 0);
    server_partial.abort();
    server_fast.abort();
}

#[tokio::test]
async fn backpressure_keeps_inflight_downloads_and_stop_discards_unparsed_bodies_for_restart() {
    let (first_url, mut got_first, release_first, server_first) =
        gated_response(rss_response()).await;
    let (second_url, mut got_second, release_second, server_second) =
        gated_response(rss_response()).await;
    let (_dir, path) = fixture();
    let scheduler = Scheduler::with_limits(
        path.clone(),
        NetworkClients::new(&path),
        |_| {},
        runtime::Limits {
            global: 2,
            parsing: 1,
            buffer_bytes: RSS.len(),
            ..runtime::Limits::default()
        },
    );
    let first = scheduler.add_feed(&first_url).await.unwrap();
    let second = scheduler.add_feed(&second_url).await.unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        got_first.recv().await.unwrap();
        got_second.recv().await.unwrap();
    })
    .await
    .unwrap();
    scheduler.start_refresh().await.unwrap();
    let first_ticket = scheduler.submit(first.id).unwrap();
    let second_ticket = scheduler.submit(second.id).unwrap();
    let conn = db::open_at(&path).unwrap();
    let transaction = db::write_transaction(&conn).unwrap();
    release_first.add_permits(1);
    wait_for_buffered_bytes(&scheduler, RSS.len()).await;
    // The other already admitted download still completes under backpressure.
    release_second.add_permits(1);
    wait_for_buffered_bytes(&scheduler, RSS.len() * 2).await;
    assert_eq!(scheduler.snapshot().operations[0].completed, 0);
    let run = scheduler.snapshot().run_id;
    scheduler.stop();
    tokio::time::timeout(Duration::from_secs(10), async {
        while scheduler.buffered_bytes() != 0 {
            tokio::task::yield_now().await;
        }
        assert!(first_ticket.wait().await.is_err());
        assert!(second_ticket.wait().await.is_err());
    })
    .await
    .unwrap();
    transaction.commit().unwrap();
    assert!(scheduler.shutdown().await);
    let pending = rrss_core::feeds::get_feed(&conn, second.id).unwrap();
    assert!(pending.last_fetched_at.is_none());
    assert!(pending.etag.is_none());
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM articles WHERE feed_id=?1",
            [second.id],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    let restarted = Scheduler::start(path.clone(), NetworkClients::new(&path), |_| {});
    assert_ne!(restarted.snapshot().run_id, run);
    let ticket = restarted.submit(second.id).unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        got_second.recv().await.unwrap();
        release_second.add_permits(1);
        assert!(matches!(
            ticket.wait().await.unwrap().status,
            RefreshStatus::Updated { inserted: 1, .. }
        ));
    })
    .await
    .unwrap();
    assert_eq!(
        rrss_core::feeds::get_feed(&conn, second.id)
            .unwrap()
            .etag
            .as_deref(),
        Some("buffered-version")
    );
    assert!(restarted.shutdown().await);
    server_first.abort();
    server_second.abort();
}

#[tokio::test]
async fn redirect_releases_origin_capacity_before_waiting_for_target() {
    let (target, mut arrived, release, target_task) = gated_server().await;
    let redirect = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(302).insert_header("Location", target.as_str()))
        .mount(&redirect)
        .await;
    let other = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(304))
        .expect(1)
        .mount(&other)
        .await;
    let (_dir, path) = fixture();
    let limits = runtime::Limits {
        global: 2,
        host: 1,
        parsing: 2,
        ..runtime::Limits::default()
    };
    let scheduler =
        Scheduler::with_limits(path.clone(), NetworkClients::new(&path), |_| {}, limits);
    let direct = scheduler.add_feed(&target).await.unwrap();
    arrived.recv().await.unwrap();
    let redirected = scheduler.add_feed(&redirect.uri()).await.unwrap();
    let redirected_ticket = scheduler.submit(redirected.id).unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        while redirect.received_requests().await.unwrap().is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let independent = scheduler.add_feed(&other.uri()).await.unwrap();
    let other_ticket = scheduler.submit(independent.id).unwrap();
    tokio::time::timeout(Duration::from_secs(10), other_ticket.wait())
        .await
        .unwrap()
        .unwrap();
    assert!(
        arrived.try_recv().is_err(),
        "redirect must obey target host capacity"
    );
    let direct_ticket = scheduler.submit(direct.id).unwrap();
    release.add_permits(2);
    direct_ticket.wait().await.unwrap();
    redirected_ticket.wait().await.unwrap();
    assert!(arrived.try_recv().is_ok());
    target_task.abort();
}

#[tokio::test]
async fn blocked_model_does_not_block_add_or_refresh_and_background_generations_survive() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string(RSS))
        .mount(&server)
        .await;
    let (_dir, path) = fixture();
    let scheduler = Scheduler::start(path.clone(), NetworkClients::new(&path), |_| {});
    let conn = db::open_at(&path).unwrap();
    conn.execute(
        "INSERT INTO ai_cache(cache_key, value, created_at) VALUES('expired', '{}', 0)",
        [],
    )
    .unwrap();
    let mut events = scheduler.shared.completed.subscribe();
    let (blocked, _) = tokio::sync::watch::channel(None);
    *scheduler.shared.capacity.embedding.lock().unwrap() = Some(model_capacity::EmbeddingRun {
        epoch: 0,
        key: "blocked".into(),
        result: blocked.clone(),
    });
    let first = scheduler.request_background().unwrap();
    let added = scheduler.add_feed(&server.uri()).await.unwrap();
    tokio::time::timeout(
        Duration::from_secs(10),
        scheduler.submit(added.id).unwrap().wait(),
    )
    .await
    .unwrap()
    .unwrap();
    let operation = scheduler.start_refresh().await.unwrap();
    let result = wait_operation(&scheduler, operation.operation_id).await;
    assert_eq!(result.completed, 1);
    let last = scheduler.request_background().unwrap();
    assert!(last > first);
    assert_eq!(*scheduler.shared.completed.borrow(), 0);
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let count: i64 = conn
                .query_row("SELECT COUNT(*) FROM ai_cache", [], |r| r.get(0))
                .unwrap();
            if count == 0 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        *events.borrow_and_update(),
        0,
        "maintenance ran while model stayed blocked"
    );
    blocked.send_replace(Some(Ok(())));
    scheduler.shared.capacity.embedding.lock().unwrap().take();
    tokio::time::timeout(Duration::from_secs(10), scheduler.wait_background(last))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(*scheduler.shared.completed.borrow(), last);
    scheduler.stop();
    assert!(scheduler.request_background().is_err());
    assert!(scheduler.wait_background(last + 1).await.is_err());
}

#[tokio::test]
async fn stopping_releases_manual_object_waiter_without_affecting_another_database() {
    let (_dir, path) = fixture();
    db::open_at(&path).unwrap().execute_batch("INSERT INTO feeds(id,url,created_at) VALUES(1,'https://fixture.invalid',0); INSERT INTO articles(id,feed_id,title,created_at) VALUES(1,1,'test',0);").unwrap();
    let scheduler = Scheduler::start(path.clone(), NetworkClients::new(&path), |_| {});
    let object = scheduler.shared.capacity.object("summary:1".into());
    let _guard = object.lock().await;
    let summary = tokio::spawn({
        let scheduler = scheduler.clone();
        async move { scheduler.summarize_article(1).await }
    });
    tokio::time::timeout(Duration::from_secs(10), async {
        while scheduler.shared.manual.available_permits() == 32 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let (_other_dir, other_path) = fixture();
    let other = Scheduler::start(other_path.clone(), NetworkClients::new(&other_path), |_| {});
    let other_object = other.shared.capacity.object("summary:1".into());
    assert!(other_object.try_lock().is_ok());
    scheduler.stop();
    let error = tokio::time::timeout(Duration::from_secs(10), summary)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert!(matches!(
        crate::UiError::from(error).code(),
        "job_runtime_stopped" | "job_model_stopped_retry" | "job_model_stopped"
    ));
    assert!(!other.snapshot().stopped);
    other.stop();
}

#[tokio::test]
async fn duplicate_manual_summary_shares_one_execution_when_a_waiter_leaves() {
    let body = serde_json::json!({"choices":[{"message":{"content":"{\"summary\":\"shared\"}"}}],
        "usage":{"prompt_tokens":3,"completion_tokens":2}})
    .to_string();
    let reply = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let (url, mut arrived, release, server) = gated_response(reply.into_bytes()).await;
    let (_dir, path) = fixture();
    let conn = db::open_at(&path).unwrap();
    conn.execute_batch("INSERT INTO feeds(id,url,created_at) VALUES(1,'https://fixture.invalid',0);
        INSERT INTO articles(id,feed_id,title,content_html,created_at) VALUES(1,1,'test','body',0);").unwrap();
    let provider = crate::test_net::fake_provider(&conn, "fixture", url);
    rrss_core::ai_store::set_task_binding(
        &conn,
        rrss_core::ai_store::TASK_ARTICLE_SUMMARY,
        Some(provider),
        Some("test"),
    )
    .unwrap();
    let scheduler = Scheduler::start(path.clone(), NetworkClients::new(&path), |_| {});
    let first = tokio::spawn({
        let scheduler = scheduler.clone();
        async move { scheduler.summarize_article(1).await }
    });
    arrived.recv().await.unwrap();
    let second = tokio::spawn({
        let scheduler = scheduler.clone();
        async move { scheduler.summarize_article(1).await }
    });
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if scheduler
                .shared
                .manual_jobs
                .lock()
                .unwrap()
                .values()
                .any(|slot| slot.receiver_count() == 2)
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    first.abort();
    release.add_permits(1);
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(10), second)
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
        "shared"
    );
    assert!(arrived.try_recv().is_err());
    assert_eq!(
        rrss_core::articles::get_article(&conn, 1)
            .unwrap()
            .summary
            .as_deref(),
        Some("shared")
    );
    scheduler.stop();
    server.abort();
}

#[tokio::test]
async fn refresh_operations_share_generations_across_pages_and_keep_fixed_targets() {
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{RSS}",
        RSS.len()
    );
    let (url, mut arrived, release, server) = gated_response(response.into_bytes()).await;
    let (_dir, path) = fixture();
    let conn = db::open_at(&path).unwrap();
    for id in 1..=70 {
        conn.execute(
            "INSERT INTO feeds(id,url,created_at) VALUES(?1,?2,0)",
            rusqlite::params![id, format!("{url}/{id}")],
        )
        .unwrap();
    }
    let scheduler = Scheduler::with_limits(
        path.clone(),
        NetworkClients::new(&path),
        |_| {},
        runtime::Limits {
            pending: 2,
            ..runtime::Limits::default()
        },
    );
    let first = scheduler.start_refresh().await.unwrap();
    arrived.recv().await.unwrap();
    let second = scheduler.start_refresh().await.unwrap();
    conn.execute("DELETE FROM feeds WHERE id=40", []).unwrap();
    conn.execute(
        "INSERT INTO feeds(id,url,created_at) VALUES(71,?1,0)",
        [format!("{url}/new")],
    )
    .unwrap();
    release.add_permits(69);
    let completed = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let snapshot = scheduler.snapshot();
            if snapshot.operations.len() == 2 && snapshot.operations.iter().all(|op| op.done) {
                break snapshot;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(first.run_id, second.run_id);
    assert_ne!(first.operation_id, second.operation_id);
    for operation in &completed.operations {
        assert_eq!(
            (
                operation.target,
                operation.completed,
                operation.deleted,
                operation.failed
            ),
            (70, 70, 1, 0)
        );
        assert_eq!(operation.inserted, 69);
    }
    let mut remaining = 0;
    while arrived.try_recv().is_ok() {
        remaining += 1;
    }
    assert_eq!(
        remaining, 68,
        "one request per existing target across both operations"
    );
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM articles WHERE feed_id=71", [], |r| {
            r.get::<_, i64>(0)
        })
        .unwrap(),
        0
    );
    scheduler.stop();
    server.abort();
}

#[tokio::test]
async fn active_refresh_admission_is_bounded_and_stop_produces_terminal_snapshots() {
    let (url, mut arrived, _release, server) = gated_server().await;
    let (_dir, path) = fixture();
    let conn = db::open_at(&path).unwrap();
    conn.execute("INSERT INTO feeds(id,url,created_at) VALUES(1,?1,0)", [url])
        .unwrap();
    let scheduler = Scheduler::start(path.clone(), NetworkClients::new(&path), |_| {});
    let same = Scheduler::start(path.clone(), NetworkClients::new(&path), |_| {});
    assert_eq!(scheduler.snapshot().run_id, same.snapshot().run_id);
    scheduler.start_refresh().await.unwrap();
    arrived.recv().await.unwrap();
    for _ in 1..8 {
        same.start_refresh().await.unwrap();
    }
    assert!(scheduler.start_refresh().await.is_err());
    scheduler.shared.capacity.summaries.send_replace(None);
    scheduler.stop();
    assert_eq!(
        *scheduler.shared.capacity.summaries.borrow(),
        Some(Default::default())
    );
    let snapshot = same.snapshot();
    assert!(snapshot.stopped);
    assert!(
        snapshot
            .operations
            .iter()
            .all(|operation| operation.done && operation.skipped == 1)
    );
    assert!(same.start_refresh().await.is_err());
    let restarted = Scheduler::start(path.clone(), NetworkClients::new(&path), |_| {});
    assert_ne!(snapshot.run_id, restarted.snapshot().run_id);
    assert!(restarted.snapshot().operations.is_empty());
    let epoch = *restarted.shared.capacity.stopped.borrow();
    drop(scheduler);
    drop(same);
    assert_eq!(*restarted.shared.capacity.stopped.borrow(), epoch);
    restarted.stop();
    server.abort();
}

#[tokio::test]
async fn shutdown_waits_for_a_started_blocking_transaction() {
    let (_dir, path) = fixture();
    let scheduler = Scheduler::start(path.clone(), NetworkClients::new(&path), |_| {});
    let (entered, started) = tokio::sync::oneshot::channel();
    let (release, released) = std::sync::mpsc::channel();
    let write = tokio::spawn(crate::jobs::spawn_db(path.clone(), move |conn| {
        rrss_core::db::atomic_write(conn, |conn| {
            entered.send(()).unwrap();
            released.recv().unwrap();
            db::set_setting(conn, "shutdown_fixture", "committed")
        })
    }));
    started.await.unwrap();
    let shutdown = tokio::spawn({
        let scheduler = scheduler.clone();
        async move { scheduler.shutdown().await }
    });
    tokio::task::yield_now().await;
    assert!(!shutdown.is_finished());
    release.send(()).unwrap();
    write.await.unwrap().unwrap();
    assert!(shutdown.await.unwrap());
    assert_eq!(
        db::get_setting(&db::open_at(&path).unwrap(), "shutdown_fixture")
            .unwrap()
            .as_deref(),
        Some("committed")
    );
}

#[tokio::test]
async fn scheduled_fetch_keeps_running_without_accumulating_automatic_ai_rounds() {
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{RSS}",
        RSS.len()
    );
    let (url, mut arrived, release, server) = gated_response(response.into_bytes()).await;
    let (_dir, path) = fixture();
    db::open_at(&path)
        .unwrap()
        .execute(
            "INSERT INTO feeds(id,url,created_at,interval_secs) VALUES(1,?1,0,0)",
            [url],
        )
        .unwrap();
    let scheduler = Scheduler::start(path.clone(), NetworkClients::new(&path), |_| {});
    let (blocked, _) = tokio::sync::watch::channel(None);
    *scheduler.shared.capacity.embedding.lock().unwrap() = Some(model_capacity::EmbeddingRun {
        epoch: 0,
        key: "blocked".into(),
        result: blocked,
    });
    scheduler.request_background().unwrap();
    let running = tokio::spawn({
        let scheduler = scheduler.clone();
        async move {
            scheduler
                .run_loop_with_interval(Duration::from_millis(10))
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(10), arrived.recv())
        .await
        .unwrap()
        .unwrap();
    release.add_permits(1);
    tokio::time::timeout(Duration::from_secs(10), arrived.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(scheduler.snapshot().background_requested, 1);
    assert_eq!(scheduler.snapshot().background_completed, 0);
    scheduler.stop();
    running.await.unwrap();
    scheduler.shared.capacity.embedding.lock().unwrap().take();
    server.abort();
}

#[tokio::test]
async fn completed_article_embeds_while_another_summary_is_still_pending() {
    use rrss_core::ai_store;
    use serde_json::json;
    let summary_server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(
                json!({"choices":[{"message":{"content":"{\"summary\":\"ready\"}"}}]}),
            ),
        )
        .mount(&summary_server)
        .await;
    let embed_server = MockServer::start().await;
    Mock::given(method("POST")).respond_with(|request: &wiremock::Request| {
        let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
        let inputs = body["input"].as_array().cloned().unwrap_or_else(|| vec![body["input"].clone()]);
        let vectors: Vec<_> = inputs.iter().enumerate().map(|(i,text)| json!({"index":i,"embedding":[text.as_str().unwrap().len() as f32,1.0]})).collect();
        ResponseTemplate::new(200).set_body_json(json!({"data":vectors,"usage":{"total_tokens":1}}))
    }).mount(&embed_server).await;
    let (_dir, path) = fixture();
    let conn = db::open_at(&path).unwrap();
    ai_store::set_daily_budget_tokens(&conn, 0).unwrap();
    db::set_setting(&conn, ai_store::FOCUS_AUTO_SUMMARY.id, "1").unwrap();
    let summaries = crate::test_net::fake_provider(&conn, "summary", summary_server.uri());
    let embeddings = crate::test_net::fake_provider(&conn, "embedding", embed_server.uri());
    ai_store::set_task_binding(
        &conn,
        ai_store::TASK_FOCUS_SUMMARY,
        Some(summaries),
        Some("m"),
    )
    .unwrap();
    ai_store::set_task_binding(&conn, ai_store::TASK_EMBED, Some(embeddings), Some("m")).unwrap();
    conn.execute_batch("INSERT INTO feeds(id,url,created_at) VALUES(1,'https://fixture.invalid',0);
        INSERT INTO articles(id,feed_id,title,hash,created_at,score,content_html) VALUES(1,1,'held','h1',1,1,'<p>held</p>'),(2,1,'ready','h2',1,2,'<p>ready</p>');").unwrap();
    conn.execute(
        "UPDATE articles SET created_at=?1",
        [rrss_core::feeds::now_ms()],
    )
    .unwrap();
    let scheduler = Scheduler::start(path.clone(), NetworkClients::new(&path), |_| {});
    let held = scheduler.shared.capacity.object("summary:1".into());
    let guard = held.lock().await;
    let version = scheduler.request_background().unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let embedded: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM chunks WHERE article_id=2 AND embedding IS NOT NULL)", [], |r| r.get(0)).unwrap();
            if embedded { break; }
            tokio::task::yield_now().await;
        }
    }).await.unwrap();
    assert!(
        rrss_core::articles::get_article(&conn, 1)
            .unwrap()
            .summary
            .is_none()
    );
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM chunks WHERE article_id=1", [], |r| {
            r.get::<_, i64>(0)
        })
        .unwrap(),
        0
    );
    drop(guard);
    tokio::time::timeout(Duration::from_secs(10), scheduler.wait_background(version))
        .await
        .unwrap()
        .unwrap();
    assert!(
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM chunks WHERE article_id=1 AND embedding IS NOT NULL)",
            [],
            |r| r.get::<_, bool>(0)
        )
        .unwrap()
    );
    assert_eq!(summary_server.received_requests().await.unwrap().len(), 2);
    assert!(scheduler.shutdown().await);
}

#[tokio::test]
async fn new_articles_publish_before_incremental_rank_finishes() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string(RSS))
        .mount(&server)
        .await;
    let (_dir, path) = fixture();
    let conn = db::open_at(&path).unwrap();
    let (tx, mut events) = mpsc::unbounded_channel();
    let scheduler = Scheduler::start(path.clone(), NetworkClients::new(&path), move |event| {
        let _ = tx.send(event);
    });
    // 排序一直算不完时，入库、源事件与刷新完成都不等它。
    let slow_rank = scheduler.shared.capacity.rank.hold().await;
    let added = scheduler.add_feed(&server.uri()).await.unwrap();
    let event = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let event = events.recv().await.unwrap();
            if event.feed_id == Some(added.id) && event.inserted == 1 {
                break event;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(event.status, "updated");
    drop(slow_rank);
    tokio::time::timeout(Duration::from_secs(10), async {
        while conn
            .query_row("SELECT COUNT(*) FROM article_rank", [], |r| {
                r.get::<_, i64>(0)
            })
            .unwrap()
            != 1
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(scheduler.shutdown().await);
}

#[tokio::test]
async fn rank_failure_reports_derived_degradation_and_recovers_without_ai_or_feed_failure() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string(RSS))
        .mount(&server)
        .await;
    let (_dir, path) = fixture();
    let conn = db::open_at(&path).unwrap();
    conn.execute_batch("CREATE TRIGGER fail_rank BEFORE INSERT ON article_rank BEGIN SELECT RAISE(ABORT,'injected rank'); END;").unwrap();
    let (tx, mut events) = mpsc::unbounded_channel();
    let scheduler = Scheduler::start(path.clone(), NetworkClients::new(&path), move |event| {
        let _ = tx.send(event);
    });
    let added = scheduler.add_feed(&server.uri()).await.unwrap();
    // 源事件在入库提交后先发；后台增量排序失败时再补发该源的衍生降级事件。
    let event = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let event = events.recv().await.unwrap();
            if event.feed_id == Some(added.id) && event.derived_degraded {
                break event;
            }
        }
    })
    .await
    .unwrap();
    assert!(event.error.is_none());
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM articles WHERE feed_id=?1",
            [added.id],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    let feed = rrss_core::feeds::get_feed(&conn, added.id).unwrap();
    assert!(feed.error.is_none());
    assert_eq!(
        conn.query_row(
            "SELECT fail_count FROM feeds WHERE id=?1",
            [added.id],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    assert_eq!(
        scheduler.snapshot().background_requested,
        0,
        "adding a source cannot request an AI round"
    );
    let mut status = scheduler.shared.background_status.subscribe();
    tokio::time::timeout(
        Duration::from_secs(10),
        status.wait_for(|jobs| {
            jobs.iter()
                .any(|job| job.kind == "clusters" && job.state == "cooldown")
        }),
    )
    .await
    .unwrap()
    .unwrap();
    conn.execute_batch("DROP TRIGGER fail_rank;").unwrap();
    scheduler
        .shared
        .capacity
        .derived
        .send_modify(|version| *version += 1);
    tokio::time::pause();
    tokio::time::advance(crate::jobs::retry::DELAY).await;
    tokio::time::resume();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if conn
                .query_row("SELECT COUNT(*) FROM article_rank", [], |r| {
                    r.get::<_, i64>(0)
                })
                .unwrap()
                == 1
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(scheduler.snapshot().background_requested, 0);
    assert!(scheduler.shutdown().await);
}

async fn wait_operation(scheduler: &Scheduler, id: u64) -> views::RefreshOperation {
    tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            if let Some(operation) = scheduler
                .snapshot()
                .operations
                .into_iter()
                .find(|op| op.id == id && op.done)
            {
                return operation;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("refresh operation must settle")
}

#[tokio::test]
async fn automatic_cluster_summary_runs_after_the_article_summary_batch() {
    use rrss_core::ai_store;
    use serde_json::json;
    let server = MockServer::start().await;
    let (_dir, path) = fixture();
    let conn = db::open_at(&path).unwrap();
    ai_store::set_daily_budget_tokens(&conn, 0).unwrap();
    for id in [
        ai_store::FOCUS_AUTO_SUMMARY.id,
        ai_store::CLUSTER_AUTO_SUMMARY.id,
    ] {
        db::set_setting(&conn, id, "1").unwrap();
    }
    db::set_setting(
        &conn,
        rrss_core::cluster::ALGORITHM_VERSION_KEY,
        rrss_core::cluster::ALGORITHM_VERSION,
    )
    .unwrap();
    let provider = crate::test_net::fake_provider(&conn, "fixture", server.uri());
    ai_store::set_task_binding(
        &conn,
        ai_store::TASK_FOCUS_SUMMARY,
        Some(provider),
        Some("article"),
    )
    .unwrap();
    ai_store::set_task_binding(
        &conn,
        ai_store::TASK_CLUSTER_SUMMARY,
        Some(provider),
        Some("cluster"),
    )
    .unwrap();
    conn.execute_batch("INSERT INTO feeds(id,url,created_at) VALUES(1,'https://a.invalid/feed',0),(2,'https://b.invalid/feed',0);
        INSERT INTO clusters(id,label,member_count,created_at) VALUES(7,'事件',2,0);
        INSERT INTO articles(id,feed_id,title,hash,score,cluster_id,simhash,created_at) VALUES
        (1,1,'甲报道','a',1,7,123,0),(2,2,'乙报道','b',1,7,123,0);").unwrap();
    conn.execute(
        "UPDATE articles SET created_at=?1",
        [rrss_core::feeds::now_ms()],
    )
    .unwrap();
    let inspected = path.clone();
    Mock::given(method("POST")).respond_with(move |request: &wiremock::Request| {
        let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
        let content = if body["model"] == "cluster" {
            let conn = db::open_at(&inspected).unwrap();
            let summarized: i64 = conn.query_row("SELECT count(*) FROM articles WHERE summary='先完成的摘要'", [], |r| r.get(0)).unwrap();
            assert_eq!(summarized, 2, "综述必须等待这批单篇摘要写入");
            json!({"overview":"随后生成的综述"})
        } else {
            json!({"summary":"先完成的摘要"})
        };
        ResponseTemplate::new(200).set_body_json(json!({"choices":[{"message":{"content":content.to_string()}}],"usage":{"prompt_tokens":1,"completion_tokens":1}}))
    }).mount(&server).await;
    let scheduler = Scheduler::start(path.clone(), NetworkClients::new(&path), |_| {});
    let version = scheduler.request_background().unwrap();
    tokio::time::timeout(Duration::from_secs(30), scheduler.wait_background(version))
        .await
        .unwrap()
        .unwrap();
    let summary = rrss_core::cluster_summary::load(&conn, 7).unwrap().unwrap();
    assert_eq!(summary.digest.overview, "随后生成的综述");
    let usage = ai_store::usage_on(&conn, &ai_store::local_day()).unwrap();
    assert_eq!((usage.auto_summaries, usage.auto_cluster_summaries), (2, 1));
    scheduler.shutdown().await;
}
