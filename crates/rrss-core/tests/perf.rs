//! 固定夹具、临时库、无网络的快速性能基准（目标 < 5 分钟）。
//! timeout 300s cargo test --release --locked -p rrss-core --test perf -- --ignored --nocapture
//! RRSS_PERF_INGEST_ONLY=1 可只测入库；计时排除夹具构造。
use std::time::{Duration, Instant};

use rrss_core::feed_scope::FeedScope;
use rrss_core::{db, feeds, ingest, interests, parse::ParsedEntry, rag, search};
use rusqlite::params;

const FEEDS: usize = 20;
const PER_FEED: usize = 250;
const EMBED_N: i64 = 800;
const NOW: i64 = 1_800_000_000_000;

fn entry(seed: usize) -> ParsedEntry {
    let topics = [
        "版本发布",
        "性能优化",
        "架构调整",
        "安全修复",
        "社区动态",
        "标准提案",
        "硬件评测",
        "语言更新",
        "工具链",
        "会议纪要",
    ];
    let cn = [
        "开源", "模型", "推理", "芯片", "框架", "编译", "异步", "内存", "协议", "安全",
    ];
    let en = [
        "runtime",
        "compiler",
        "release",
        "benchmark",
        "network",
        "storage",
    ];
    let mut body = String::from("<p>");
    for i in 0..80 {
        if i % 5 == 0 {
            body.push_str(&format!(" {} ", en[(seed + i) % en.len()]));
        } else {
            body.push_str(cn[(seed + i * 7) % cn.len()]);
        }
        if i % 40 == 39 {
            body.push('。');
        }
    }
    if seed == 17 {
        body.push_str("独有稀疏关键词，AI Go 编程。");
    }
    body.push_str("</p>");
    ParsedEntry {
        guid: format!("perf-{seed}"),
        canonical_url: Some(format!("https://perf.example/a-{seed}")),
        title: format!(
            "第 {seed} 期 {}：关于 {} 的深度报道 {}",
            topics[seed % 10],
            topics[(seed / 7) % 10],
            seed % 97
        ),
        author: Some("benchmark".into()),
        published_at: Some(NOW - seed as i64 * 60_000),
        content_html: Some(body),
        hash: format!("hash-{seed}"),
        tags: vec!["tech".into()],
    }
}

fn ingest_scenario(
    conn: &rusqlite::Connection,
    feeds: &[i64],
    batches: &[Vec<ParsedEntry>],
    label: &str,
    expected: (u32, u32),
) {
    let mut sum = ingest::IngestReport::default();
    for (&feed, entries) in feeds.iter().zip(batches) {
        let r = ingest::upsert_entries_measured(conn, feed, entries, NOW).unwrap();
        sum.inserted += r.inserted;
        sum.skipped += r.skipped;
        sum.dedup += r.dedup;
        sum.prepare += r.prepare;
        sum.write += r.write;
        sum.commit += r.commit;
        sum.cluster += r.cluster;
        sum.total += r.total;
    }
    assert_eq!((sum.inserted, sum.skipped), expected);
    println!(
        "PERF {}",
        serde_json::json!({"scenario": label, "inserted": sum.inserted, "skipped": sum.skipped,
        "total_ms": sum.total.as_secs_f64() * 1000.0, "dedup_ms": sum.dedup.as_secs_f64() * 1000.0,
        "prepare_ms": sum.prepare.as_secs_f64() * 1000.0, "write_ms": sum.write.as_secs_f64() * 1000.0,
        "commit_ms": sum.commit.as_secs_f64() * 1000.0, "cluster_ms": sum.cluster.as_secs_f64() * 1000.0})
    );
}

fn sample(mut run: impl FnMut()) -> (u128, u128) {
    run(); // warm up; results below are warm-cache measurements, not cold start.
    let mut samples = Vec::new();
    for _ in 0..9 {
        let t = Instant::now();
        run();
        samples.push(t.elapsed().as_micros());
    }
    samples.sort();
    (samples[4], samples[8])
}

#[test]
#[ignore = "explicit offline performance benchmark; target under five minutes"]
fn perf_baseline_5k() {
    let started = Instant::now();
    let dir = tempfile::tempdir().unwrap();
    let conn = db::open_at(&dir.path().join("perf.db")).unwrap();
    db::set_setting(&conn, "pi_auto_download", "0").unwrap();
    let feeds: Vec<_> = (0..FEEDS)
        .map(|f| {
            feeds::subscribe(
                &conn,
                feeds::SubscribeRequest {
                    url: format!("https://perf.example/feed-{f}"),
                    folder_id: None,
                    as_rsshub: false,
                    rsshub_instance_id: None,
                },
            )
            .unwrap()
        })
        .collect();
    let batches: Vec<Vec<_>> = (0..FEEDS)
        .map(|f| (0..PER_FEED).map(|i| entry(f * PER_FEED + i)).collect())
        .collect();
    let mixed: Vec<Vec<_>> = (0..FEEDS)
        .map(|f| {
            let mut entries = batches[f][..45].to_vec();
            entries.extend((0..5).map(|i| entry(FEEDS * PER_FEED + f * 5 + i)));
            entries
        })
        .collect();
    println!(
        "PERF fixture=5000 articles,20 feeds,80 tokens,64 dimensions; no HTTP; fixture construction excluded"
    );
    ingest_scenario(&conn, &feeds, &batches, "new", (5000, 0));
    ingest_scenario(&conn, &feeds, &batches, "duplicate", (0, 5000));
    ingest_scenario(&conn, &feeds, &mixed, "mixed", (100, 900));
    if std::env::var_os("RRSS_PERF_INGEST_ONLY").is_some() {
        return;
    }
    let (p50, max) = sample(|| {
        assert!(
            !rrss_core::articles::list_focus_page(&conn, 50, None, &[], &[], FeedScope::All)
                .unwrap()
                .0
                .is_empty()
        );
    });
    println!("PERF focus_warm p50_us={p50} max_us={max}");
    for query in [
        "性能优化",
        "独有稀疏关键词",
        "完全无匹配的词组",
        "开源",
        "AI",
        "Go",
        "compiler",
    ] {
        let mut hits = 0;
        let (p50, max) = sample(|| {
            hits = search::search_articles(&conn, query, 50).unwrap().len();
        });
        println!("PERF search_warm query={query:?} hits={hits} p50_us={p50} max_us={max}");
    }
    let tx = db::write_transaction(&conn).unwrap();
    tx.execute("INSERT INTO vector_spaces(id,base_url,model,dims,prefix_version,normalized,state,created_at) VALUES(1,'','perf',64,1,0,'active',0)", []).unwrap();
    let iid = interests::insert_interest(&tx, "perf prototype", "manual").unwrap();
    rag::set_interest_embedding(
        &tx,
        iid,
        &(0..64).map(|i| i as f32 / 64.0).collect::<Vec<_>>(),
    )
    .unwrap();
    for id in 1..=EMBED_N {
        let v: Vec<f32> = (0..64)
            .map(|k| ((id * 31 + k) % 100) as f32 / 100.0)
            .collect();
        tx.execute("INSERT INTO chunks (article_id,ordinal,kind,text,embedding,model,space_id) VALUES (?1,0,'title_summary','fixture',?2,'perf',1)", params![id, rag::f32s_to_le(&v)]).unwrap();
    }
    tx.commit().unwrap();
    let query: Vec<f32> = (0..64).map(|i| i as f32 / 64.0).collect();
    let (p50, max) = sample(|| {
        assert_eq!(rag::knn_articles(&conn, &query, 50).unwrap().len(), 50);
    });
    println!("PERF knn_warm vectors={EMBED_N} dims=64 p50_us={p50} max_us={max}");
    let t = Instant::now();
    let boosted = interests::recompute_interest_boost(&conn).unwrap();
    println!(
        "PERF boost_first us={} boosted={boosted}",
        t.elapsed().as_micros()
    );
    let (p50, max) = sample(|| {
        interests::recompute_interest_boost(&conn).unwrap();
    });
    println!("PERF boost_unchanged p50_us={p50} max_us={max}");
    println!("PERF whole_test_ms={}", started.elapsed().as_millis());
    assert!(
        started.elapsed() < Duration::from_secs(300),
        "quick benchmark exceeded five minutes"
    );
}

#[test]
#[ignore = "explicit B1/B2 45k offline performance benchmark"]
fn perf_rank_45k() {
    let dir = tempfile::tempdir().unwrap();
    let conn = db::open_at(&dir.path().join("rank.db")).unwrap();
    db::set_setting(&conn, "pi_auto_download", "0").unwrap();
    let vectors: Vec<Vec<f32>> = (1..=45_000)
        .map(|id| {
            let mut state = id as u32;
            let mut v: Vec<f32> = (0..512)
                .map(|_| {
                    state = state.wrapping_mul(1664525).wrapping_add(1013904223);
                    ((state >> 16) as f32 / 65535.0 - 0.5) * 0.02
                })
                .collect();
            v[id % 8] += 1.0;
            rrss_core::rank::affinity::normalized(&v).unwrap()
        })
        .collect();
    let tx = db::write_transaction(&conn).unwrap();
    tx.execute("INSERT INTO vector_spaces(id,base_url,model,dims,prefix_version,normalized,state,created_at)
        VALUES(1,'','perf',512,1,1,'active',0)",[]).unwrap();
    tx.execute(
        "INSERT INTO feeds(id,url,created_at) VALUES(1,'https://perf.invalid/feed',0)",
        [],
    )
    .unwrap();
    {
        let mut insert = tx.prepare("INSERT INTO articles(id,feed_id,title,author,canonical_url,published_at,created_at) VALUES(?1,1,?2,'benchmark','https://perf.invalid',?3,0)").unwrap();
        let mut tag = tx
            .prepare("INSERT INTO article_tags(article_id,tag) VALUES(?1,'tech')")
            .unwrap();
        for id in 1..=45_000 {
            insert
                .execute(params![
                    id,
                    format!("Rust 性能优化 异步内存 第 {id} 期"),
                    NOW - id * 60_000
                ])
                .unwrap();
            tag.execute([id]).unwrap();
            tx.execute(
                "INSERT INTO chunks(article_id,ordinal,kind,text,embedding,model,space_id)
                VALUES(?1,0,'title_summary','perf',?2,'perf',1)",
                params![id, rrss_core::rag::f32s_to_le(&vectors[(id - 1) as usize])],
            )
            .unwrap();
        }
    }
    for id in 1..=1000 {
        tx.execute(
            "INSERT INTO feedback(article_id,kind,created_at) VALUES(?1,?2,?3)",
            params![id, if id % 5 == 0 { "down" } else { "up" }, NOW - 86400000],
        )
        .unwrap();
    }
    tx.commit().unwrap();
    let feedback: Vec<_> = vectors
        .iter()
        .take(1000)
        .enumerate()
        .map(|(i, v)| rrss_core::rank::affinity::Feedback {
            id: i as i64 + 1,
            vector: v,
            weight: if (i + 1) % 5 == 0 { -1.0 } else { 1.0 },
        })
        .collect();
    let t = Instant::now();
    let profile = rrss_core::rank::affinity::Profile::learn(&feedback);
    println!(
        "PERF affinity_learning positives=800 negatives=200 dims=512 ms={:.3}",
        t.elapsed().as_secs_f64() * 1000.0
    );
    let t = Instant::now();
    let sum: f64 = vectors.iter().map(|v| profile.score(v).0).sum();
    assert!(sum.is_finite());
    println!(
        "PERF affinity_scoring articles=45000 centers=8 dims=512 ms={:.3}; uses preselected centroid representatives for reasons",
        t.elapsed().as_secs_f64() * 1000.0
    );
    for enabled in [false, true] {
        db::set_setting_enabled(&conn, db::RANK_AFFINITY_ENABLED_KEY, enabled).unwrap();
        let t = Instant::now();
        let count = rrss_core::rank::store::rebuild(&conn, NOW).unwrap();
        assert_eq!(count, 45_000);
        println!(
            "PERF rank_full_window b2={enabled} articles={count} ms={:.3}; includes learning, vector reads, reasons and transactional writes; fixture construction excluded",
            t.elapsed().as_secs_f64() * 1000.0
        );
    }
    let ids: Vec<_> = (44_951..=45_000).collect();
    let t = Instant::now();
    let count = rrss_core::rank::store::update(&conn, &ids, NOW).unwrap();
    assert_eq!(count, 50);
    println!(
        "PERF rank_incremental b2=true articles=45000 updated={count} feedback=1000 dims=512 ms={:.3}; includes learning, scoped vector reads and transactional writes",
        t.elapsed().as_secs_f64() * 1000.0
    );
}
