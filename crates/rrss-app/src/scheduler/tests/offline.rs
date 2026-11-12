use super::*;
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use rrss_core::network::{self, ProxyConfig, ProxyRule};
use std::net::SocketAddr;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Clone, Debug)]
struct Request {
    method: String,
    host: String,
    target: String,
}

struct HttpFixture {
    address: SocketAddr,
    online: tokio::sync::watch::Sender<bool>,
    requests: Arc<Mutex<Vec<Request>>>,
    peak_gets: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for HttpFixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}

struct ActiveGet(Arc<AtomicUsize>);
impl Drop for ActiveGet {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

async fn http_fixture(bind: Option<SocketAddr>, available: bool) -> HttpFixture {
    let listener =
        tokio::net::TcpListener::bind(bind.unwrap_or_else(|| "127.0.0.1:0".parse().unwrap()))
            .await
            .unwrap();
    let address = listener.local_addr().unwrap();
    let (online, _) = tokio::sync::watch::channel(available);
    let flag = online.clone();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let observed = requests.clone();
    let active = Arc::new(AtomicUsize::new(0));
    let peak_gets = Arc::new(AtomicUsize::new(0));
    let peak = peak_gets.clone();
    let task = tokio::spawn(async move {
        let mut tasks = tokio::task::JoinSet::new();
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let observed = observed.clone();
            let active = active.clone();
            let peak = peak.clone();
            let mut flag = flag.subscribe();
            tasks.spawn(async move {
                let mut bytes = Vec::new();
                let mut chunk = [0;4096];
                while !bytes.ends_with(b"\r\n\r\n") {
                    let count = socket.read(&mut chunk).await.unwrap_or(0);
                    if count == 0 { return; }
                    bytes.extend_from_slice(&chunk[..count]);
                    assert!(bytes.len() <= 32 * 1024);
                }
                let text = String::from_utf8(bytes).unwrap();
                let first = text.lines().next().unwrap();
                let mut parts = first.split_whitespace();
                let method = parts.next().unwrap().to_string();
                let target = parts.next().unwrap().to_string();
                let host = text.lines().find_map(|line| {
                    line.split_once(':').filter(|(name,_)| name.eq_ignore_ascii_case("host")).map(|(_,value)| value.trim().to_string())
                }).unwrap();
                observed.lock().unwrap().push(Request {method:method.clone(),host,target});
                let _get = if method == "GET" {
                    let count = active.fetch_add(1,Ordering::AcqRel)+1;
                    peak.fetch_max(count,Ordering::AcqRel);
                    Some(ActiveGet(active))
                } else { None };
                if !*flag.borrow() {
                    tokio::select! {
                        _ = flag.wait_for(|available| *available) => {},
                        _ = socket.read(&mut chunk) => return,
                    }
                }
                let response = if method == "HEAD" {
                    b"HTTP/1.1 405 Method Not Allowed\r\nContent-Length: 999999\r\nConnection: close\r\n\r\n".as_slice()
                } else {
                    b"HTTP/1.1 304 Not Modified\r\nContent-Length: 999999\r\nConnection: close\r\n\r\n".as_slice()
                };
                socket.write_all(response).await.ok();
                // HEAD/304 must complete without reading this advertised body.
                let _ = socket.read_exact(&mut [0; 1]).await;
            });
            while tasks.try_join_next().is_some() {}
        }
    });
    HttpFixture {
        address,
        online,
        requests,
        peak_gets,
        task,
    }
}

struct TestDns {
    available: Arc<AtomicBool>,
    address: SocketAddr,
    fallback_only: bool,
    calls: Arc<Mutex<Vec<String>>>,
}
impl Resolve for TestDns {
    fn resolve(&self, name: Name) -> Resolving {
        let name = name.as_str().to_string();
        self.calls.lock().unwrap().push(name.clone());
        let available = self.available.load(Ordering::Acquire)
            || (self.fallback_only
                && matches!(
                    name.as_str(),
                    "captive.apple.com" | "www.msftconnecttest.com"
                ));
        let address = self.address;
        Box::pin(async move {
            if available {
                Ok(Box::new([address].into_iter()) as Addrs)
            } else {
                Err(Box::new(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "injected DNS failure",
                ))
                    as Box<dyn std::error::Error + Send + Sync>)
            }
        })
    }
}

fn seed_sources(conn: &rusqlite::Connection, port: u16, recent: bool) -> i64 {
    let old = now_ms() - 600_000;
    for (id, host) in [(1, "one.invalid"), (2, "two.invalid"), (3, "three.invalid")] {
        conn.execute(
            "INSERT INTO feeds(id,url,created_at,last_fetched_at,last_success_at,
            fail_count,failing_since,slow_fail_count,error,next_fetch_at)
            VALUES(?1,?2,0,?3,?4,2,?3,2,'previous failure',0)",
            rusqlite::params![
                id,
                format!("http://{host}:{port}/feed"),
                old,
                recent.then_some(old)
            ],
        )
        .unwrap();
    }
    old
}

fn health(
    conn: &rusqlite::Connection,
) -> Vec<(
    i64,
    Option<i64>,
    i64,
    i64,
    Option<i64>,
    Option<i64>,
    Option<String>,
)> {
    conn.prepare("SELECT fail_count,failing_since,slow_fail_count,next_fetch_at,last_fetched_at,last_success_at,error FROM feeds WHERE id<=3 ORDER BY id").unwrap()
        .query_map([],|r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?))).unwrap()
        .collect::<rusqlite::Result<_>>().unwrap()
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

fn limits() -> runtime::Limits {
    runtime::Limits {
        global: 4,
        policy: rrss_core::ingest::feed_http::FeedPolicy {
            timeout: Duration::from_secs(1),
            ..Default::default()
        },
        offline: super::super::offline::Policy {
            timeout: Duration::from_millis(500),
            ..Default::default()
        },
        ..runtime::Limits::default()
    }
}

#[tokio::test]
async fn dns_outage_preserves_health_pauses_dispatch_and_automatic_head_probe_resumes() {
    let server = http_fixture(None, true).await;
    let (_dir, path) = fixture();
    let conn = db::open_at(&path).unwrap();
    let old = seed_sources(&conn, server.address.port(), true);
    let original = health(&conn);
    let available = Arc::new(AtomicBool::new(false));
    let calls = Arc::new(Mutex::new(Vec::new()));
    let dns = Arc::new(TestDns {
        available: available.clone(),
        address: server.address,
        fallback_only: false,
        calls: calls.clone(),
    });
    let clients = NetworkClients::with_feed_builder(&path, move |builder| {
        builder.dns_resolver(dns.clone()).pool_max_idle_per_host(0)
    });
    let mut policy = limits();
    policy.offline.first_interval = Duration::from_millis(100);
    policy.offline.max_interval = Duration::from_millis(200);
    let scheduler = Scheduler::with_limits(path.clone(), clients, |_| {}, policy);
    scheduler.start_refresh().await.unwrap();
    wait_until(|| scheduler.snapshot().feed_pause.is_some()).await;
    assert_eq!(health(&conn), original);
    assert!(server.requests.lock().unwrap().is_empty());
    let added = scheduler
        .add_feed(&format!(
            "http://four.invalid:{}/feed",
            server.address.port()
        ))
        .await
        .unwrap();
    assert!(added.message.contains("网络不可用"));
    let start = tokio::time::timeout(Duration::from_secs(5), scheduler.start_refresh())
        .await
        .unwrap()
        .unwrap();
    let view = scheduler
        .snapshot()
        .operations
        .into_iter()
        .find(|op| op.id == start.operation_id)
        .unwrap();
    assert!(view.done);
    assert_eq!((view.skipped, view.failed), (4, 0));
    assert!(view.error.unwrap().contains("网络不可用"));
    assert_eq!(health(&conn), original);
    assert!(
        calls
            .lock()
            .unwrap()
            .iter()
            .all(|host| !host.contains("apple") && !host.contains("msft")),
        "recent subscription hosts must be used"
    );
    available.store(true, Ordering::Release);
    wait_until(|| {
        scheduler.snapshot().feed_pause.is_none()
            && conn
                .query_row(
                    "SELECT count(*) FROM feeds WHERE last_success_at>?1",
                    [old],
                    |r| r.get::<_, usize>(0),
                )
                .unwrap()
                == 4
    })
    .await;
    assert!(
        health(&conn)
            .iter()
            .all(|state| state.0 == 0 && state.2 == 0)
    );
    let requests = server.requests.lock().unwrap().clone();
    let heads: Vec<_> = requests.iter().filter(|r| r.method == "HEAD").collect();
    assert!(!heads.is_empty());
    assert!(
        heads.iter().all(
            |r| r.host.ends_with(&format!(":{}", server.address.port())) && r.target == "/feed"
        )
    );
    assert!(server.peak_gets.load(Ordering::Acquire) <= 4);
    assert!(scheduler.shutdown().await);
}

#[tokio::test]
async fn refused_content_proxy_preserves_health_and_manual_probe_reports_then_recovers() {
    let reserved = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = reserved.local_addr().unwrap();
    drop(reserved);
    let (_dir, path) = fixture();
    let conn = db::open_at(&path).unwrap();
    let old = seed_sources(&conn, 80, true);
    let original = health(&conn);
    network::save(
        &conn,
        &ProxyConfig {
            content: ProxyRule::custom(&format!("http://{address}")),
            ..Default::default()
        },
    )
    .unwrap();
    let scheduler =
        Scheduler::with_limits(path.clone(), NetworkClients::new(&path), |_| {}, limits());
    scheduler.start_refresh().await.unwrap();
    wait_until(|| scheduler.snapshot().feed_pause.is_some()).await;
    assert_eq!(health(&conn), original);
    let background = scheduler.snapshot().background_requested;
    let start = tokio::time::timeout(Duration::from_secs(5), scheduler.start_refresh())
        .await
        .unwrap()
        .unwrap();
    let view = scheduler
        .snapshot()
        .operations
        .into_iter()
        .find(|op| op.id == start.operation_id)
        .unwrap();
    assert!(view.done && view.error.unwrap().contains("网络不可用"));
    assert_eq!((view.skipped, view.failed), (3, 0));
    assert!(scheduler.snapshot().background_requested > background);
    assert_eq!(health(&conn), original);
    let proxy = http_fixture(Some(address), true).await;
    assert!(scheduler.shared.check_network(true).await);
    wait_until(|| {
        conn.query_row(
            "SELECT count(*) FROM feeds WHERE last_success_at>?1",
            [old],
            |r| r.get::<_, usize>(0),
        )
        .unwrap()
            == 3
    })
    .await;
    let requests = proxy.requests.lock().unwrap().clone();
    assert!(requests.iter().any(|r| r.method == "HEAD"));
    assert!(requests.iter().all(|r| r.host.ends_with(".invalid")
        && r.target.starts_with("http://")
        && !r.target.contains("connecttest")));
    assert!(
        requests
            .iter()
            .filter(|r| r.method != "GET")
            .all(|r| r.method == "HEAD")
    );
    assert!(scheduler.snapshot().feed_pause.is_none());
    assert!(scheduler.shutdown().await);
}

#[tokio::test]
async fn timed_out_proxy_failures_and_probes_do_not_advance_source_backoff() {
    let proxy = http_fixture(None, false).await;
    let (_dir, path) = fixture();
    let conn = db::open_at(&path).unwrap();
    let old = seed_sources(&conn, 80, true);
    let original = health(&conn);
    network::save(
        &conn,
        &ProxyConfig {
            content: ProxyRule::custom(&format!("http://{}", proxy.address)),
            ..Default::default()
        },
    )
    .unwrap();
    let scheduler =
        Scheduler::with_limits(path.clone(), NetworkClients::new(&path), |_| {}, limits());
    scheduler.start_refresh().await.unwrap();
    wait_until(|| scheduler.snapshot().feed_pause.is_some()).await;
    assert_eq!(health(&conn), original);
    let before = proxy
        .requests
        .lock()
        .unwrap()
        .iter()
        .filter(|r| r.method == "GET")
        .count();
    scheduler.start_refresh().await.unwrap();
    assert_eq!(health(&conn), original);
    assert_eq!(
        proxy
            .requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.method == "GET")
            .count(),
        before
    );
    assert!(
        proxy
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|r| r.method == "HEAD")
    );
    proxy.online.send_replace(true);
    assert!(scheduler.shared.check_network(true).await);
    wait_until(|| {
        conn.query_row(
            "SELECT count(*) FROM feeds WHERE last_success_at>?1",
            [old],
            |r| r.get::<_, usize>(0),
        )
        .unwrap()
            == 3
    })
    .await;
    assert!(
        health(&conn)
            .iter()
            .all(|state| state.0 == 0 && state.2 == 0)
    );
    assert!(scheduler.shutdown().await);
}

#[tokio::test]
async fn one_success_in_window_prevents_offline_detection_among_mostly_slow_failures() {
    let proxy = MockServer::start().await;
    Mock::given(method("GET"))
        .and(wiremock::matchers::path("/success"))
        .respond_with(ResponseTemplate::new(304))
        .mount(&proxy)
        .await;
    Mock::given(method("GET"))
        .and(wiremock::matchers::path("/slow"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(RSS)
                .set_delay(Duration::from_secs(10)),
        )
        .mount(&proxy)
        .await;
    let (_dir, path) = fixture();
    let conn = db::open_at(&path).unwrap();
    for id in 1..=4 {
        conn.execute(
            "INSERT INTO feeds(id,url,created_at) VALUES(?1,?2,0)",
            rusqlite::params![
                id,
                format!(
                    "http://source-{id}.invalid/{}",
                    if id == 1 { "success" } else { "slow" }
                )
            ],
        )
        .unwrap();
    }
    network::save(
        &conn,
        &ProxyConfig {
            content: ProxyRule::custom(&proxy.uri()),
            ..Default::default()
        },
    )
    .unwrap();
    let scheduler =
        Scheduler::with_limits(path.clone(), NetworkClients::new(&path), |_| {}, limits());
    let start = scheduler.start_refresh().await.unwrap();
    wait_until(|| {
        scheduler
            .snapshot()
            .operations
            .iter()
            .any(|op| op.id == start.operation_id && op.done)
    })
    .await;
    assert!(scheduler.snapshot().feed_pause.is_none());
    assert!(
        proxy
            .received_requests()
            .await
            .unwrap()
            .iter()
            .all(|r| r.method == "GET")
    );
    let counters: Vec<(i64, i64)> = conn
        .prepare("SELECT fail_count,slow_fail_count FROM feeds ORDER BY id")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(counters, [(0, 0), (1, 1), (1, 1), (1, 1)]);
    assert!(scheduler.shutdown().await);
}

#[tokio::test]
async fn without_recent_successes_only_head_fallbacks_are_used() {
    let server = http_fixture(None, true).await;
    let (_dir, path) = fixture();
    let conn = db::open_at(&path).unwrap();
    seed_sources(&conn, 80, false);
    let dns = Arc::new(TestDns {
        available: Arc::new(AtomicBool::new(false)),
        address: server.address,
        fallback_only: true,
        calls: Arc::new(Mutex::new(Vec::new())),
    });
    let clients = NetworkClients::with_feed_builder(&path, move |builder| {
        builder.dns_resolver(dns.clone()).pool_max_idle_per_host(0)
    });
    let scheduler = Scheduler::with_limits(path.clone(), clients, |_| {}, limits());
    let start = scheduler.start_refresh().await.unwrap();
    wait_until(|| {
        scheduler
            .snapshot()
            .operations
            .iter()
            .any(|op| op.id == start.operation_id && op.done)
    })
    .await;
    assert!(scheduler.snapshot().feed_pause.is_none());
    let requests = server.requests.lock().unwrap().clone();
    assert!(!requests.is_empty());
    assert!(requests.iter().all(|r| r.method == "HEAD"
        && matches!(
            r.host.as_str(),
            "captive.apple.com" | "www.msftconnecttest.com"
        )));
    assert!(
        health(&conn)
            .iter()
            .all(|state| state.0 == 3 && state.2 == 0)
    );
    assert!(scheduler.shutdown().await);
}

#[tokio::test]
async fn isolated_dns_failure_is_recorded_after_a_reachable_probe_and_keeps_base_interval() {
    // One dead domain while the network works: without any recent success the
    // failure triggers a HEAD probe at once; a reachable target releases it.
    let server = http_fixture(None, true).await;
    let (_dir, path) = fixture();
    let conn = db::open_at(&path).unwrap();
    conn.execute(
        "INSERT INTO feeds(id,url,created_at) VALUES(1,'http://one.invalid/feed',0)",
        [],
    )
    .unwrap();
    let dns = Arc::new(TestDns {
        available: Arc::new(AtomicBool::new(false)),
        address: server.address,
        fallback_only: true,
        calls: Arc::new(Mutex::new(Vec::new())),
    });
    let clients =
        NetworkClients::with_feed_builder(&path, move |builder| builder.dns_resolver(dns.clone()));
    let mut policy = limits();
    policy.offline.window = Duration::from_millis(100);
    let scheduler = Scheduler::with_limits(path.clone(), clients, |_| {}, policy);
    for _ in 0..2 {
        let result =
            tokio::time::timeout(Duration::from_secs(10), scheduler.submit(1).unwrap().wait())
                .await
                .unwrap()
                .unwrap();
        assert!(matches!(result.status, RefreshStatus::Failed { .. }));
        let feed = rrss_core::feeds::get_feed(&conn, 1).unwrap();
        assert_eq!(feed.slow_fail_count, 0);
        assert!(
            (3_600_000..=3_960_000).contains(&(feed.next_fetch_at - feed.last_fetched_at.unwrap()))
        );
    }
    let requests = server.requests.lock().unwrap().clone();
    assert!(!requests.is_empty(), "the failure must be probed, not held");
    assert!(requests.iter().all(|r| r.method == "HEAD"
        && matches!(
            r.host.as_str(),
            "captive.apple.com" | "www.msftconnecttest.com"
        )));
    assert!(scheduler.snapshot().feed_pause.is_none());
    assert!(scheduler.shutdown().await);
}

#[tokio::test]
async fn one_previously_successful_dead_host_does_not_pause_all_subscriptions() {
    let server = http_fixture(None, true).await;
    let (_dir, path) = fixture();
    let conn = db::open_at(&path).unwrap();
    conn.execute(
        "INSERT INTO feeds(id,url,created_at,last_success_at) VALUES(1,'http://one.invalid/feed',0,?1)",
        [now_ms() - 600_000],
    ).unwrap();
    let dns = Arc::new(TestDns {
        available: Arc::new(AtomicBool::new(false)),
        address: server.address,
        fallback_only: true,
        calls: Arc::new(Mutex::new(Vec::new())),
    });
    let clients =
        NetworkClients::with_feed_builder(&path, move |builder| builder.dns_resolver(dns.clone()));
    let scheduler = Scheduler::with_limits(path.clone(), clients, |_| {}, limits());
    let result = tokio::time::timeout(Duration::from_secs(10), scheduler.submit(1).unwrap().wait())
        .await
        .unwrap()
        .unwrap();
    let pause = scheduler.snapshot().feed_pause;
    assert!(scheduler.shutdown().await);
    assert!(
        matches!(result.status, RefreshStatus::Failed { .. }),
        "{result:?}"
    );
    assert!(pause.is_none(), "one dead host is not a network outage");
    assert_eq!(health(&conn)[0].0, 1);
}
