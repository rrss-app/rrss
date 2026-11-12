//! Small capacity guards run by default; larger synthetic scenarios use --ignored
//! --nocapture; S3_REPORT lines contain numeric fixtures and measured results.
//! These exercise backend paths, not native desktop rendering or real services.
use super::*;
use serde_json::json;
use std::time::Instant;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn small_multi_host_refresh_respects_capacity_peaks() {
    const SOURCES: usize = 100;
    const HOSTS: usize = 4;
    let mut servers = Vec::new();
    for _ in 0..HOSTS {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string(RSS))
            .mount(&server)
            .await;
        servers.push(server);
    }
    let (_dir, path) = fixture();
    let conn = db::open_at(&path).unwrap();
    let tx = db::write_transaction(&conn).unwrap();
    for id in 0..SOURCES {
        tx.execute(
            "INSERT INTO feeds(url,created_at) VALUES(?1,0)",
            [format!("{}/{id}", servers[id % HOSTS].uri())],
        )
        .unwrap();
    }
    tx.commit().unwrap();
    let scheduler = Scheduler::with_limits(
        path.clone(),
        NetworkClients::new(&path),
        |_| {},
        runtime::Limits {
            global: 4,
            host: 2,
            parsing: 2,
            pending: 16,
            buffer_bytes: 4 * RSS.len(),
            policy: rrss_core::ingest::feed_http::FeedPolicy {
                max_body_bytes: RSS.len(),
                ..Default::default()
            },
            ..Default::default()
        },
    );
    let operation = scheduler.start_refresh().await.unwrap();
    let result = wait_operation(&scheduler, operation.operation_id).await;
    assert_eq!(
        (result.completed, result.failed, result.skipped),
        (SOURCES, 0, 0)
    );
    let feed = scheduler.diagnostics().feeds;
    assert!(feed.http_peak > 0 && feed.http_peak <= feed.http_limit);
    assert!(feed.host_peak > 0 && feed.host_peak <= feed.host_limit);
    assert!(feed.parsing_peak > 0 && feed.parsing_peak <= feed.parsing_limit);
    assert!(feed.peak_admitted > 0 && feed.peak_admitted <= feed.admission_limit);
    assert!(feed.buffered_bytes_peak > 0 && feed.buffered_bytes_peak <= feed.buffer_envelope);
    assert!(scheduler.shutdown().await);
    let stopped = scheduler.diagnostics().feeds;
    assert_eq!(stopped.http_active, 0);
    assert_eq!(stopped.parsing, 0);
    assert_eq!(stopped.buffered_bytes, 0);
    assert_eq!(stopped.buffered_bodies, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn small_body_backpressure_respects_envelope_and_releases_on_shutdown() {
    const BODY: usize = 4096;
    large_bodies_and_shutdown(
        32,
        BODY,
        runtime::Limits {
            global: 4,
            host: 2,
            parsing: 0,
            buffer_bytes: 4 * BODY,
            policy: rrss_core::ingest::feed_http::FeedPolicy {
                max_body_bytes: BODY,
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .await;
}

fn distribution(samples: &[f64]) -> serde_json::Value {
    let mut values = samples.to_vec();
    values.sort_by(f64::total_cmp);
    let percentile = |percent: usize| {
        values
            .get((values.len() * percent).div_ceil(100).saturating_sub(1))
            .copied()
            .unwrap_or(0.0)
    };
    json!({"samples":values.len(), "p50_ms":percentile(50), "p95_ms":percentile(95),
        "p99_ms":percentile(99), "max_ms":values.last().copied().unwrap_or(0.0)})
}

#[derive(Default)]
struct Interactions {
    list: Vec<f64>,
    reader: Vec<f64>,
    read: Vec<f64>,
}
impl Interactions {
    async fn sample(&mut self, path: &std::path::Path, index: usize) {
        let start = Instant::now();
        crate::jobs::spawn_db(path.to_owned(), |conn| {
            crate::queries::focus_page(
                conn,
                None,
                &[],
                &[],
                false,
                rrss_core::feed_scope::FeedScope::All,
            )
        })
        .await
        .unwrap();
        self.list.push(start.elapsed().as_secs_f64() * 1000.0);
        let article = 1 + (index % 5000) as i64;
        let start = Instant::now();
        let view = crate::jobs::spawn_db(path.to_owned(), move |conn| {
            crate::queries::reader_view(conn, article)
        })
        .await
        .unwrap();
        assert_eq!(view.id, article);
        self.reader.push(start.elapsed().as_secs_f64() * 1000.0);
        let start = Instant::now();
        crate::jobs::spawn_db(path.to_owned(), move |conn| {
            crate::actions::mark_read(conn, article, index.is_multiple_of(2))
        })
        .await
        .unwrap();
        self.read.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    fn report(&self) -> serde_json::Value {
        json!({"focus_list":distribution(&self.list),"open_article":distribution(&self.reader),"mark_read":distribution(&self.read)})
    }
}

struct FeedReply {
    delay: Duration,
}
impl wiremock::Respond for FeedReply {
    fn respond(&self, request: &wiremock::Request) -> ResponseTemplate {
        let id: usize = request.url.path().trim_start_matches('/').parse().unwrap();
        if !id.is_multiple_of(5) {
            return ResponseTemplate::new(304).set_delay(self.delay);
        }
        // Distinct synthetic items; neither source addresses nor body text enter reports.
        ResponseTemplate::new(200).set_delay(self.delay).set_body_string(format!(
            "<rss version='2.0'><channel><title>Scale</title><link>https://fixture.test</link><description>fixture</description><item><guid>new-{id}</guid><title>New {id}</title><description>synthetic body {id}</description></item></channel></rss>"))
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "explicit synthetic scale/interaction benchmark; no real network or AI"]
async fn scheduler_scale_mixed_sources_and_interactions() {
    for cluster_upgrade in [false, true] {
        mixed_sources(cluster_upgrade).await;
    }
}

async fn mixed_sources(cluster_upgrade: bool) {
    const SOURCES: usize = 1200;
    const RETAINED: usize = 5000;
    let slow = MockServer::start().await;
    let fast = MockServer::start().await;
    let other = MockServer::start().await;
    let limited = MockServer::start().await;
    for (server, delay) in [(&slow, 40), (&fast, 2), (&other, 2)] {
        Mock::given(method("GET"))
            .respond_with(FeedReply {
                delay: Duration::from_millis(delay),
            })
            .mount(server)
            .await;
    }
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "60"))
        .mount(&limited)
        .await;
    let (_dir, path) = fixture();
    let conn = db::open_at(&path).unwrap();
    db::set_setting(&conn, "pi_auto_download", "0").unwrap();
    let tx = db::write_transaction(&conn).unwrap();
    for id in 1..=SOURCES {
        let base = if id <= 200 {
            slow.uri()
        } else if id <= 300 {
            limited.uri()
        } else if id.is_multiple_of(2) {
            fast.uri()
        } else {
            other.uri()
        };
        tx.execute(
            "INSERT INTO feeds(id,url,title,created_at) VALUES(?1,?2,'fixture',?3)",
            rusqlite::params![id, format!("{base}/{id}"), now_ms()],
        )
        .unwrap();
    }
    // Retained items are already clustered; setup is excluded from the timings.
    tx.execute("WITH RECURSIVE n(id) AS (SELECT 1 UNION ALL SELECT id+1 FROM n WHERE id<?1)
        INSERT INTO clusters(id,label,representative_id,member_count,created_at) SELECT id,'fixture',id,1,?2 FROM n",
        rusqlite::params![RETAINED, now_ms()]).unwrap();
    tx.execute("WITH RECURSIVE n(id) AS (SELECT 1 UNION ALL SELECT id+1 FROM n WHERE id<?1)
        INSERT INTO articles(id,feed_id,guid,title,content_html,hash,score,simhash,cluster_id,created_at,published_at)
        SELECT id,1,'retained-'||id,'Retained '||id,'<p>Synthetic reading fixture.</p>','old-'||id,10,0,id,?2,?2 FROM n",
        rusqlite::params![RETAINED, now_ms()]).unwrap();
    tx.commit().unwrap();
    rrss_core::rank::store::rebuild(&conn, now_ms()).unwrap();
    db::set_setting(
        &conn,
        rrss_core::cluster::ALGORITHM_VERSION_KEY,
        if cluster_upgrade {
            "synthetic-previous-version"
        } else {
            rrss_core::cluster::ALGORITHM_VERSION
        },
    )
    .unwrap();
    let scheduler = Scheduler::start(path.clone(), NetworkClients::new(&path), |_| {});
    let mut baseline = Interactions::default();
    for i in 0..30 {
        baseline.sample(&path, i).await;
    }
    let start = Instant::now();
    let operation = scheduler.start_refresh().await.unwrap();
    let acknowledgment_ms = start.elapsed().as_secs_f64() * 1000.0;
    let mut loaded = Interactions::default();
    let mut fast_progress_while_slow = false;
    let mut progress_at = Instant::now();
    let completed = tokio::time::timeout(Duration::from_secs(90), async {
        loop {
            let done = scheduler
                .snapshot()
                .operations
                .iter()
                .any(|op| op.id == operation.operation_id && op.done);
            if done {
                break;
            }
            loaded.sample(&path, loaded.list.len()).await;
            if progress_at.elapsed() >= Duration::from_secs(10) {
                let status = scheduler.snapshot();
                let diagnostics = scheduler.diagnostics();
                println!("S3_PROGRESS {}", json!({"completed":status.operations[0].completed,
                    "feeds":diagnostics.feeds,"background":diagnostics.background,"timings":diagnostics.timings}));
                progress_at = Instant::now();
            }

            if !fast_progress_while_slow {
                fast_progress_while_slow = crate::jobs::spawn_db(path.clone(), |conn| {
                    let fast: i64 = conn.query_row(
                        "SELECT count(*) FROM feeds WHERE id>300 AND last_success_at IS NOT NULL",
                        [],
                        |r| r.get(0),
                    )?;
                    let slow: i64 = conn.query_row(
                        "SELECT count(*) FROM feeds WHERE id<=200 AND last_success_at IS NOT NULL",
                        [],
                        |r| r.get(0),
                    )?;
                    Ok(fast > 0 && slow < 200)
                })
                .await
                .unwrap();
            }
            tokio::time::sleep(Duration::from_millis(15)).await;
        }
    })
    .await;
    if completed.is_err() {
        println!(
            "S3_STALLED {}",
            serde_json::to_string(&scheduler.diagnostics()).unwrap()
        );
    }
    completed.expect("finite fake-service burst must settle");
    let elapsed = start.elapsed().as_secs_f64();
    let result = wait_operation(&scheduler, operation.operation_id).await;
    let snapshot = scheduler.diagnostics();
    assert_eq!(result.completed, SOURCES);
    assert_eq!(result.failed, 0);
    assert_eq!(result.skipped, 100);
    assert_eq!(result.inserted, 220);
    assert!(
        fast_progress_while_slow,
        "a concentrated slow host must not hide healthy hosts"
    );
    assert!(
        loaded.list.len() >= 10,
        "interaction distribution needs multiple concurrent samples"
    );
    assert!(snapshot.feeds.peak_admitted <= snapshot.feeds.admission_limit);
    assert!(snapshot.feeds.http_peak <= snapshot.feeds.http_limit);
    assert!(snapshot.feeds.host_peak <= snapshot.feeds.host_limit);
    assert!(snapshot.feeds.parsing_peak <= snapshot.feeds.parsing_limit);
    assert!(snapshot.feeds.buffered_bytes_peak <= snapshot.feeds.buffer_envelope);
    assert!(limited.received_requests().await.unwrap().len() <= snapshot.feeds.host_limit);
    assert_eq!(
        conn.query_row("SELECT count(*) FROM articles", [], |r| r
            .get::<_, usize>(0))
            .unwrap(),
        RETAINED + 220
    );
    println!(
        "S3_REPORT {}",
        json!({"scenario":"mixed_sources", "sources":SOURCES,"retained_articles":RETAINED,
        "arrival":"single_due_burst", "cluster_upgrade_pending":cluster_upgrade,
        "retained_ranking_prepared":true,"new_articles":result.inserted,"daily_arrival_assumption":null,
        "vector_dimensions":0,"vector_chunks":0,"fake_feed_delay_ms":{"slow":40,"healthy":2},
        "runtime_workers":4,"available_parallelism":std::thread::available_parallelism().map(|n|n.get()).unwrap_or(1),
        "debug_build":cfg!(debug_assertions),"elapsed_s":elapsed,"sources_per_s":SOURCES as f64 / elapsed,
        "articles_per_s":result.inserted as f64 / elapsed,"command_ack_ms":acknowledgment_ms,
        "idle_interactions":baseline.report(),"loaded_interactions":loaded.report(),"diagnostics":snapshot})
    );
    assert!(scheduler.shutdown().await);
    assert_eq!(scheduler.diagnostics().feeds.buffered_bytes, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "explicit synthetic byte-backpressure benchmark"]
async fn scheduler_scale_large_bodies_and_shutdown() {
    large_bodies_and_shutdown(
        160,
        1024 * 1024,
        runtime::Limits {
            parsing: 0,
            ..Default::default()
        },
    )
    .await;
}

async fn large_bodies_and_shutdown(sources: usize, body_bytes: usize, limits: runtime::Limits) {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![b'x'; body_bytes]))
        .mount(&server)
        .await;
    let (_dir, path) = fixture();
    let conn = db::open_at(&path).unwrap();
    let tx = db::write_transaction(&conn).unwrap();
    for id in 1..=sources {
        tx.execute(
            "INSERT INTO feeds(id,url,created_at) VALUES(?1,?2,0)",
            rusqlite::params![id, format!("{}/{id}", server.uri())],
        )
        .unwrap();
    }
    tx.commit().unwrap();
    let scheduler =
        Scheduler::with_limits(path.clone(), NetworkClients::new(&path), |_| {}, limits);
    scheduler.start_refresh().await.unwrap();
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let feed = scheduler.diagnostics().feeds;
            // A finished download leaves the in-flight gauge before the coordinator
            // moves its body into the buffer; wait until both counters agree.
            if feed.buffered_bytes >= feed.buffer_high_water
                && feed.downloads == 0
                && feed.buffered_bodies * body_bytes == feed.buffered_bytes
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let blocked = scheduler.diagnostics();
    let before = server.received_requests().await.unwrap().len();
    assert!(before < sources);
    assert!(blocked.feeds.queued > 0);
    assert!(blocked.feeds.buffered_bodies >= limits.buffer_bytes.div_ceil(body_bytes));
    let added = scheduler
        .add_feed(&format!("{}/added", server.uri()))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let feed = scheduler.diagnostics().feeds;
            if feed.buffered_bytes >= blocked.feeds.buffered_bytes + body_bytes
                && feed.downloads == 0
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let after = scheduler.diagnostics();
    assert_eq!(server.received_requests().await.unwrap().len(), before + 1);
    assert!(after.feeds.buffered_bytes_peak <= after.feeds.buffer_envelope);
    assert!(
        after.feeds.http_peak <= after.feeds.http_limit
            && after.feeds.host_peak <= after.feeds.host_limit
    );
    assert!(scheduler.shutdown().await);
    assert_eq!(scheduler.diagnostics().feeds.buffered_bytes, 0);
    assert_eq!(scheduler.diagnostics().feeds.http_active, 0);
    assert!(
        rrss_core::feeds::get_feed(&conn, added.id)
            .unwrap()
            .last_fetched_at
            .is_none()
    );
    println!(
        "S3_REPORT {}",
        json!({"scenario":"byte_backpressure","sources":sources,"body_bytes":body_bytes,
        "parsing":"intentionally_paused","diagnostics":after,"released_on_shutdown":true})
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "explicit many-origin concurrency benchmark"]
async fn scheduler_scale_many_origins_saturate_global_capacity() {
    const SOURCES: usize = 1200;
    const HOSTS: usize = 40;
    let mut servers = Vec::new();
    for _ in 0..HOSTS {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(304).set_delay(Duration::from_millis(20)))
            .mount(&server)
            .await;
        servers.push(server);
    }
    let (_dir, path) = fixture();
    let conn = db::open_at(&path).unwrap();
    let tx = db::write_transaction(&conn).unwrap();
    for id in 0..SOURCES {
        tx.execute(
            "INSERT INTO feeds(url,created_at) VALUES(?1,0)",
            [format!("{}/{id}", servers[id % HOSTS].uri())],
        )
        .unwrap();
    }
    tx.commit().unwrap();
    let scheduler = Scheduler::start(path.clone(), NetworkClients::new(&path), |_| {});
    let start = Instant::now();
    let operation = scheduler.start_refresh().await.unwrap();
    let result = wait_operation(&scheduler, operation.operation_id).await;
    let elapsed = start.elapsed().as_secs_f64();
    let diagnostics = scheduler.diagnostics();
    assert_eq!(
        (result.completed, result.failed, result.skipped),
        (SOURCES, 0, 0)
    );
    assert_eq!(
        diagnostics.feeds.http_peak, diagnostics.feeds.http_limit,
        "exercise the production global cap, not just a few busy origins"
    );
    assert!(diagnostics.feeds.host_peak <= diagnostics.feeds.host_limit);
    assert!(diagnostics.feeds.peak_admitted <= diagnostics.feeds.admission_limit);
    assert_eq!(diagnostics.feeds.buffered_bytes_peak, 0);
    let mut requests = 0;
    for server in &servers {
        requests += server.received_requests().await.unwrap().len();
    }
    assert_eq!(requests, SOURCES);
    println!(
        "S3_REPORT {}",
        json!({"scenario":"many_origins", "sources":SOURCES, "origins":HOSTS,
        "fake_feed_delay_ms":20, "elapsed_s":elapsed,"sources_per_s":SOURCES as f64/elapsed,"diagnostics":diagnostics})
    );
    assert!(scheduler.shutdown().await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "explicit fake-model priority/latency benchmark"]
async fn scheduler_scale_model_interactive_priority() {
    use model_capacity::{Priority, RequestAdmission};
    use rrss_ai::admission::Admission;
    const BACKGROUND: usize = 32;
    const INTERACTIVE: usize = 4;
    let server = MockServer::start().await;
    Mock::given(method("POST")).respond_with(ResponseTemplate::new(200).set_delay(Duration::from_millis(40))
        .set_body_json(json!({"choices":[{"message":{"content":"{}"}}],
            "data":[{"index":0,"embedding":vec![0.1f32;64]}],"usage":{"prompt_tokens":8,"completion_tokens":2}}))).mount(&server).await;
    let (_dir, path) = fixture();
    let scheduler = Scheduler::start(path.clone(), NetworkClients::new(&path), |_| {});
    let client = scheduler
        .shared
        .network
        .client(rrss_core::network::NetworkScope::Api)
        .await
        .unwrap();
    // Hold one existing request's slot until all contenders are demonstrably
    // queued. No sleep-based race decides whether an interactive request wins.
    let gate = RequestAdmission::new(&path, &server.uri(), Priority::Background, false).unwrap();
    let held = gate.acquire().await.unwrap();
    let mut jobs = tokio::task::JoinSet::new();
    for index in 0..BACKGROUND + INTERACTIVE {
        let interactive = index >= BACKGROUND;
        let path = path.clone();
        let client = client.clone();
        let base = server.uri();
        jobs.spawn(async move {
            let start = Instant::now();
            let admission = RequestAdmission::new(
                &path,
                &base,
                if interactive {
                    Priority::Interactive
                } else {
                    Priority::Background
                },
                false,
            )
            .unwrap();
            let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
            if !interactive && index.is_multiple_of(2) {
                rrss_ai::embed::embed_batch_admitted_until(
                    &client,
                    &rrss_ai::EmbedBatchRequest {
                        base_url: base,
                        api_key: None,
                        model: "fake".into(),
                        inputs: vec!["background".into()],
                    },
                    deadline,
                    Some(&admission),
                )
                .await
                .unwrap();
            } else {
                rrss_ai::complete::complete_admitted_until(
                    &client,
                    &rrss_ai::CompleteRequest {
                        base_url: base,
                        api_key: None,
                        model: "fake".into(),
                        system: "fixture".into(),
                        user: if interactive {
                            "interactive"
                        } else {
                            "background"
                        }
                        .into(),
                        json_schema: None,
                        protocol: rrss_ai::Protocol::Chat,
                    },
                    None,
                    &|_, _| {},
                    deadline,
                    Some(&admission),
                )
                .await
                .unwrap();
            }
            (interactive, start.elapsed().as_secs_f64() * 1000.0)
        });
    }
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let model = scheduler.diagnostics().models;
            if model.waiting_background == BACKGROUND && model.waiting_interactive == INTERACTIVE {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    drop(held);
    let mut interactive = Vec::new();
    let mut background = Vec::new();
    while let Some(result) = jobs.join_next().await {
        let (priority, elapsed) = result.unwrap();
        if priority {
            interactive.push(elapsed);
        } else {
            background.push(elapsed);
        }
    }
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), BACKGROUND + INTERACTIVE);
    assert!(requests[..INTERACTIVE].iter().all(|request| {
        request.body_json::<serde_json::Value>().unwrap()["messages"][1]["content"] == "interactive"
    }));
    let snapshot = scheduler.diagnostics();
    assert_eq!(snapshot.models.peak_active, 1);
    assert_eq!(snapshot.models.active, 0);
    assert_eq!(
        snapshot.models.waiting_background + snapshot.models.waiting_interactive,
        0
    );
    println!(
        "S3_REPORT {}",
        json!({"scenario":"model_priority","background_requests":BACKGROUND,
        "interactive_requests":INTERACTIVE,"fake_model_delay_ms":40,"vector_dimensions":64,"embedding_inputs":BACKGROUND/2,
        "interactive":distribution(&interactive),"background":distribution(&background),"diagnostics":snapshot.models})
    );
    assert!(scheduler.shutdown().await);
}
