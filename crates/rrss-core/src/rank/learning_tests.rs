//! Production-model contract tests; only disposable synthetic databases.
use super::*;
use crate::{
    db,
    rank::evidence::DAY_MS,
    signals::{self, ArticleEvent, EventKind},
};
use rusqlite::params;

const TOPICS: Switches = Switches {
    features: true,
    source: false,
    coverage: false,
    freshness: false,
    affinity: false,
};
const VECTORS: Switches = Switches {
    features: false,
    affinity: true,
    ..TOPICS
};

fn fixture() -> (tempfile::TempDir, db::PooledConnection) {
    let dir = tempfile::tempdir().unwrap();
    let conn = db::open_at(&dir.path().join("learning.db")).unwrap();
    conn.execute_batch("INSERT INTO feeds(id,url,created_at) VALUES(1,'https://daily.example/rss',0),(2,'https://quiet.example/rss',0);
        INSERT INTO vector_spaces(id,base_url,model,dims,prefix_version,normalized,state,created_at)
        VALUES(1,'','fixture',2,1,0,'active',0);").unwrap();
    (dir, conn)
}
fn article(conn: &Connection, id: i64, feed: i64, title: &str, vector: &[f32]) {
    conn.execute(
        "INSERT INTO articles(id,feed_id,title,author,canonical_url,fulltext_md,created_at)
        VALUES(?1,?2,?3,?4,?5,'body',0)",
        params![
            id,
            feed,
            title,
            format!("Author {feed}"),
            format!("https://feed{feed}.example/{id}")
        ],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO chunks(article_id,ordinal,kind,text,embedding,model,space_id)
        VALUES(?1,0,'title_summary','',?2,'fixture',1)",
        params![id, crate::rag::f32s_to_le(vector)],
    )
    .unwrap();
}
fn session(conn: &Connection, id: i64, at: i64, via: &str, exposure: Option<i64>, deep: bool) {
    conn.execute("INSERT INTO read_sessions(article_id,opened_at,exposure_id,active_ms,read_chars,skimmed_chars,total_chars,reached_end,translated,ended_by,via,idle_ms,max_block)
        VALUES(?1,?2,?3,?4,?5,0,6000,?6,0,'back',?7,0,0)",params![id,at,exposure,if deep { 400000 } else { 1000 },if deep { 5700 } else { 0 },deep,via]).unwrap();
}
fn event(conn: &Connection, id: i64, at: i64, kind: EventKind, exposure: Option<i64>) {
    signals::record_events(
        conn,
        &[ArticleEvent {
            article_id: id,
            kind,
            at,
            exposure_id: exposure,
        }],
    )
    .unwrap();
}
fn boost(conn: &Connection, id: i64, now: i64, switches: Switches) -> i64 {
    Model::load(conn, now)
        .unwrap()
        .article_boost(id, now, switches)
        .unwrap()
        .score
}
fn exposure(conn: &Connection, id: i64, at: i64) -> i64 {
    signals::record_exposures(
        conn,
        &[signals::Exposure {
            article_id: id,
            surface: signals::Surface::Focus,
            position: 0,
            variant: "baseline".into(),
            session_id: format!("{id}-{at}"),
            shown_at: at,
        }],
    )
    .unwrap()[0]
        .unwrap()
}

#[test]
fn one_explicit_choice_transfers_topics_not_author_or_entire_source_and_is_reversible() {
    let (_dir, conn) = fixture();
    article(&conn, 1, 1, "Orbital mechanics", &[1.0, 0.0]);
    article(&conn, 2, 2, "Orbital dynamics", &[1.0, 0.0]);
    article(&conn, 3, 1, "Garden plants", &[0.0, 1.0]);
    event(&conn, 1, 1, EventKind::Star, None);
    assert_eq!(boost(&conn, 2, 2, TOPICS), 1);
    assert_eq!(boost(&conn, 2, 2, VECTORS), 1);
    assert_eq!(boost(&conn, 3, 2, TOPICS), 0);
    assert_eq!(boost(&conn, 3, 2, VECTORS), 0);
    event(&conn, 1, 3, EventKind::Unstar, None);
    assert_eq!(boost(&conn, 2, 4, TOPICS), 0);
    assert_eq!(boost(&conn, 2, 4, VECTORS), 0);
    for (at, kind, sign) in [
        (5, EventKind::Up, 1),
        (7, EventKind::Down, -1),
        (9, EventKind::Cite, 1),
        (11, EventKind::Ask, 1),
    ] {
        event(&conn, 1, at, kind, None);
        assert_eq!(boost(&conn, 2, at + 1, TOPICS).signum(), sign);
        assert_eq!(boost(&conn, 3, at + 1, TOPICS), 0);
    }
    assert_eq!(boost(&conn, 2, 5, TOPICS), 0); // actual event time is a strict cutoff
}

#[test]
fn one_day_burst_is_bounded_and_expires_in_both_profiles_but_repeated_days_persist() {
    let (_dir, conn) = fixture();
    for id in 1..=10 {
        article(&conn, id, 1, "Orbital dynamics", &[1.0, 0.0]);
    }
    article(&conn, 11, 2, "Orbital dynamics", &[1.0, 0.0]);
    let now = 10 * DAY_MS + 100;
    session(&conn, 1, 10 * DAY_MS, "feeds", None, true);
    let one = Model::load(&conn, now).unwrap();
    for id in 2..=10 {
        session(&conn, id, 10 * DAY_MS, "feeds", None, true);
    }
    let burst = Model::load(&conn, now).unwrap();
    assert_eq!(
        one.article_boost(11, now, TOPICS),
        burst.article_boost(11, now, TOPICS)
    );
    assert_eq!(one.affinity.score(&[1.0, 0.0]).0, 0.0);
    assert_eq!(burst.affinity.score(&[1.0, 0.0]).0, 0.0);
    assert!(
        (one.recent_affinity.score(&[1.0, 0.0]).0 - burst.recent_affinity.score(&[1.0, 0.0]).0)
            .abs()
            < 1e-8
    );
    let topic_component = |model: &Model| {
        model
            .features
            .weighted(&model.articles[&11].chips)
            .iter()
            .map(|(_, weight)| weight)
            .sum::<f64>()
    };
    assert!(topic_component(&burst) > 0.0);
    let fading = Model::load(&conn, now + DAY_MS).unwrap();
    assert!(
        fading.recent_affinity.score(&[1.0, 0.0]).0 < burst.recent_affinity.score(&[1.0, 0.0]).0
    );
    let expired = Model::load(&conn, now + 7 * DAY_MS).unwrap();
    assert_eq!(topic_component(&expired), 0.0);
    assert_eq!(expired.recent_affinity.score(&[1.0, 0.0]).0, 0.0);
    assert_eq!(boost(&conn, 11, now + 7 * DAY_MS, TOPICS), 0);
    assert_eq!(boost(&conn, 11, now + 7 * DAY_MS, VECTORS), 0);
    // Re-reading the same article on different days counts as persistence, while
    // repeated same-day sessions are still one observation.
    conn.execute("DELETE FROM read_sessions", []).unwrap();
    for day in 1..=4 {
        session(&conn, 1, day * DAY_MS, "search", None, true);
    }
    let later = 12 * DAY_MS;
    assert!(boost(&conn, 11, later, TOPICS) >= 2);
    assert!(boost(&conn, 11, later, VECTORS) >= 1);
    let stable = boost(&conn, 11, later, TOPICS);
    session(&conn, 1, 4 * DAY_MS + 1, "search", None, true);
    assert_eq!(boost(&conn, 11, later, TOPICS), stable);
    println!(
        "learning: one-day topic={} vector={}; after 7d topic=0 vector=0; four-day topic={} vector={}",
        burst.article_boost(11, now, TOPICS).unwrap().score,
        burst.article_boost(11, now, VECTORS).unwrap().score,
        stable,
        boost(&conn, 11, later, VECTORS)
    );
}

#[test]
fn explicit_feedback_keeps_strength_with_busy_same_day_and_unstar_restores_reading() {
    let (_dir, conn) = fixture();
    for id in 1..=11 {
        article(
            &conn,
            id,
            if id == 11 { 2 } else { 1 },
            "Orbital dynamics",
            &[1.0, 0.0],
        );
    }
    session(&conn, 1, DAY_MS, "feeds", None, true);
    let baseline = boost(&conn, 11, DAY_MS + 20, TOPICS);
    event(&conn, 1, DAY_MS + 1, EventKind::Star, None);
    let star = boost(&conn, 11, DAY_MS + 20, TOPICS);
    for id in 2..=10 {
        session(&conn, id, DAY_MS, "feeds", None, true);
    }
    assert!(boost(&conn, 11, DAY_MS + 20, TOPICS) >= star);
    event(&conn, 1, DAY_MS + 2, EventKind::Unstar, None);
    assert_eq!(boost(&conn, 11, DAY_MS + 20, TOPICS), baseline);
    assert!(star > baseline);
}

fn source_fixture(conn: &Connection, deep: bool) -> Vec<(i64, i64, i64)> {
    let mut records = Vec::new();
    for day in 1..=10 {
        let id = day;
        article(
            conn,
            id,
            1,
            if day % 2 == 0 {
                "Orbital dynamics"
            } else {
                "Garden plants"
            },
            &[1.0, 0.0],
        );
        article(conn, id + 100, 2, "Neutral item", &[0.0, 1.0]);
        let at = day * DAY_MS;
        let e = exposure(conn, id, at);
        exposure(conn, id + 100, at);
        session(conn, id, at + 1, "focus", Some(e), deep);
        records.push((id, at, e));
    }
    records
}

#[test]
fn source_requires_real_opportunities_matching_entry_points_and_broad_repeated_reading() {
    let (_dir, conn) = fixture();
    source_fixture(&conn, true);
    let now = 11 * DAY_MS;
    let good = Model::load(&conn, now).unwrap().sources[&1];
    assert!(good > 0.25 && good <= 0.5);
    for via in [
        "feeds",
        "search",
        "digest",
        "citation",
        "drawer",
        "related",
        "focus_column",
    ] {
        conn.execute("UPDATE read_sessions SET via=?1", [via])
            .unwrap();
        assert_eq!(Model::load(&conn, now).unwrap().sources[&1], 0.0, "{via}");
    }
    conn.execute("UPDATE read_sessions SET via='focus',exposure_id=NULL", [])
        .unwrap();
    assert_eq!(Model::load(&conn, now).unwrap().sources[&1], 0.0);
    conn.execute("UPDATE read_sessions SET exposure_id=(SELECT id FROM exposures WHERE article_id=read_sessions.article_id LIMIT 1)",[]).unwrap();
    assert_eq!(Model::load(&conn, now).unwrap().sources[&1], good);
    conn.execute(
        "UPDATE articles SET title='Orbital dynamics' WHERE feed_id=1",
        [],
    )
    .unwrap();
    assert_eq!(Model::load(&conn, now).unwrap().sources[&1], 0.0);
}

#[test]
fn source_does_not_credit_search_after_focus_or_keep_revoked_explicit_states() {
    let (_dir, conn) = fixture();
    let records = source_fixture(&conn, false);
    let now = 15 * DAY_MS;
    for &(id, at, e) in &records {
        event(&conn, id, at + 2, EventKind::Star, Some(e));
    }
    assert!(Model::load(&conn, now).unwrap().sources[&1] > 0.25);
    for &(id, at, e) in &records {
        session(&conn, id, at + 3, "search", Some(e), false);
        event(&conn, id, at + 4, EventKind::Star, Some(e));
    }
    assert!(Model::load(&conn, now).unwrap().sources[&1] <= 0.0);
    conn.execute("DELETE FROM article_events WHERE at%86400000=4", [])
        .unwrap();
    assert!(Model::load(&conn, now).unwrap().sources[&1] > 0.25);
    for &(id, _, e) in &records {
        event(&conn, id, 14 * DAY_MS, EventKind::Unstar, Some(e));
    }
    assert!(Model::load(&conn, now).unwrap().sources[&1] <= 0.0);
    conn.execute("DELETE FROM article_events WHERE kind='unstar'", [])
        .unwrap();
    for &(id, _, e) in &records {
        event(&conn, id, 14 * DAY_MS, EventKind::Down, Some(e));
    }
    assert!(Model::load(&conn, now).unwrap().sources[&1] <= 0.0);
}

#[test]
fn switches_and_missing_vectors_preserve_lexical_fallback() {
    let (_dir, conn) = fixture();
    article(&conn, 1, 1, "Orbital mechanics", &[1.0, 0.0]);
    article(&conn, 2, 2, "Orbital dynamics", &[1.0, 0.0]);
    event(&conn, 1, 1, EventKind::Star, None);
    let on = boost(&conn, 2, 2, TOPICS);
    assert_eq!(
        boost(
            &conn,
            2,
            2,
            Switches {
                features: false,
                ..TOPICS
            }
        ),
        0
    );
    assert_eq!(
        boost(
            &conn,
            2,
            2,
            Switches {
                affinity: false,
                ..VECTORS
            }
        ),
        0
    );
    conn.execute("DELETE FROM chunks", []).unwrap();
    assert_eq!(boost(&conn, 2, 2, TOPICS), on);
    assert_eq!(boost(&conn, 2, 2, VECTORS), 0);
}

#[test]
fn explicit_choice_beats_a_single_day_of_light_reading_across_languages_and_supply() {
    for (liked, related, fleeting, transient) in [
        (
            "Orbital mechanics",
            "Orbital dynamics",
            "Orchard harvest",
            "Orchard growth",
        ),
        (
            "深海生态研究",
            "深海生态笔记",
            "园艺植物消息",
            "园艺植物变化",
        ),
    ] {
        for count in [1, 10, 100] {
            let (_dir, conn) = fixture();
            let at = 10 * DAY_MS;
            for id in 1..=count {
                article(&conn, id, 1, fleeting, &[0.0, 1.0]);
                session(&conn, id, at, "feeds", None, true);
            }
            // Real reading, below the deep-reading coverage threshold.
            conn.execute("UPDATE read_sessions SET read_chars=4000", [])
                .unwrap();
            article(&conn, 1001, 2, liked, &[1.0, 0.0]);
            session(&conn, 1001, at, "feeds", None, true);
            event(&conn, 1001, at + 1, EventKind::Star, None);
            article(&conn, 1002, 1, related, &[1.0, 0.0]);
            article(&conn, 1003, 2, transient, &[0.0, 1.0]);
            article(&conn, 1004, 2, "Untouched", &[-1.0, 0.0]);
            let now = at + 2;
            for affinity in [false, true] {
                let switches = Switches { affinity, ..TOPICS };
                let liked_score = boost(&conn, 1002, now, switches);
                let transient_score = boost(&conn, 1003, now, switches);
                assert!(
                    liked_score > transient_score,
                    "count={count}, affinity={affinity}"
                );
                assert_eq!(boost(&conn, 1004, now, switches), 0);
            }
            conn.execute("DELETE FROM read_sessions;", []).unwrap();
            for day in 1..=10 {
                session(&conn, 1, day * DAY_MS, "feeds", None, true);
            }
            assert!(boost(&conn, 1003, now, TOPICS) > 0);
            let vector = boost(&conn, 1003, now, VECTORS);
            assert!((1..=2).contains(&vector));
        }
    }
}

#[test]
fn broad_tagged_source_can_learn_without_an_unengaged_comparison_feed() {
    for reward in ["read", "deep", "explicit"] {
        let (_dir, conn) = fixture();
        for day in 1..=10 {
            article(&conn, day, 1, "Daily journal edition report", &[1.0, 0.0]);
            conn.execute(
                "INSERT INTO article_tags(article_id,tag) VALUES(?1,?2)",
                params![day, ["astronomy", "botany", "literature"][day as usize % 3]],
            )
            .unwrap();
            let at = day * DAY_MS;
            let e = exposure(&conn, day, at);
            session(&conn, day, at + 1, "focus", Some(e), reward != "explicit");
            if reward == "explicit" {
                event(&conn, day, at + 2, EventKind::Star, Some(e));
            }
        }
        if reward == "read" {
            conn.execute("UPDATE read_sessions SET read_chars=4000", [])
                .unwrap();
        }
        let now = 11 * DAY_MS;
        let score = Model::load(&conn, now).unwrap().sources[&1];
        assert!(score >= 0.25 && score <= 0.5, "{reward}: {score}");
        conn.execute("UPDATE read_sessions SET via='search'", [])
            .unwrap();
        assert_eq!(Model::load(&conn, now).unwrap().sources[&1], 0.0);
        conn.execute("UPDATE read_sessions SET via='focus', exposure_id=NULL", [])
            .unwrap();
        assert_eq!(Model::load(&conn, now).unwrap().sources[&1], 0.0);
    }
}
