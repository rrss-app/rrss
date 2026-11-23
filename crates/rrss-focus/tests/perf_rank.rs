//! 45k 篇的排序全量重建与增量更新基准；固定夹具、临时库、无网络。从 core 的 `perf` 拆出。
//! timeout 300s cargo test --release --locked -p rrss-focus --test perf_rank -- --ignored --nocapture
use std::time::Instant;

use rrss_core::db;
use rusqlite::params;

const NOW: i64 = 1_800_000_000_000;

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
            rrss_core::vector_math::normalized(&v).unwrap()
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
        .map(|(i, v)| rrss_focus::affinity::Feedback {
            id: i as i64 + 1,
            vector: v,
            weight: if (i + 1) % 5 == 0 { -1.0 } else { 1.0 },
        })
        .collect();
    let t = Instant::now();
    let profile = rrss_focus::affinity::Profile::learn(&feedback);
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
        let count = rrss_focus::store::rebuild(&conn, NOW).unwrap();
        assert_eq!(count, 45_000);
        println!(
            "PERF rank_full_window b2={enabled} articles={count} ms={:.3}; includes learning, vector reads, reasons and transactional writes; fixture construction excluded",
            t.elapsed().as_secs_f64() * 1000.0
        );
    }
    let ids: Vec<_> = (44_951..=45_000).collect();
    let t = Instant::now();
    let count = rrss_focus::store::update(&conn, &ids, NOW).unwrap();
    assert_eq!(count, 50);
    println!(
        "PERF rank_incremental b2=true articles=45000 updated={count} feedback=1000 dims=512 ms={:.3}; includes learning, scoped vector reads and transactional writes",
        t.elapsed().as_secs_f64() * 1000.0
    );
}
