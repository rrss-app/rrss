//! 阶段 23「实现与开销」：临时库、无网络的 E0 写入与清理基准。
//! cargo test -p rrss-core --test signals_perf -- --ignored --nocapture

use std::time::Instant;

use rrss_core::{
    db,
    signals::{self, ArticleEvent, EndedBy, EventKind, Exposure, ReadSession, Surface, Via},
};

const NOW: i64 = 1_800_000_000_000;

#[test]
#[ignore = "explicit offline E0 performance benchmark"]
fn batch_write_1000_and_prune_200k() {
    let dir = tempfile::tempdir().unwrap();
    let conn = db::open_at(&dir.path().join("signals-perf.db")).unwrap();
    db::set_setting(&conn, "pi_auto_download", "0").unwrap();
    conn.execute_batch(
        "INSERT INTO feeds(id,url,created_at) VALUES(1,'https://test.invalid/rss',0);
        INSERT INTO articles(id,feed_id,title,created_at) VALUES(1,1,'fixture',0);",
    )
    .unwrap();
    let old = NOW - signals::RETENTION_MS - 1;
    let exposures: Vec<_> = (0..1000)
        .map(|i| Exposure {
            article_id: 1,
            surface: Surface::Focus,
            position: i,
            variant: "baseline".into(),
            session_id: format!("s-{i}"),
            shown_at: old,
        })
        .collect();
    let started = Instant::now();
    let ids = signals::record_exposures(&conn, &exposures).unwrap();
    println!(
        "PERF exposures_write rows=1000 ms={:.3}",
        started.elapsed().as_secs_f64() * 1000.0
    );
    let events: Vec<_> = ids
        .iter()
        .map(|&id| ArticleEvent {
            article_id: 1,
            kind: EventKind::Open,
            exposure_id: id,
            at: old,
        })
        .collect();
    let started = Instant::now();
    assert_eq!(signals::record_events(&conn, &events).unwrap(), 1000);
    println!(
        "PERF events_write rows=1000 ms={:.3}",
        started.elapsed().as_secs_f64() * 1000.0
    );
    let sessions: Vec<_> = ids
        .iter()
        .map(|&id| ReadSession {
            article_id: 1,
            exposure_id: id,
            opened_at: old,
            active_ms: 10_000,
            idle_ms: 1_000,
            read_chars: 100,
            skimmed_chars: 200,
            total_chars: 300,
            max_block: 4,
            reached_end: true,
            translated: false,
            ended_by: EndedBy::Back,
            via: Via::Focus,
        })
        .collect();
    let started = Instant::now();
    assert_eq!(
        signals::record_read_sessions(&conn, &sessions).unwrap(),
        1000
    );
    println!(
        "PERF sessions_write rows=1000 ms={:.3}",
        started.elapsed().as_secs_f64() * 1000.0
    );
    // 在计时外补到三表共 20 万行，并覆盖清理时的关联解除。
    let tx = db::write_transaction(&conn).unwrap();
    {
        let mut exposure = tx.prepare("INSERT INTO exposures(article_id,surface,position,score,interest_boost,rank_score,variant,session_id,shown_at)
            VALUES(1,'focus',0,1,2,3,'baseline',?1,?2)").unwrap();
        let mut event = tx
            .prepare(
                "INSERT INTO article_events(article_id,kind,exposure_id,at) VALUES(1,'open',?1,?2)",
            )
            .unwrap();
        let mut session = tx.prepare("INSERT INTO read_sessions(article_id,exposure_id,opened_at,active_ms,idle_ms,read_chars,skimmed_chars,total_chars,max_block,reached_end,translated,ended_by,via)
            VALUES(1,?1,?2,10000,1000,100,200,300,4,1,0,'back','focus')").unwrap();
        for i in 1000..66_666 {
            exposure
                .execute(rusqlite::params![format!("s-{i}"), old])
                .unwrap();
            let id = tx.last_insert_rowid();
            event.execute(rusqlite::params![id, old]).unwrap();
            session.execute(rusqlite::params![id, old]).unwrap();
        }
        for _ in 0..2 {
            event.execute(rusqlite::params![ids[0], old]).unwrap();
        }
    }
    tx.commit().unwrap();
    let started = Instant::now();
    let removed = signals::prune(&conn, NOW).unwrap();
    println!(
        "PERF signals_prune rows={removed} ms={:.3}",
        started.elapsed().as_secs_f64() * 1000.0
    );
    assert_eq!(removed, 200_000);
    for table in ["exposures", "article_events", "read_sessions"] {
        assert_eq!(
            conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
}
