//! Continuous observations complement integer cache scores in behavioral replay.
//! These tests load real signal rows into the production model; no amplified weights.

use super::*;
use crate::{
    db,
    signals::{EndedBy, ReadSession, Via},
};
use rusqlite::params;

const DAY: i64 = 86_400_000;
const NOW: i64 = 100 * DAY;

fn continuous(ages: &[i64]) -> (f64, f64, f64) {
    let dir = tempfile::tempdir().unwrap();
    let conn = db::open_at(&dir.path().join("continuous.db")).unwrap();
    conn.execute_batch(
        "INSERT INTO feeds(id,url,created_at) VALUES(1,'https://bulletin.example/feed',0);
        INSERT INTO vector_spaces(id,base_url,model,dims,prefix_version,normalized,state,created_at)
        VALUES(1,'','synthetic',2,1,1,'active',0);",
    )
    .unwrap();
    for (i, age) in ages.iter().enumerate() {
        let id = i as i64 + 1;
        let at = NOW - age;
        conn.execute("INSERT INTO articles(id,feed_id,title,created_at,fulltext_md) VALUES(?1,1,'Phone update',?2,'Complete synthetic body')",
            params![id, at - DAY]).unwrap();
        conn.execute(
            "INSERT INTO chunks(article_id,ordinal,kind,text,embedding,model,space_id)
            VALUES(?1,0,'title_summary','',?2,'synthetic',1)",
            params![id, crate::rag::f32s_to_le(&[1., 0.])],
        )
        .unwrap();
        crate::signals::record_read_sessions(
            &conn,
            &[ReadSession {
                article_id: id,
                exposure_id: None,
                opened_at: at,
                active_ms: 8_000,
                idle_ms: 0,
                read_chars: 480,
                skimmed_chars: 0,
                total_chars: 480,
                max_block: 10,
                reached_end: true,
                translated: false,
                ended_by: EndedBy::Back,
                via: Via::Feeds,
            }],
        )
        .unwrap();
    }
    conn.execute(
        "INSERT INTO articles(id,feed_id,title,created_at) VALUES(1001,1,'Phone update 1001',?1)",
        [NOW - 60_000],
    )
    .unwrap();
    let model = Model::load(&conn, NOW).unwrap();
    let lexical = model
        .features
        .weighted(&model.articles[&1001].chips)
        .iter()
        .map(|(_, value)| value)
        .sum::<f64>()
        .clamp(-combine::MAX_FEATURE_SCORE, combine::MAX_FEATURE_SCORE);
    (
        model.affinity.score(&[1., 0.]).0,
        model.recent_affinity.score(&[1., 0.]).0,
        lexical,
    )
}

#[test]
fn continuous_temporal_evidence_reports_decay_and_daily_cap_without_amplifying_scores() {
    let quarter_day = DAY / 4;
    let one = continuous(&[quarter_day]);
    let ten = continuous(&[quarter_day; 10]);
    let hundred = continuous(&[quarter_day; 100]);
    let aged_seven = continuous(&[quarter_day + 7 * DAY; 10]);
    let aged_forty_five = continuous(&[quarter_day + 45 * DAY; 10]);
    let sustained = continuous(&(0..10).map(|d| d * DAY + quarter_day).collect::<Vec<_>>());
    let mean_age_matched = continuous(&[4 * DAY + 3 * DAY / 4; 10]);
    println!(
        "CONTINUOUS {}",
        serde_json::json!({
            "fields": ["long_profile", "recent_profile", "lexical_before_rounding"], "one": one, "ten": ten,
            "hundred": hundred, "aged_7d": aged_seven, "aged_45d": aged_forty_five,
            "sustained": sustained, "mean_age_matched_burst": mean_age_matched,
            "note": "raw vector profile cosine times confidence and production-capped lexical contribution; no integer rounding"
        })
    );
    assert_eq!(one.0, 0.0);
    assert_eq!(ten.0, 0.0);
    assert!(ten.1 > one.1 && ten.1 < 1.0);
    assert!(
        (ten.1 - hundred.1).abs() < 1e-8,
        "same-day volume cannot bypass its evidence cap"
    );
    assert!(ten.1 > aged_seven.1 && ten.1 > aged_forty_five.1);
    assert_eq!(aged_seven, (0.0, 0.0, 0.0));
    assert_eq!(aged_forty_five, (0.0, 0.0, 0.0));
    assert!(sustained.0 > ten.0 && sustained.0 > mean_age_matched.0);
    assert!(mean_age_matched.1 > 0.0 && mean_age_matched.1 < ten.1);
    assert!(ten.2 > aged_seven.2 && ten.2 > aged_forty_five.2);
    assert!((ten.2 - hundred.2).abs() < 1e-8);
    assert!(sustained.2 > ten.2 && sustained.2 > mean_age_matched.2);
}
