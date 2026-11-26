//! 固定夹具、临时库、无网络的快速性能基准（目标 < 5 分钟）。
//! timeout 300s cargo test --release --locked -p rrss-core --test perf -- --ignored --nocapture
//! RRSS_PERF_INGEST_ONLY=1 可只测入库；计时排除夹具构造。
use std::time::{Duration, Instant};

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
        language: None,
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
    // Replay the fixed ingestion fixture in today's recommendation window.
    // Shift both dates together, outside timings, preserving relative ages and
    // all new/duplicate/mixed ingestion measurements above.
    conn.execute(
        "UPDATE articles SET published_at=published_at+?1,created_at=created_at+?1",
        [feeds::now_ms() - NOW],
    )
    .unwrap();
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
