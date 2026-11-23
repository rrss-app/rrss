use super::tests::{fixture, page};
use crate::store;
use rrss_core::{db, feeds::now_ms};
use rusqlite::{Connection, params};

const DAY: i64 = 86_400_000;

fn cached_topic(conn: &Connection, id: i64) -> crate::topics::Evidence {
    let json: String = conn
        .query_row(
            "SELECT reasons_json FROM article_rank WHERE article_id=?1",
            [id],
            |r| r.get(0),
        )
        .unwrap();
    crate::cache::topics(&json).unwrap()
}

#[test]
fn semantic_partition_survives_later_incremental_scoring() {
    let (_dir, conn) = fixture();
    let now = trained(&conn, true, 1, 1, 120);
    rrss_core::articles::set_list_scope(&conn, rrss_core::articles::ListScope::Unread).unwrap();
    db::set_setting_enabled(&conn, db::RANK_FEATURES_ENABLED_KEY, false).unwrap();
    // Non-axis-aligned centers expose rounding after the common decay factor.
    for id in 1..=10 {
        let vector = [1.0, id as f32 * 0.02, id as f32 * 0.003];
        conn.execute(
            "UPDATE chunks SET embedding=?2 WHERE article_id=?1",
            params![id, rrss_core::rag::f32s_to_le(&vector)],
        )
        .unwrap();
    }
    store::rebuild(&conn, now).unwrap();
    let old = cached_topic(&conn, 50);
    let later = now + 3_600_000;
    article(
        &conn,
        4000,
        1,
        "Fresh fermentation report",
        now,
        &[0., 1., 0.],
    );
    assert_eq!(store::update(&conn, &[4000], later).unwrap(), 1);
    let new = cached_topic(&conn, 4000);
    assert!(new.affinity < old.affinity, "confidence must still decay");
    assert_eq!(
        old.semantic, new.semantic,
        "unchanged evidence and partition must share an epoch"
    );
    let computed: i64 = conn
        .query_row(
            "SELECT computed_at FROM article_rank WHERE article_id=50",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(computed, now, "the old cache must remain incremental");
    let first = page(&conn, 40, &[]).0;
    assert!(
        first
            .iter()
            .take(10)
            .any(|c| matches!(c.id, 50 | 51 | 4000)),
        "semantic topic must retain a first-page opportunity"
    );
    let ids =
        |cards: &[rrss_core::articles::ArticleCard]| cards.iter().map(|c| c.id).collect::<Vec<_>>();
    assert_eq!(ids(&first), ids(&page(&conn, 40, &[]).0));
    let mut seen = ids(&first);
    loop {
        let (next, more) = page(&conn, 40, &seen);
        for id in ids(&next) {
            assert!(!seen.contains(&id), "pagination repeated {id}");
            seen.push(id);
        }
        if !more {
            break;
        }
        assert!(!next.is_empty());
    }
    assert_eq!(seen.len(), 124);
}

#[test]
fn changed_semantic_spaces_and_memberships_keep_lexical_pagination() {
    for change in ["space", "membership"] {
        let (_dir, conn) = fixture();
        let now = trained(&conn, true, 1, 1, 120);
        rrss_core::articles::set_list_scope(&conn, rrss_core::articles::ListScope::Unread).unwrap();
        let old = cached_topic(&conn, 50);
        if change == "space" {
            conn.execute_batch("UPDATE vector_spaces SET state='retired' WHERE id=1;
                INSERT INTO vector_spaces(id,base_url,model,dims,prefix_version,normalized,state,created_at)
                VALUES(2,'','other-fixture',3,1,1,'active',0);
                UPDATE chunks SET space_id=2,model='other-fixture';").unwrap();
        } else {
            conn.execute(
                "UPDATE chunks SET embedding=?1 WHERE article_id=10",
                [rrss_core::rag::f32s_to_le(&[0., 1., 0.])],
            )
            .unwrap();
        }
        assert_eq!(store::update(&conn, &[51], now + 3_600_000).unwrap(), 1);
        let new = cached_topic(&conn, 51);
        assert_ne!(
            old.epoch(),
            new.epoch(),
            "{change} must invalidate compatibility"
        );
        let ids = |cards: &[rrss_core::articles::ArticleCard]| {
            cards.iter().map(|c| c.id).collect::<Vec<_>>()
        };
        let first = ids(&page(&conn, 40, &[]).0);
        let next = ids(&page(&conn, 40, &first).0);
        let switches = crate::store::switches(&conn).unwrap();
        for topic in [&old, &new] {
            assert!(topic.allocation(switches, false).eligible());
            assert!(matches!(
                crate::topics::Lane::from_evidence(topic).key(switches, false),
                Some(crate::topics::LaneKey::Lexical(_))
            ));
        }
        assert!(next.iter().all(|id| !first.contains(id)));
        // Clearing just semantics must reproduce the mixed-epoch fallback,
        // including both candidate admission and marginal topic allocation.
        conn.execute(
            "UPDATE article_rank SET reasons_json=json_set(reasons_json,
            '$.topics.semantic',NULL,'$.lane[5]',NULL,'$.lane[6]',NULL)
            WHERE substr(reasons_json,1,1)='{'",
            [],
        )
        .unwrap();
        assert_eq!(first, ids(&page(&conn, 40, &[]).0), "{change}");
        assert_eq!(next, ids(&page(&conn, 40, &first).0), "{change}");
    }
}

fn article(conn: &Connection, id: i64, feed: i64, title: &str, at: i64, vector: &[f32]) {
    conn.execute(
        "INSERT INTO articles(id,feed_id,title,author,created_at,published_at,fulltext_md)
        VALUES(?1,?2,?3,?4,?5,?5,'Synthetic complete article')",
        params![id, feed, title, format!("Writer {feed}"), at],
    )
    .unwrap();
    if !vector.is_empty() {
        conn.execute(
            "INSERT INTO chunks(article_id,ordinal,kind,text,embedding,model,space_id)
            VALUES(?1,0,'title_summary','',?2,'fixture',1)",
            params![id, rrss_core::rag::f32s_to_le(vector)],
        )
        .unwrap();
    }
}

fn trained(
    conn: &Connection,
    vectors: bool,
    news_sources: i64,
    secondary_feed: i64,
    count: i64,
) -> i64 {
    let now = now_ms();
    db::set_setting(conn, db::RANK_FRESHNESS_ENABLED_KEY, "0").unwrap();
    db::set_setting(conn, db::RANK_SOURCE_ENABLED_KEY, "0").unwrap();
    conn.execute_batch("INSERT INTO vector_spaces(id,base_url,model,dims,prefix_version,normalized,state,created_at)
        VALUES(1,'','fixture',3,1,1,'active',0);").unwrap();
    let v = |topic| {
        if !vectors {
            &[][..]
        } else if topic == 0 {
            &[1., 0., 0.][..]
        } else {
            &[0., 1., 0.][..]
        }
    };
    for day in 1..=10 {
        let at = now - day * DAY;
        article(
            conn,
            day,
            1,
            "Orbital telescope instruments",
            at - DAY,
            v(0),
        );
        conn.execute("INSERT INTO read_sessions(article_id,opened_at,active_ms,read_chars,total_chars,reached_end,translated,ended_by,via,idle_ms,skimmed_chars,max_block)
            VALUES(?1,?2,8000,480,480,1,0,'back','feeds',0,0,10)", params![day,at]).unwrap();
    }
    article(
        conn,
        20,
        2,
        "Sourdough fermentation cultures",
        now - DAY,
        v(1),
    );
    conn.execute(
        "INSERT INTO article_events(article_id,kind,at) VALUES(20,'star',?1)",
        [now - DAY / 2],
    )
    .unwrap();
    conn.execute("UPDATE articles SET read=1", []).unwrap();
    for i in 0..count {
        article(
            conn,
            100 + i,
            1 + i % news_sources,
            &format!("Orbital telescope instruments {i}"),
            now - 60_000 + i,
            v(0),
        );
    }
    article(
        conn,
        50,
        secondary_feed,
        "Sourdough fermentation methods",
        now - 120_000,
        v(1),
    );
    article(
        conn,
        51,
        secondary_feed,
        "Sourdough fermentation temperatures",
        now - 120_001,
        v(1),
    );
    article(
        conn,
        60,
        3,
        "Mountain hiking routes",
        now - 30_000,
        if vectors { &[0., 0., 1.] } else { &[] },
    );
    store::rebuild(conn, now).unwrap();
    now
}

#[test]
fn persistent_primary_leads_but_saved_secondary_surfaces_across_sources_and_ids() {
    for (vectors, sources, secondary, count) in [
        (true, 2, 3, 24),
        (true, 1, 1, 120),
        (false, 2, 3, 24),
        (false, 1, 1, 120),
    ] {
        let (_dir, conn) = fixture();
        trained(&conn, vectors, sources, secondary, count);
        let top = page(&conn, 40, &[]).0;
        assert!(top[0].id >= 100);
        assert!(top.iter().take(10).filter(|c| c.id >= 100).count() > 5);
        assert!(
            top.iter().take(10).any(|c| matches!(c.id, 50 | 51)),
            "vectors={vectors}, sources={sources}: {:?}",
            top.iter()
                .take(15)
                .map(|c| (c.id, c.rank_boost))
                .collect::<Vec<_>>()
        );
        assert!(!top.iter().take(10).any(|c| c.id == 60));
        println!(
            "TOPIC vectors={vectors} sources={sources} supply={count} secondary={} primary_top10={}",
            top.iter().position(|c| matches!(c.id, 50 | 51)).unwrap() + 1,
            top.iter().take(10).filter(|c| c.id >= 100).count()
        );
    }
}

#[test]
fn secondary_evidence_can_be_revoked_and_learning_switches_remain_independent() {
    for (features, affinity, event) in [
        (true, true, "unstar"),
        (true, true, "down"),
        (false, false, ""),
        (false, true, ""),
        (true, false, ""),
    ] {
        let (_dir, conn) = fixture();
        let now = trained(&conn, true, 2, 3, 80);
        if !event.is_empty() {
            conn.execute(
                "INSERT INTO article_events(article_id,kind,at) VALUES(20,?1,?2)",
                params![event, now - 1],
            )
            .unwrap();
        }
        db::set_setting_enabled(&conn, db::RANK_FEATURES_ENABLED_KEY, features).unwrap();
        db::set_setting_enabled(&conn, db::RANK_AFFINITY_ENABLED_KEY, affinity).unwrap();
        store::rebuild(&conn, now).unwrap();
        let top = page(&conn, 40, &[]).0;
        if event.is_empty() && (features || affinity) {
            assert!(top.iter().take(10).any(|c| matches!(c.id, 50 | 51)));
        } else if !event.is_empty() {
            assert!(top.iter().take(10).all(|c| c.id >= 100));
        } else {
            let json: String = conn
                .query_row(
                    "SELECT reasons_json FROM article_rank WHERE article_id=50",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert!(crate::cache::topics(&json).is_none());
        }
    }
}

#[test]
fn coverage_continues_across_pages_without_counting_lookahead_or_expanded_members_twice() {
    let (_dir, conn) = fixture();
    let now = trained(&conn, true, 1, 1, 90);
    for i in 0..40 {
        article(
            &conn,
            1000 + i,
            1,
            &format!("Sourdough fermentation batch {i}"),
            now - 180_000 + i,
            &[0., 1., 0.],
        );
    }
    store::rebuild(&conn, now).unwrap();
    let whole = page(&conn, 80, &[]).0;
    for range in [0..10, 10..40, 40..80] {
        assert!(
            whole[range.clone()]
                .iter()
                .any(|c| c.title.starts_with("Sourdough"))
        );
        assert!(whole[range].iter().any(|c| c.title.starts_with("Orbital")));
    }
    let first = page(&conn, 7, &[]).0;
    let previous: Vec<_> = first.iter().map(|c| c.id).collect();
    let next = page(&conn, 7, &previous).0;
    assert_eq!(
        first.iter().chain(&next).map(|c| c.id).collect::<Vec<_>>(),
        whole.iter().take(14).map(|c| c.id).collect::<Vec<_>>()
    );
    let duplicates: Vec<_> = previous.iter().flat_map(|id| [*id, *id]).collect();
    assert_eq!(
        page(&conn, 7, &duplicates)
            .0
            .iter()
            .map(|c| c.id)
            .collect::<Vec<_>>(),
        next.iter().map(|c| c.id).collect::<Vec<_>>()
    );

    conn.execute(
        "UPDATE articles SET cluster_id=77 WHERE id BETWEEN 187 AND 189",
        [],
    )
    .unwrap();
    let grouped = |seen: &[i64]| {
        page(&conn, 10, seen)
            .0
            .iter()
            .map(|c| c.id)
            .collect::<Vec<_>>()
    };
    assert_eq!(grouped(&[189]), grouped(&[189, 188, 187]));
}

#[test]
fn substantive_rules_read_state_and_exposure_costs_still_protect_order() {
    let (_dir, conn) = fixture();
    let now = trained(&conn, true, 2, 3, 80);
    // An important item without learned-topic evidence keeps its rule advantage.
    conn.execute("UPDATE articles SET score=10 WHERE id=60", [])
        .unwrap();
    assert_eq!(page(&conn, 40, &[]).0[0].id, 60);
    conn.execute("UPDATE articles SET score=5 WHERE id=60", [])
        .unwrap();
    let top = page(&conn, 40, &[]).0;
    assert!(
        top.iter().position(|c| c.id == 60).unwrap()
            < top
                .iter()
                .position(|c| matches!(c.id, 50 | 51))
                .unwrap_or(usize::MAX)
    );
    conn.execute("UPDATE articles SET score=0 WHERE id=60", [])
        .unwrap();
    conn.execute("UPDATE articles SET read=1 WHERE id IN (50,51)", [])
        .unwrap();
    assert!(
        !page(&conn, 40, &[])
            .0
            .iter()
            .take(10)
            .any(|c| matches!(c.id, 50 | 51))
    );
    conn.execute("UPDATE articles SET read=0 WHERE id IN (50,51)", [])
        .unwrap();
    for id in [50, 51] {
        for session in 0..3 {
            conn.execute("INSERT INTO exposures(article_id,surface,position,score,interest_boost,rank_score,variant,session_id,shown_at)
            VALUES(?1,'focus',0,0,0,0,'baseline',?2,?3)",params![id,format!("shown-{session}"),now-session*86_400_000]).unwrap();
        }
    }
    assert!(
        !page(&conn, 40, &[])
            .0
            .iter()
            .take(10)
            .any(|c| matches!(c.id, 50 | 51))
    );
    conn.execute("UPDATE articles SET score=-20 WHERE id IN (50,51)", [])
        .unwrap();
    assert!(
        !page(&conn, 200, &[])
            .0
            .iter()
            .any(|c| matches!(c.id, 50 | 51))
    );
}

#[test]
fn date_and_id_ties_do_not_starve_the_same_supported_secondary() {
    for (id, newer) in [(40, false), (9000, true)] {
        let (_dir, conn) = fixture();
        let now = trained(&conn, true, 2, 3, 80);
        conn.execute("DELETE FROM articles WHERE id IN (50,51)", [])
            .unwrap();
        article(
            &conn,
            id,
            3,
            "Sourdough fermentation alternatives",
            now - if newer { 1 } else { DAY },
            &[0., 1., 0.],
        );
        store::rebuild(&conn, now).unwrap();
        assert!(page(&conn, 40, &[]).0.iter().take(10).any(|c| c.id == id));
    }
}

#[test]
fn abundant_secondary_supply_does_not_invert_interest_strength_with_uneven_sources() {
    for vectors in [false, true] {
        for (primary_sources, secondary_sources) in
            [(1, vec![2, 3]), (1, vec![1]), (2, vec![1, 2, 3])]
        {
            let (_dir, conn) = fixture();
            let now = trained(&conn, vectors, primary_sources, secondary_sources[0], 90);
            for i in 0..60 {
                article(
                    &conn,
                    1000 + i,
                    secondary_sources[i as usize % secondary_sources.len()],
                    &format!("Sourdough fermentation method {i}"),
                    now - 90_000 + i,
                    if vectors { &[0., 1., 0.] } else { &[] },
                );
            }
            store::rebuild(&conn, now).unwrap();
            let top = page(&conn, 40, &[]).0;
            let main = top
                .iter()
                .take(10)
                .filter(|c| c.title.starts_with("Orbital"))
                .count();
            assert!(top[0].title.starts_with("Orbital"));
            assert!(
                main > 5 && main < 10,
                "vectors={vectors}, primary_sources={primary_sources}, secondary_sources={secondary_sources:?}, primary_top10={main}"
            );
            assert!(top[10..40].iter().any(|c| c.title.starts_with("Sourdough")));
            println!(
                "SUPPLY vectors={vectors} primary_sources={primary_sources} secondary_sources={secondary_sources:?} primary_top10={main}"
            );
        }
    }
}

#[test]
fn saved_retrieval_keeps_baseline_order_even_with_deep_topic_context() {
    let (_dir, conn) = fixture();
    trained(&conn, true, 1, 1, 90);
    conn.execute("UPDATE articles SET starred=1", []).unwrap();
    rrss_core::articles::set_list_scope(&conn, rrss_core::articles::ListScope::Starred).unwrap();
    conn.execute(
        "WITH RECURSIVE n(id) AS (SELECT 10000 UNION ALL SELECT id+1 FROM n WHERE id<19999)
        INSERT INTO articles(id,feed_id,title,created_at,published_at,starred,score)
        SELECT id,1,'Orbital telescope instruments',0,0,1,-1 FROM n",
        [],
    )
    .unwrap();
    conn.execute("INSERT INTO article_rank(article_id,boost,reasons_json,algo_version,computed_at)
        SELECT id,r.boost,r.reasons_json,r.algo_version,r.computed_at FROM articles CROSS JOIN article_rank r
        WHERE articles.id>=10000 AND r.article_id=100",[]).unwrap();
    let previous: Vec<_> = (10000..20000).collect();
    let deep = page(&conn, 40, &previous).0;
    assert!(deep.iter().take(10).all(|c| (100..190).contains(&c.id)));
    let recent = page(&conn, 40, &previous[previous.len() - 2..]).0;
    assert_eq!(
        deep.iter().map(|c| c.id).collect::<Vec<_>>(),
        recent.iter().map(|c| c.id).collect::<Vec<_>>()
    );
}

#[test]
fn content_reranking_remains_optional_and_mixed_vector_profiles_fall_back() {
    let (_dir, conn) = fixture();
    let now = trained(&conn, true, 2, 3, 80);
    for (id, vector) in [
        (3000, [0., 0., 1.]),
        (3001, [1., 0., 0.]),
        (3002, [1., 0., 0.]),
    ] {
        article(
            &conn,
            id,
            2,
            "Rule-selected report",
            now - 6000 + id,
            &vector,
        );
    }
    conn.execute("UPDATE articles SET score=1 WHERE id>=50", [])
        .unwrap();
    conn.execute("UPDATE articles SET score=10 WHERE id>=3000", [])
        .unwrap();
    let before = page(&conn, 40, &[])
        .0
        .iter()
        .map(|c| c.id)
        .collect::<Vec<_>>();
    rrss_core::interests::set_rerank_enabled(&conn, true).unwrap();
    let reranked = page(&conn, 40, &[])
        .0
        .iter()
        .map(|c| c.id)
        .collect::<Vec<_>>();
    assert_ne!(before, reranked);
    rrss_core::interests::set_rerank_enabled(&conn, false).unwrap();
    assert_eq!(
        before,
        page(&conn, 40, &[])
            .0
            .iter()
            .map(|c| c.id)
            .collect::<Vec<_>>()
    );

    conn.execute("DELETE FROM articles WHERE id>=3000", [])
        .unwrap();

    db::set_setting_enabled(&conn, db::RANK_FEATURES_ENABLED_KEY, false).unwrap();
    store::rebuild(&conn, now).unwrap();
    conn.execute("UPDATE article_rank SET reasons_json=json_set(reasons_json,'$.topics.semantic[0]',?1,'$.lane[5]',?1)
        WHERE article_id IN (50,51)",["f".repeat(64)]).unwrap();
    let mixed = page(&conn, 40, &[])
        .0
        .iter()
        .map(|c| c.id)
        .collect::<Vec<_>>();
    conn.execute("UPDATE article_rank SET reasons_json=json_extract(reasons_json,'$.reasons') WHERE substr(reasons_json,1,1)='{'",[]).unwrap();
    assert_eq!(
        mixed,
        page(&conn, 40, &[])
            .0
            .iter()
            .map(|c| c.id)
            .collect::<Vec<_>>()
    );
}

#[test]
fn malformed_topic_metadata_preserves_old_recovery_and_display_reasons() {
    let (_dir, conn) = fixture();
    let now = trained(&conn, true, 2, 3, 24);
    article(
        &conn,
        42,
        2,
        "Older fermentation reference",
        now - 50 * DAY,
        &[],
    );
    conn.execute("UPDATE articles SET score=1 WHERE id=42", [])
        .unwrap();
    let reason = r#"[{"component":"freshness","code":"rank_freshness","params":{},"points":-2}]"#;
    for json in [
        reason.to_owned(),
        format!(r#"{{"reasons":{reason},"topics":{{"features":"bad"}},"lane":["bad"]}}"#),
    ] {
        conn.execute("INSERT OR REPLACE INTO article_rank(article_id,boost,reasons_json,algo_version,computed_at)
            VALUES(42,-2,?1,?2,?3)",params![json,crate::combine::ALGO_VERSION,now]).unwrap();
        let cards = page(&conn, 80, &[]).0;
        let recovered = cards.iter().find(|c| c.id == 42).unwrap();
        assert_eq!(recovered.rank_reasons[0].points, -2);
        assert_eq!(recovered.rank_boost, -2);
    }
}

#[test]
fn scalar_topic_lanes_preserve_pagination_and_reasons_without_extra_candidate_access() {
    let (_dir, conn) = fixture();
    trained(&conn, true, 1, 1, 120);
    let supported = page(&conn, 40, &[]).0;
    let reasons = supported
        .iter()
        .find(|c| c.id == 50)
        .unwrap()
        .rank_reasons
        .clone();
    assert!(!reasons.is_empty());
    let replace_lane = |lane: &str| {
        conn.execute(
            "UPDATE article_rank SET reasons_json=json_set(reasons_json,'$.lane',json(?1))
             WHERE article_id IN (50,51)",
            [lane],
        )
        .unwrap();
    };
    let ids =
        |cards: &[rrss_core::articles::ArticleCard]| cards.iter().map(|c| c.id).collect::<Vec<_>>();
    // With no usable topic lane, these lower-scoring items cannot reach the
    // first page through its global/source shortlists in this single feed.
    replace_lane("null");
    let fallback = page(&conn, 40, &[]);
    let seen = ids(&fallback.0);
    assert!(fallback.1);
    assert!(!seen.iter().any(|id| matches!(id, 50 | 51)));
    let next = page(&conn, 40, &seen);
    let next_ids = ids(&next.0);
    assert!(!next_ids.is_empty());
    assert!(next_ids.iter().all(|id| !seen.contains(id)));

    for lane in [
        "7",
        "0",
        "true",
        "false",
        "0.5",
        "-3.25",
        r#""invalid""#,
        "{}",
        "[]",
    ] {
        replace_lane(lane);
        let first = page(&conn, 40, &[]);
        assert_eq!(
            (ids(&first.0), first.1),
            (seen.clone(), fallback.1),
            "lane={lane}"
        );
        let continuation = page(&conn, 40, &seen);
        assert_eq!(
            (ids(&continuation.0), continuation.1),
            (next_ids.clone(), next.1),
            "lane={lane}"
        );
        // A larger ordinary page can still retrieve the article through the
        // original channels. Invalid lane metadata must not erase its reasons.
        let all = page(&conn, 200, &[]).0;
        assert_eq!(
            all.iter().find(|c| c.id == 50).unwrap().rank_reasons,
            reasons,
            "lane={lane}"
        );
    }
}
