//! Explicit offline latency probe: real app queries, disposable database, no AI or downloads.
//! Run with `cargo test -p rrss-app --test db_latency -- --ignored --nocapture`.
//! The injected lock scenario demonstrates contention; it does not identify a production holder.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use rrss_app::{actions, execution, queries, window_close};
use rrss_core::{db, feed_scope::FeedScope};
use serde_json::{Value, json};

// Exercise the actual synchronous media implementation without requiring a Tauri build.
#[path = "../../rrss-desktop/src/protocol.rs"]
mod desktop_protocol;

const ARTICLES: usize = 5_000;
const SAMPLES: usize = 200;
// Mirrors the current pool size for this explicit saturation experiment.
// Acquisition barriers fail with a timeout if the pool becomes smaller.
const POOL_CONNECTIONS: usize = 8;
const HOLD: Duration = Duration::from_millis(500);

#[test]
fn cached_media_can_be_served_before_a_concurrent_writer_commits() {
    let (dir, path) = fixture_with_articles(1);
    let conn = db::open_at(&path).unwrap();
    let file = dir.path().join("fixture.svg");
    std::fs::write(&file, "<svg xmlns=\"http://www.w3.org/2000/svg\"/>").unwrap();
    conn.execute(
        "INSERT INTO media(id,article_id,url,local_path,mime,kind,bytes,created_at,accessed_at)
        VALUES(1,1,'https://latency.example/image.svg',?1,'image/svg+xml','image',40,1,1)",
        [file.to_str().unwrap()],
    )
    .unwrap();
    let held = db::write_transaction(&conn).unwrap();
    let (send, receive) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        let payload = desktop_protocol::media_payload(1, &path, None).unwrap();
        send.send(payload).unwrap();
    });
    let result = receive.recv_timeout(Duration::from_secs(2));
    held.rollback().unwrap();
    reader.join().unwrap();
    let payload = result.expect("cached response must not depend on writer completion");
    assert_eq!(payload.status, 200);
    assert!(!payload.body.is_empty());
    rrss_core::media::flush_accesses(&conn).unwrap();
    assert!(
        conn.query_row("SELECT accessed_at FROM media WHERE id=1", [], |row| row
            .get::<_, i64>(0))
            .unwrap()
            > 1
    );
}

#[derive(Clone, Copy)]
enum Operation {
    CloseSetting,
    SetSetting,
    Focus,
    Reader,
    MarkRead,
}

impl Operation {
    fn name(self) -> &'static str {
        match self {
            Self::CloseSetting => "close_setting",
            Self::SetSetting => "set_setting",
            Self::Focus => "focus_page",
            Self::Reader => "reader_view",
            Self::MarkRead => "mark_read",
        }
    }

    fn run(self, conn: &rusqlite::Connection, index: usize) -> rrss_core::Result<()> {
        match self {
            Self::CloseSetting => {
                assert_eq!(window_close::action(conn)?, window_close::CloseAction::Ask);
            }
            Self::SetSetting => window_close::remember(
                conn,
                if index.is_multiple_of(2) {
                    window_close::CloseChoice::Tray
                } else {
                    window_close::CloseChoice::Quit
                },
            )?,
            Self::Focus => {
                let page = queries::focus_page(conn, None, &[], &[], false, FeedScope::All)?;
                assert!(!page.articles.is_empty());
            }
            Self::Reader => {
                let id = 1 + (index % ARTICLES) as i64;
                assert_eq!(queries::reader_view(conn, id)?.id, id);
            }
            Self::MarkRead => actions::mark_read(conn, 1, index.is_multiple_of(2))?,
        }
        Ok(())
    }
}

#[derive(Default)]
struct Sample {
    total_ms: f64,
    worker_queue_ms: f64,
    connection_ms: f64,
    operation_ms: f64,
}

fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

async fn sample(path: PathBuf, operation: Operation, index: usize) -> Sample {
    let started = Instant::now();
    // The same blocking/pool/app boundaries as execution::core_db and desktop state::with_db.
    // Only timing is added here; database and business behavior are production implementations.
    let mut sample = execution::blocking(move || -> rrss_core::Result<Sample> {
        let worker_queue_ms = ms(started.elapsed());
        let acquiring = Instant::now();
        let conn = db::open_at(&path)?;
        let connection_ms = ms(acquiring.elapsed());
        let running = Instant::now();
        operation.run(&conn, index)?;
        Ok(Sample {
            worker_queue_ms,
            connection_ms,
            operation_ms: ms(running.elapsed()),
            ..Sample::default()
        })
    })
    .await
    .unwrap()
    .unwrap();
    sample.total_ms = ms(started.elapsed());
    sample
}

fn distribution(values: impl Iterator<Item = f64>) -> Value {
    let mut values = values.collect::<Vec<_>>();
    assert!(!values.is_empty());
    values.sort_by(f64::total_cmp);
    let percentile = |p: usize| values[(values.len() * p).div_ceil(100) - 1];
    json!({
        "samples": values.len(), "p50_ms": percentile(50), "p95_ms": percentile(95),
        "p99_ms": percentile(99), "max_ms": values.last().unwrap(),
    })
}

fn report(samples: &[Sample]) -> Value {
    let mut total = distribution(samples.iter().map(|sample| sample.total_ms));
    total["over_100ms"] = json!(
        samples
            .iter()
            .filter(|sample| sample.total_ms > 100.0)
            .count()
    );
    json!({
        "total": total,
        "worker_queue": distribution(samples.iter().map(|s| s.worker_queue_ms)),
        "connection": distribution(samples.iter().map(|s| s.connection_ms)),
        // Includes implicit SQLite lock waits; not a measurement of pure SQL execution.
        "operation_including_sqlite_wait": distribution(samples.iter().map(|s| s.operation_ms)),
    })
}

fn fixture() -> (tempfile::TempDir, PathBuf) {
    fixture_with_articles(ARTICLES)
}

fn fixture_with_articles(articles: usize) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("latency.db");
    let conn = db::open_at(&path).unwrap();
    let tx = db::write_transaction(&conn).unwrap();
    let now = rrss_core::feeds::now_ms();
    tx.execute(
        "INSERT INTO feeds(id,url,title,created_at) VALUES(1,'https://latency.example/feed','Fixture',?1)",
        [now],
    ).unwrap();
    tx.execute(
        "WITH RECURSIVE n(id) AS (SELECT 1 UNION ALL SELECT id+1 FROM n WHERE id<?1)
         INSERT INTO clusters(id,label,representative_id,member_count,created_at)
         SELECT id,'Fixture',id,1,?2 FROM n",
        rusqlite::params![articles, now],
    )
    .unwrap();
    tx.execute(
        "WITH RECURSIVE n(id) AS (SELECT 1 UNION ALL SELECT id+1 FROM n WHERE id<?1)
         INSERT INTO articles(id,feed_id,guid,title,content_html,hash,score,simhash,cluster_id,created_at,published_at)
         SELECT id,1,'fixture-'||id,'Fixture article '||id,'<p>Synthetic reading fixture.</p>',
         'hash-'||id,10,0,id,?2,?2 FROM n",
        rusqlite::params![articles, now],
    ).unwrap();
    db::set_setting(&tx, "pi_auto_download", "0").unwrap();
    db::set_setting(&tx, window_close::SETTING, "ask").unwrap();
    tx.commit().unwrap();
    rrss_focus::store::rebuild(&conn, now).unwrap();
    assert_eq!(
        conn.pragma_query_value(None, "journal_mode", |r| r.get::<_, String>(0))
            .unwrap(),
        "wal"
    );
    assert_eq!(
        conn.pragma_query_value(None, "synchronous", |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    drop(conn);
    // Exclude physical-connection initialization from the warm contention measurements.
    let warm = (0..POOL_CONNECTIONS)
        .map(|_| db::open_at(&path).unwrap())
        .collect::<Vec<_>>();
    drop(warm);
    (dir, path)
}

async fn contention(path: &Path, waiters: usize, index: usize) -> (Sample, Sample, f64) {
    let (locked_tx, locked_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let holder_path = path.to_owned();
    let holder = tokio::spawn(execution::core_db(holder_path, move |conn| {
        let tx = db::write_transaction(conn)?;
        let started = Instant::now();
        locked_tx.send(()).unwrap();
        // A finite watchdog prevents a failed acquisition barrier from leaving a held lock.
        let _ = release_rx.recv_timeout(Duration::from_secs(5));
        tx.commit()?;
        Ok(ms(started.elapsed()))
    }));
    locked_rx.await.unwrap();
    let (ready_tx, mut ready_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut writers = Vec::new();
    for i in 0..waiters {
        let path = path.to_owned();
        let ready = ready_tx.clone();
        writers.push(tokio::spawn(execution::core_db(path, move |conn| {
            // Signal only after a real pooled connection has been acquired.
            ready.send(()).unwrap();
            db::set_setting(conn, &format!("latency_fixture_{i}"), "saved")
        })));
    }
    drop(ready_tx);
    tokio::time::timeout(Duration::from_secs(2), async {
        for _ in 0..waiters {
            ready_rx.recv().await.unwrap();
        }
    })
    .await
    .expect("all writers must acquire a connection before the probe");
    let release = tokio::spawn(async move {
        tokio::time::sleep(HOLD).await;
        release_tx.send(()).unwrap();
    });
    let (read, write) = tokio::join!(
        sample(path.to_owned(), Operation::CloseSetting, index),
        sample(path.to_owned(), Operation::MarkRead, index),
    );
    release.await.unwrap();
    let held_ms = holder.await.unwrap().unwrap();
    for writer in writers {
        writer.await.unwrap().unwrap();
    }
    (read, write, held_ms)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "explicit offline latency baseline and controlled lock/pool contention"]
async fn database_latency_baseline() {
    let (_dir, path) = fixture();
    let mut idle = serde_json::Map::new();
    for operation in [
        Operation::CloseSetting,
        Operation::SetSetting,
        Operation::Focus,
        Operation::Reader,
        Operation::MarkRead,
    ] {
        for i in 0..10 {
            sample(path.clone(), operation, i).await;
        }
        let mut samples = Vec::new();
        for i in 0..SAMPLES {
            samples.push(sample(path.clone(), operation, i).await);
        }
        idle.insert(operation.name().into(), report(&samples));
        db::set_setting(&db::open_at(&path).unwrap(), window_close::SETTING, "ask").unwrap();
    }
    let mut contended = Vec::new();
    for waiters in [1, POOL_CONNECTIONS - 1] {
        let mut reads = Vec::new();
        let mut writes = Vec::new();
        let mut held = Vec::new();
        for i in 0..5 {
            let (read, write, hold) = contention(&path, waiters, i).await;
            reads.push(read);
            writes.push(write);
            held.push(hold);
        }
        contended.push(json!({
            "waiting_writers": waiters, "injected_hold_ms": ms(HOLD),
            "actual_holder_duration": distribution(held.into_iter()),
            "close_setting": report(&reads), "mark_read": report(&writes),
        }));
    }

    println!(
        "DB_LATENCY_REPORT {}",
        json!({
            "fixture": {"articles": ARTICLES, "vectors": 0, "journal_mode": "WAL", "synchronous": "NORMAL"},
            "debug_build": cfg!(debug_assertions), "runtime_workers": 4,
            "scope": "app blocking worker, pooled connection and real app operation; excludes IPC and rendering",
            "idle": idle, "controlled_contention": contended,
        })
    );
}

#[test]
#[ignore = "explicit legacy preparation reference for comparison with the paged production path"]
fn rebuild_preparation_baseline() {
    let (_dir, path) = fixture();
    let conn = db::open_at(&path).unwrap();
    conn.execute("UPDATE articles SET read=1, embed_tier=2", [])
        .unwrap();
    let mut passes = Vec::new();
    for pass in 0..3 {
        let started = Instant::now();
        let tx = db::write_transaction(&conn).unwrap();
        let acquired = Instant::now();
        let loading = Instant::now();
        // Legacy whole-database preparation reference; production uses rag_prepare::prepare_page.
        // Measures preparation only, without model requests, staging or the final space switch.
        let articles = rrss_core::rag::rebuild_articles(&tx).unwrap();
        let load_ms = ms(loading.elapsed());
        let changes_before = tx.total_changes();
        let syncing = Instant::now();
        for article in &articles {
            rrss_core::rag::sync_article_chunks(&tx, article, article.tier).unwrap();
        }
        let sync_ms = ms(syncing.elapsed());
        let changes = tx.total_changes() - changes_before;
        let committing = Instant::now();
        tx.commit().unwrap();
        let commit_ms = ms(committing.elapsed());
        let transaction_ms = ms(acquired.elapsed());
        passes.push(json!({
            "pass": pass, "inputs_unchanged": pass > 0, "articles_scanned": articles.len(),
            "database_rows_changed": changes, "load_ms": load_ms, "sync_ms": sync_ms,
            "commit_ms": commit_ms, "transaction_ms_including_commit": transaction_ms,
            "total_ms": ms(started.elapsed()),
        }));
    }
    println!(
        "DB_PREPARATION_REPORT {}",
        json!({
            "articles": ARTICLES, "body": "short synthetic paragraph", "embedding_tier": 2,
            "vectors": 0, "debug_build": cfg!(debug_assertions),
            "journal_mode": "WAL", "synchronous": "NORMAL", "passes": passes,
        })
    );
}

#[test]
#[ignore = "explicit production paged preparation comparison, no model requests"]
fn paged_rebuild_preparation_latency() {
    let (_dir, path) = fixture();
    let conn = db::open_at(&path).unwrap();
    conn.execute_batch(
        "UPDATE articles SET read=1,embed_tier=2;
        INSERT INTO vector_spaces(id,base_url,model,dims,prefix_version,normalized,state,created_at)
        VALUES(1,'','fixture',3,1,0,'active',0),(2,'','fixture',3,1,0,'building',0);
        UPDATE vector_spaces SET rebuild_from=1 WHERE id=2;",
    )
    .unwrap();
    let mut passes = Vec::new();
    for pass in 0..3 {
        let started = Instant::now();
        let before = conn.total_changes();
        let mut pages = Vec::new();
        let mut articles = 0;
        loop {
            let start = Instant::now();
            let written = rrss_core::rag_prepare::prepare_page(&conn, 2).unwrap();
            pages.push(ms(start.elapsed()));
            articles += written;
            if !rrss_core::rag_prepare::pending(&conn).unwrap() {
                break;
            }
        }
        passes.push(json!({"pass":pass,"articles_prepared":articles,"database_rows_changed":conn.total_changes()-before,
            "page_ms":distribution(pages.into_iter()),"total_ms":ms(started.elapsed())}));
        assert_eq!(articles, if pass == 0 { ARTICLES } else { 0 });
    }
    println!(
        "DB_PAGED_PREPARATION_REPORT {}",
        json!({"articles":ARTICLES,"passes":passes,
        "debug_build":cfg!(debug_assertions),"journal_mode":"WAL","synchronous":"NORMAL"})
    );
}

#[test]
#[ignore = "explicit small-database probe of cached media blocking the async executor"]
fn cached_media_executor_latency() {
    let mut scenarios = Vec::new();
    for offload in [false, true] {
        let (dir, path) = fixture_with_articles(300);
        let conn = db::open_at(&path).unwrap();
        let image = dir.path().join("fixture.svg");
        std::fs::write(
            &image,
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"1\" height=\"1\"/>",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO media(id,article_id,url,local_path,mime,kind,bytes,created_at,accessed_at)
             VALUES(1,1,'https://latency.example/image.svg',?1,'image/svg+xml','image',64,1,1)",
            [image.to_str().unwrap()],
        )
        .unwrap();
        drop(conn);
        let (locked_tx, locked_rx) = std::sync::mpsc::channel();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let holder_path = path.clone();
        let holder = std::thread::spawn(move || {
            let conn = db::open_at(&holder_path).unwrap();
            let tx = db::write_transaction(&conn).unwrap();
            locked_tx.send(()).unwrap();
            for _ in 0..2 {
                ready_rx.recv_timeout(Duration::from_secs(3)).unwrap();
            }
            std::thread::sleep(Duration::from_millis(800));
            tx.commit().unwrap();
        });
        locked_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let started = Instant::now();
        let (heartbeat_ms, timeout_ms, media_ms) = runtime.block_on(async {
            let (armed_tx, armed_rx) = tokio::sync::oneshot::channel();
            let heartbeat = tokio::spawn(async move {
                let timer = tokio::time::sleep(Duration::from_millis(20));
                armed_tx.send(()).unwrap();
                timer.await;
                ms(started.elapsed())
            });
            armed_rx.await.unwrap();
            let timeout = tokio::spawn(async move {
                let _ =
                    tokio::time::timeout(Duration::from_millis(250), std::future::pending::<()>())
                        .await;
                ms(started.elapsed())
            });
            let mut readers = Vec::new();
            for _ in 0..2 {
                let path = path.clone();
                let ready = ready_tx.clone();
                readers.push(tokio::spawn(async move {
                    let read = move || {
                        ready.send(()).unwrap();
                        let payload = desktop_protocol::media_payload(1, &path, None).unwrap();
                        assert_eq!(payload.status, 200);
                        assert!(!payload.body.is_empty());
                        assert_eq!(payload.mime, "image/svg+xml");
                        assert!(payload.content_range.is_none());
                        assert!(!payload.accept_ranges);
                    };
                    if offload {
                        execution::blocking(read).await.unwrap();
                    } else {
                        // The current rrss:// handler runs this directly in async_runtime::spawn.
                        read();
                    }
                    ms(started.elapsed())
                }));
            }
            let mut times = Vec::new();
            for reader in readers {
                times.push(reader.await.unwrap());
            }
            (heartbeat.await.unwrap(), timeout.await.unwrap(), times)
        });
        holder.join().unwrap();
        scenarios.push(json!({
            "offload_to_blocking_worker": offload, "articles": 300,
            "cached_media_reads": 2, "async_workers": 2, "injected_write_hold_ms": 800,
            "heartbeat_target_ms": 20, "heartbeat_observed_ms": heartbeat_ms,
            "timeout_target_ms": 250, "timeout_observed_ms": timeout_ms,
            "media_completed_ms": media_ms,
        }));
    }
    println!("DB_MEDIA_EXECUTOR_REPORT {}", json!(scenarios));
}

#[test]
#[ignore = "explicit 300 retained / 30 new article baseline without injected contention"]
fn small_database_work_baseline() {
    let (_dir, path) = fixture_with_articles(300);
    let conn = db::open_at(&path).unwrap();
    let now = rrss_core::feeds::now_ms();
    let entries = (0..30)
        .map(|index| rrss_core::parse::ParsedEntry {
            guid: format!("new-{index}"),
            canonical_url: Some(format!("https://latency.example/new-{index}")),
            title: format!("New fixture article {index}"),
            author: None,
            published_at: Some(now),
            content_html: Some(format!("<p>Synthetic article content number {index}.</p>")),
            hash: format!("new-hash-{index}"),
            tags: Vec::new(),
        })
        .collect::<Vec<_>>();
    let ingest = rrss_core::ingest::upsert_entries_measured(&conn, 1, &entries, now).unwrap();
    assert_eq!(ingest.inserted, 30);
    let rank_started = Instant::now();
    rrss_focus::store::rebuild(&conn, now).unwrap();
    let rank_ms = ms(rank_started.elapsed());
    conn.execute("UPDATE articles SET read=1, embed_tier=2", [])
        .unwrap();
    let mut preparation_ms = Vec::new();
    for _ in 0..3 {
        let tx = db::write_transaction(&conn).unwrap();
        let started = Instant::now();
        for article in rrss_core::rag::rebuild_articles(&tx).unwrap() {
            rrss_core::rag::sync_article_chunks(&tx, &article, article.tier).unwrap();
        }
        tx.commit().unwrap();
        preparation_ms.push(ms(started.elapsed()));
    }
    println!(
        "DB_SMALL_WORK_REPORT {}",
        json!({
            "retained_articles":300, "new_articles":30, "debug_build":cfg!(debug_assertions),
            "journal_mode":"WAL", "synchronous":"NORMAL", "vectors":0,
            "ingest":{
                "prepare_ms":ms(ingest.prepare), "dedup_ms":ms(ingest.dedup),
                "write_ms":ms(ingest.write), "commit_ms":ms(ingest.commit),
                "cluster_ms":ms(ingest.cluster), "total_ms":ms(ingest.total),
            },
            "rank_rebuild_ms":rank_ms, "chunk_preparation_transactions_ms":preparation_ms,
        })
    );
}

#[test]
#[ignore = "explicit short-transaction contention probe; distinguishes lock hold from starvation"]
fn consecutive_short_writes_latency() {
    let mut reports = Vec::new();
    for round in 0..3 {
        let (_dir, path) = fixture_with_articles(300);
        let (locked_tx, locked_rx) = std::sync::mpsc::channel();
        let (probe_tx, probe_rx) = std::sync::mpsc::channel();
        let writer_path = path.clone();
        let writer = std::thread::spawn(move || {
            let conn = db::open_at(&writer_path).unwrap();
            let mut locked_tx = Some(locked_tx);
            let mut holds = Vec::new();
            for i in 0..100 {
                let tx = db::write_transaction(&conn).unwrap();
                if let Some(signal) = locked_tx.take() {
                    signal.send(()).unwrap();
                    probe_rx.recv_timeout(Duration::from_secs(3)).unwrap();
                }
                let start = Instant::now();
                db::set_setting(&tx, "latency_short_writes", &i.to_string()).unwrap();
                // Controlled 5 ms work per transaction; commits release the lock each time.
                std::thread::sleep(Duration::from_millis(5));
                tx.commit().unwrap();
                holds.push(ms(start.elapsed()));
            }
            holds
        });
        locked_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        let conn = db::open_at(&path).unwrap();
        let started = Instant::now();
        window_close::action(&conn).unwrap();
        let read_ms = ms(started.elapsed());
        let started = Instant::now();
        probe_tx.send(()).unwrap();
        actions::mark_read(&conn, 1, true).unwrap();
        let write_ms = ms(started.elapsed());
        let holds = writer.join().unwrap();
        reports.push(json!({
            "round":round, "articles":300, "background_transactions":holds.len(),
            "individual_hold":distribution(holds.into_iter()),
            "close_setting_ms":read_ms, "mark_read_ms":write_ms,
        }));
    }
    println!("DB_SHORT_WRITES_REPORT {}", json!(reports));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "explicit 300+30 mixed local workload with vectors, media, long bodies and cleanup"]
async fn small_mixed_background_latency() {
    let (dir, path) = fixture_with_articles(300);
    let conn = db::open_at(&path).unwrap();
    let now = rrss_core::feeds::now_ms();
    let body = format!(
        "<p>{}</p>",
        "Synthetic long-form content for latency measurement. ".repeat(400)
    );
    let vector = rrss_core::rag::f32s_to_le(&vec![1.0 / 32.0; 1024]);
    let tx = db::write_transaction(&conn).unwrap();
    tx.execute_batch("INSERT INTO vector_spaces(id,base_url,model,dims,source_dims,prefix_version,normalized,state,created_at)
        VALUES(1,'','fixture',1024,1024,1,1,'active',0),(2,'','fixture',1024,1024,1,1,'building',0);
        UPDATE vector_spaces SET rebuild_from=1 WHERE id=2;") .unwrap();
    tx.execute(
        "UPDATE articles SET read=1,embed_tier=2,content_html=?1 WHERE id<=30",
        [&body],
    )
    .unwrap();
    for id in 1..=300 {
        tx.execute(
            "INSERT INTO chunks(article_id,ordinal,kind,text,embedding,space_id)
            VALUES(?1,0,'title_summary','fixture',?2,1)",
            rusqlite::params![id, &vector],
        )
        .unwrap();
    }
    tx.execute(
        "INSERT INTO interests(label,weight,source,enabled,embedding,space_id,created_at)
        VALUES('fixture',2,'manual',1,?1,1,0)",
        [&vector],
    )
    .unwrap();
    tx.commit().unwrap();
    drop(conn);
    let network = rrss_app::network::NetworkClients::new(&path);
    let data = dir.path().to_owned();
    let background_path = path.clone();
    let background = tokio::spawn(async move {
        let cluster = rrss_app::jobs::run_cluster_jobs(&background_path);
        let interest = rrss_app::jobs::run_interest_boost(&background_path);
        let preparing = async {
            loop {
                let remaining = execution::core_db(background_path.clone(), |conn| {
                    rrss_core::rag_prepare::prepare_page(conn, 2)?;
                    rrss_core::rag_prepare::pending(conn)
                })
                .await
                .unwrap();
                if !remaining {
                    break;
                }
                tokio::task::yield_now().await;
            }
        };
        let ingest = async {
            let body = body.clone();
            execution::core_db(background_path.clone(), move |conn| {
                let entries = (301..=330)
                    .map(|id| rrss_core::parse::ParsedEntry {
                        guid: format!("new-{id}"),
                        canonical_url: Some(format!("https://latency.example/{id}")),
                        title: format!("New fixture {id}"),
                        author: None,
                        published_at: Some(now),
                        content_html: Some(body.clone()),
                        hash: format!("new-{id}"),
                        tags: Vec::new(),
                    })
                    .collect::<Vec<_>>();
                rrss_core::ingest::upsert_entries(conn, 1, &entries, now).map(|_| ())
            })
            .await
            .unwrap();
        };
        let media = async {
            for id in 1..=30 {
                let data = data.clone();
                execution::core_db(background_path.clone(), move |conn| {
                    rrss_core::media::store_image(
                        conn,
                        &data,
                        id,
                        &format!("https://media.example/{id}.svg"),
                        b"<svg/>",
                    )?;
                    if id % 5 == 0 {
                        rrss_core::media::flush_accesses(conn)?;
                    }
                    Ok(())
                })
                .await
                .unwrap();
            }
        };
        let cleanup = async {
            execution::core_db(background_path.clone(), move |conn| {
                let tx = db::write_transaction(conn)?;
                rrss_core::retention::prune_articles(&tx, &(291..=300).collect::<Vec<_>>(), now)?;
                tx.commit()?;
                rrss_core::media::flush_removed_files(conn)?;
                Ok(())
            })
            .await
            .unwrap();
        };
        let (cluster, interest, (), (), (), ()) =
            tokio::join!(cluster, interest, preparing, ingest, media, cleanup);
        let mut retries = 0;
        for result in [cluster, interest] {
            if let Err(error) = result {
                assert!(
                    matches!(error.downcast_ref::<rrss_core::Error>(),Some(rrss_core::Error::Sqlite(rusqlite::Error::SqliteFailure(code,_))) if code.code==rusqlite::ErrorCode::DatabaseBusy)
                );
                retries += 1;
            }
        }
        // Simulate the coordinator's retry after the finite write burst, and verify convergence.
        rrss_app::jobs::run_cluster_jobs(&background_path)
            .await
            .unwrap();
        rrss_app::jobs::run_interest_boost(&background_path)
            .await
            .unwrap();
        retries
    });
    let mut read = Vec::new();
    let mut reader = Vec::new();
    let mut focus = Vec::new();
    let mut settings = Vec::new();
    for i in 0..150 {
        let timed = |work| sample(path.clone(), work, i);
        read.push(timed(Operation::MarkRead).await);
        reader.push(timed(Operation::Reader).await);
        focus.push(timed(Operation::Focus).await);
        settings.push(timed(Operation::CloseSetting).await);
        if background.is_finished() && i >= 49 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    let retries = background.await.unwrap();
    println!(
        "DB_SMALL_MIXED_REPORT {}",
        json!({"articles":300,"new_articles":30,"vector_dimensions":1024,
        "long_body_articles":30,"cached_images":30,"snapshot_retries":retries,"debug_build":cfg!(debug_assertions),
        "mark_read":report(&read),"reader":report(&reader),"focus":report(&focus),"settings":report(&settings),
        "database_activity":db::pool::metrics(&path).unwrap().activity.snapshot()})
    );
    drop(network);
}
