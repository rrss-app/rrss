use rrss_core::articles::{self, ArticleCard, ListScope, ROW_CARD};
use rrss_core::{db, feed_scope::FeedScope, feeds::now_ms};
use rusqlite::{Connection, params};

pub(super) fn fixture() -> (tempfile::TempDir, db::PooledConnection) {
    let dir = tempfile::tempdir().unwrap();
    let conn = db::open_at(&dir.path().join("focus.db")).unwrap();
    db::set_setting(&conn, "pi_auto_download", "0").unwrap();
    db::set_setting(&conn, db::HIDDEN_SPOT_CHECK_RATE_KEY, "0").unwrap();
    conn.execute_batch(
        "INSERT INTO feeds(id,url,created_at) VALUES
        (1,'https://daily.example/rss',0),(2,'https://weekly.example/rss',0),
        (3,'https://monthly.example/rss',0);",
    )
    .unwrap();
    (dir, conn)
}

fn fill(conn: &Connection, feed: i64, count: i64, published: i64) {
    conn.execute(
        "WITH RECURSIVE seq(n) AS (
        SELECT 1 UNION ALL SELECT n+1 FROM seq WHERE n<?2)
        INSERT INTO articles(feed_id,title,published_at,created_at)
        SELECT ?1,'story ' || n,?3+n,?4 FROM seq",
        params![feed, count, now_ms() - 60_000 + published, now_ms()],
    )
    .unwrap();
}

pub(super) fn page(conn: &Connection, limit: i64, seen: &[i64]) -> (Vec<ArticleCard>, bool) {
    crate::list_focus_page(conn, limit, None, seen, &[], FeedScope::All).unwrap()
}

#[test]
fn quiet_sources_compete_outside_the_global_window_and_in_small_libraries() {
    for news in [8, 42, 130] {
        let (_dir, conn) = fixture();
        fill(&conn, 1, news, 3000);
        fill(&conn, 2, 2, 2000);
        fill(&conn, 3, 1, 1000);
        let (first, more) = page(&conn, 40, &[]);
        let top: Vec<_> = first.iter().take(3).map(|c| c.feed_id).collect();
        assert_eq!(top, [1, 2, 3]);
        assert_eq!(more, news + 3 > 40);
        conn.execute("UPDATE articles SET score=10 WHERE feed_id=1", [])
            .unwrap();
        assert!(
            page(&conn, 40, &[])
                .0
                .iter()
                .take(news.min(40) as usize)
                .all(|c| c.feed_id == 1)
        );
    }
}

#[test]
fn diversity_continues_through_both_pages_and_single_source_tail_is_reachable() {
    let (_dir, conn) = fixture();
    fill(&conn, 1, 130, 2000);
    fill(&conn, 2, 60, 1000);
    let mut shown = Vec::new();
    let mut feeds = Vec::new();
    loop {
        let (next, more) = page(&conn, 40, &shown);
        for c in next {
            assert!(!shown.contains(&c.id));
            shown.push(c.id);
            feeds.push(c.feed_id);
        }
        if !more {
            break;
        }
    }
    assert_eq!(shown.len(), 190);
    for section in [&feeds[..10], &feeds[10..40], &feeds[40..80]] {
        assert!(section.contains(&1) && section.contains(&2));
    }
    assert!(feeds[..80].windows(3).all(|w| w[0] != w[1] || w[1] != w[2]));
    // Two already displayed daily articles should affect the very next slot.
    assert_eq!(page(&conn, 1, &[1, 2]).0[0].feed_id, 2);
    assert_eq!(page(&conn, 1, &[]).0[0].feed_id, 1);
}

#[test]
fn default_focus_page_order_snapshot() {
    let (_dir, conn) = fixture();
    fill(&conn, 1, 8, 3000);
    fill(&conn, 2, 4, 2000);
    fill(&conn, 3, 2, 1000);
    // Three sources compete across page boundaries, followed by a single-source tail.
    // A one-card request has a shorter candidate shortlist than the larger pages.
    for (size, expected) in [
        (1, [8, 12, 7, 6, 11, 5, 4, 10, 3, 2, 9, 14, 1, 13]),
        (4, [8, 12, 14, 7, 11, 13, 6, 10, 5, 4, 9, 3, 2, 1]),
        (10, [8, 12, 14, 7, 11, 13, 6, 10, 5, 4, 9, 3, 2, 1]),
        (40, [8, 12, 14, 7, 11, 13, 6, 10, 5, 4, 9, 3, 2, 1]),
    ] {
        let ids: Vec<_> = collect_pages(&conn, size).iter().map(|c| c.id).collect();
        assert_eq!(ids, expected, "page size {size}");
    }
}

#[test]
fn read_and_repeated_exposure_costs_are_scoped_and_expire() {
    let (_dir, conn) = fixture();
    fill(&conn, 1, 4, 0);
    conn.execute_batch(
        "UPDATE articles SET starred=1;
        UPDATE articles SET score=10,read=1 WHERE id=4;",
    )
    .unwrap();
    let order = || {
        page(&conn, 10, &[])
            .0
            .iter()
            .map(|c| c.id)
            .collect::<Vec<_>>()
    };
    let now = now_ms();
    let expose = |id, session, surface: &str, at| {
        conn.execute("INSERT INTO exposures(article_id,surface,position,score,interest_boost,rank_score,variant,session_id,shown_at)
        VALUES(?1,?2,0,0,0,0,'baseline',?3,?4)", params![id,surface,session,at]).unwrap();
    };
    expose(3, 1, "focus", now);
    expose(3, 2, "focus", now);
    expose(3, 3, "feeds", now);
    expose(3, 4, "focus", now - 8 * 86_400_000);
    assert_eq!(order(), [3, 2, 1, 4]);
    expose(3, 5, "focus", now);
    assert_eq!(order(), [3, 2, 1, 4]); // Three sessions, but one UTC day.
    expose(3, 6, "focus", now - 86_400_000);
    assert_eq!(order(), [3, 2, 1, 4]);
    expose(3, 7, "focus", now - 2 * 86_400_000);
    assert_eq!(order(), [2, 1, 4]);
    articles::set_list_scope(&conn, ListScope::Starred).unwrap();
    assert_eq!(order(), [4, 3, 2, 1]);
    articles::set_list_scope(&conn, ListScope::Unread).unwrap();
    assert_eq!(order(), [2, 1]);
    conn.execute("UPDATE exposures SET shown_at=0", []).unwrap();
    assert_eq!(order(), [3, 2, 1]);
}

#[test]
fn cluster_representative_and_expansion_follow_the_list_scope() {
    let (_dir, conn) = fixture();
    conn.execute_batch(
        "INSERT INTO clusters(id,member_count,created_at) VALUES(7,3,0);
        INSERT INTO articles(id,feed_id,title,read,starred,score,cluster_id,created_at) VALUES
        (1,1,'previous report',1,0,10,7,0),(2,2,'update',0,1,1,7,0),
        (3,3,'another update',0,0,0,7,0);",
    )
    .unwrap();
    conn.execute("UPDATE articles SET created_at=?1", [now_ms()])
        .unwrap();
    let cards = |expanded| {
        crate::list_focus_page(&conn, 1, expanded, &[], &[], FeedScope::All)
            .unwrap()
            .0
            .into_iter()
            .filter(|c| c.row_kind == ROW_CARD)
            .map(|c| c.id)
            .collect::<Vec<_>>()
    };
    assert_eq!(cards(None), [2]);
    articles::set_list_scope(&conn, ListScope::Unread).unwrap();
    assert_eq!(cards(None), [2]);
    assert_eq!(cards(Some(7)), [2, 3]);
    articles::set_list_scope(&conn, ListScope::Starred).unwrap();
    assert_eq!(cards(None), [2]);
    assert_eq!(
        rrss_core::cluster_summary::visible_members(&conn, 7, FeedScope::All)
            .unwrap()
            .len(),
        3
    );
    assert_eq!(
        articles::mark_cluster_read(&conn, 7, FeedScope::All).unwrap(),
        [2]
    );
}

#[test]
fn shortlists_preserve_source_positions_after_grouping_and_total_order_ties() {
    let (_dir, conn) = fixture();
    conn.execute_batch(
        "INSERT INTO articles(id,feed_id,cluster_id,created_at) VALUES
        (11,1,1,0),(10,2,1,0),(9,1,2,0),(8,3,2,0),
        (7,1,NULL,0),(6,1,NULL,0),(5,2,NULL,0),(4,2,NULL,0),
        (3,3,NULL,0),(2,3,NULL,0),(1,1,NULL,0);",
    )
    .unwrap();
    let shortlist = |group, limit, eligible| {
        super::candidates(
            &conn,
            &crate::store::settings(&conn).unwrap(),
            eligible,
            group,
            ListScope::Starred,
            &[limit, 0, 1],
            &[],
            &[],
            None,
        )
        .unwrap()
        .into_iter()
        .map(|(id, _)| id)
        .collect::<Vec<_>>()
    };
    // Equal score and NULL publication dates fall back to descending IDs.
    // Losing group members must not consume their sources' first positions.
    assert_eq!(shortlist(true, 2, "a.id > ?2"), [11, 9, 5]);
    assert_eq!(shortlist(true, 3, "a.id > ?2"), [11, 9, 7, 5, 3]);
    assert_eq!(shortlist(true, 4, "a.id > ?2"), [11, 9, 7, 6, 5, 3]);
    assert_eq!(shortlist(true, 7, "a.id > ?2"), [11, 9, 7, 6, 5, 4, 3, 2]);
    assert_eq!(
        shortlist(true, 50, "a.id > ?2"),
        [11, 9, 7, 6, 5, 4, 3, 2, 1]
    );
    assert_eq!(shortlist(false, 3, "a.id > ?2"), [11, 10, 9, 8]);
    assert_eq!(shortlist(true, 2, "a.id > ?2 AND a.feed_id != 1"), [10, 8]);
    assert!(shortlist(true, 2, "a.id < ?2").is_empty());
    conn.execute("UPDATE articles SET published_at=1 WHERE id=5", [])
        .unwrap();
    assert_eq!(shortlist(true, 2, "a.id > ?2"), [5, 11]);
}

#[test]
fn shortlists_stop_consuming_a_saved_library_after_both_lists_are_decided() {
    let consumed = std::cell::Cell::new(0);
    let rows = (0..100_000).map(|n| {
        consumed.set(consumed.get() + 1);
        Ok((100_000 - n, n, None, 0))
    });
    let selected = super::shortlist(rows, 2, true, &[], &[]).unwrap();
    assert_eq!(selected, [(100_000, 0), (99_999, 0)]);
    assert_eq!(consumed.get(), 2);
}

#[test]
fn exposure_probe_counts_only_qualifying_utc_days() {
    let (_dir, conn) = fixture();
    fill(&conn, 1, 2, 0);
    let now = now_ms();
    for (session, surface, variant, at) in [
        (1, "focus", "baseline", now - 8 * 86_400_000),
        (2, "focus", "baseline", now + 86_400_000),
        (3, "feeds", "baseline", now),
        (4, "focus", "spot_check", now),
        (8, "focus", "explore", now),
        (9, "focus", "explore", now),
        (10, "focus", "explore", now),
        (5, "focus", "baseline", now),
        (6, "focus", "baseline", now),
    ] {
        conn.execute(
            "INSERT INTO exposures(article_id,surface,position,score,interest_boost,rank_score,
             variant,session_id,shown_at) VALUES(2,?1,0,0,0,0,?2,?3,?4)",
            params![surface, variant, session, at],
        )
        .unwrap();
    }
    assert_eq!(page(&conn, 1, &[]).0[0].id, 2);
    // Two more sessions on today's UTC day still count as one day.
    let midnight = (now / 86_400_000 - 2) * 86_400_000;
    for (session, at) in [("7", now), ("11", midnight + 1), ("12", midnight - 1)] {
        conn.execute(
            "INSERT INTO exposures(article_id,surface,position,score,interest_boost,rank_score,
             variant,session_id,shown_at) VALUES(2,'focus',0,0,0,0,'baseline',?1,?2)",
            params![session, at],
        )
        .unwrap();
        if session != "12" {
            assert_eq!(page(&conn, 1, &[]).0[0].id, 2); // At most two UTC days so far.
        }
    }
    assert_eq!(page(&conn, 1, &[]).0[0].id, 1);
}

fn collect_pages(conn: &Connection, size: i64) -> Vec<ArticleCard> {
    let mut result = Vec::new();
    let mut seen = Vec::new();
    loop {
        let (cards, more) = page(conn, size, &seen);
        assert!(!more || !cards.is_empty());
        for card in &cards {
            assert!(!seen.contains(&card.id), "duplicate {}", card.id);
            seen.push(card.id);
        }
        result.extend(cards);
        assert!(seen.len() <= 2200, "pool never exhausts");
        if !more {
            assert!(page(conn, size, &seen).0.is_empty());
            break;
        }
    }
    result
}

#[test]
fn flood_is_finite_and_quality_lane_preserves_older_relevant_work() {
    let (_dir, conn) = fixture();
    fill(&conn, 1, 3000, 0);
    fill(&conn, 2, 2, -7 * 86_400_000);
    fill(&conn, 3, 1, -14 * 86_400_000);
    // The oldest relevant story is far outside the newest 256 from this source.
    conn.execute("UPDATE articles SET score=10 WHERE id=1", [])
        .unwrap();
    let first = page(&conn, 40, &[]).0;
    assert_eq!(first[0].id, 1);
    assert!(first.iter().take(10).any(|c| c.feed_id == 2));
    assert!(first.iter().take(10).any(|c| c.feed_id == 3));
    let all = collect_pages(&conn, 40);
    assert_eq!(all.len(), 163);
    assert_eq!(all.iter().filter(|c| c.feed_id == 1).count(), 160);
    let mut ids: Vec<_> = all.iter().map(|c| c.id).collect();
    ids.sort_unstable();
    let mut other: Vec<_> = collect_pages(&conn, 17).iter().map(|c| c.id).collect();
    other.sort_unstable();
    assert_eq!(ids, other, "membership must not depend on page size");
    assert_eq!(
        conn.query_row("SELECT count(*) FROM articles", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        3003
    );
}

#[test]
fn dates_recovery_exposure_and_explicit_saved_retrieval_have_distinct_boundaries() {
    let (_dir, conn) = fixture();
    let now = now_ms();
    let day = 86_400_000;
    for (id, age, created_age, published, read, score) in [
        (1, 1, 1, true, false, 0),
        (2, 8, 8, true, true, 100),  // old read
        (3, 50, 0, true, false, 10), // dated backfill keeps its old age
        (4, 51, 51, true, false, 10),
        (5, 52, 52, true, false, 10), // third recovery does not replenish on page two
        (6, 181, 0, true, false, 100),
        (7, 0, 400, false, false, 0), // unknown date is old ingestion, never today
        (8, 0, 1, false, false, 0),
        (9, -400, 40, true, false, 0), // future publication capped by ingestion
        (10, -400, 1, true, false, 0),
        (11, 45, 45, true, false, 20), // previously shown, not a missed article
    ] {
        conn.execute(
            "INSERT INTO articles(id,feed_id,title,published_at,created_at,read,score,starred)
            VALUES(?1,1,'date fixture',?2,?3,?4,?5,1)",
            params![
                id,
                published.then_some(now - age * day),
                now - created_age * day,
                read,
                score
            ],
        )
        .unwrap();
    }
    conn.execute(
        "INSERT INTO exposures(article_id,surface,position,score,interest_boost,rank_score,
        variant,session_id,shown_at) VALUES(11,'focus',0,0,0,0,'baseline','older',?1)",
        [now - 20 * day],
    )
    .unwrap();
    let mut ids: Vec<_> = collect_pages(&conn, 2).iter().map(|c| c.id).collect();
    ids.sort_unstable();
    assert_eq!(ids, [1, 3, 4, 8, 10]);
    // Editing content and publication to the future cannot refresh first ingestion.
    conn.execute(
        "UPDATE articles SET title='updated',published_at=?1 WHERE id=7",
        [now + day],
    )
    .unwrap();
    assert!(!collect_pages(&conn, 40).iter().any(|c| c.id == 7));
    articles::set_list_scope(&conn, ListScope::Starred).unwrap();
    assert_eq!(collect_pages(&conn, 3).len(), 11);
}

#[test]
fn dormant_neutral_recovery_ignores_only_identified_freshness_penalties() {
    let (_dir, conn) = fixture();
    let now = now_ms();
    let day = 86_400_000;
    conn.execute(
        "INSERT INTO articles(id,feed_id,title,published_at,created_at) VALUES
        (1,1,'today',?1,?1),(2,2,'older neutral',?2,?2),(3,2,'oldest neutral',?3,?3),
        (4,3,'negative preference',?2,?2)",
        params![now, now - 40 * day, now - 50 * day],
    )
    .unwrap();
    crate::store::rebuild(&conn, now).unwrap();
    assert_eq!(
        conn.query_row(
            "SELECT boost FROM article_rank WHERE article_id=2",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        -2
    );
    let ids = || {
        collect_pages(&conn, 40)
            .iter()
            .map(|c| c.id)
            .collect::<Vec<_>>()
    };
    assert_eq!(ids(), [1, 4, 2]); // one neutral per dormant source, freshness still orders it later
    conn.execute(
        "UPDATE article_rank SET boost=-4,reasons_json=?1 WHERE article_id=4",
        [
            r#"[{"component":"freshness","code":"rank_freshness","params":{},"points":-2},
            {"component":"source","code":"rank_source","params":{},"points":-2}]"#,
        ],
    )
    .unwrap();
    assert_eq!(ids(), [1, 2]);
    // A hidden recent publication still makes source 2 active.
    conn.execute(
        "INSERT INTO articles(id,feed_id,title,score,published_at,created_at,read,expires_at)
        VALUES(5,2,'hidden and expired recent publication',-100,?1,?1,1,?1)",
        [now],
    )
    .unwrap();
    assert_eq!(ids(), [1]);
    // Positive old work remains eligible for active feeds and takes precedence.
    conn.execute("UPDATE articles SET score=1 WHERE id=3", [])
        .unwrap();
    assert_eq!(ids(), [1, 3]);
    assert_eq!(page(&conn, 40, &[]).0[1].rank_boost, -2);
}

#[test]
fn final_global_budget_and_recovery_reserve_are_exhaustible() {
    let (_dir, conn) = fixture();
    let now = now_ms();
    conn.execute("WITH RECURSIVE n(id) AS (SELECT 4 UNION ALL SELECT id+1 FROM n WHERE id<40)
        INSERT INTO feeds(id,url,created_at) SELECT id,'https://source' || id || '.example/rss',?1 FROM n", [now]).unwrap();
    conn.execute(
        "WITH RECURSIVE n(id) AS (SELECT 1 UNION ALL SELECT id+1 FROM n WHERE id<200)
        INSERT INTO articles(feed_id,title,published_at,created_at)
        SELECT f.id,'current item ' || n.id,?1-n.id,?1 FROM feeds f CROSS JOIN n",
        [now],
    )
    .unwrap();
    conn.execute(
        "WITH RECURSIVE n(id) AS (SELECT 1 UNION ALL SELECT id+1 FROM n WHERE id<3)
        INSERT INTO articles(feed_id,title,score,published_at,created_at)
        SELECT f.id,'missed item ' || n.id,10,?1-n.id,?1 FROM feeds f CROSS JOIN n",
        [now - 40 * 86_400_000],
    )
    .unwrap();
    let all = collect_pages(&conn, 500);
    assert_eq!(all.len(), 2048);
    assert_eq!(all.iter().filter(|c| c.score == 10).count(), 64);
    let mut counts = std::collections::HashMap::<i64, usize>::new();
    for card in all {
        *counts.entry(card.feed_id).or_default() += 1;
    }
    assert_eq!(counts.len(), 40);
    assert!(counts.values().all(|n| *n <= 162));
}
