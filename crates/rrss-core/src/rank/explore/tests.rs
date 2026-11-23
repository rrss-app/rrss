use super::*;
use crate::{db, feed_scope::FeedScope, rank::store};

fn fixture() -> (tempfile::TempDir, db::PooledConnection) {
    let dir = tempfile::tempdir().unwrap();
    let conn = db::open_at(&dir.path().join("explore.db")).unwrap();
    conn.execute_batch(
        "INSERT INTO feeds(id,url,created_at) VALUES
        (1,'https://news.example/rss',0),(2,'https://trial.example/rss',0);
        INSERT INTO folders(id,name,created_at) VALUES(1,'Folder',0);
        UPDATE feeds SET folder_id=1;",
    )
    .unwrap();
    let now = crate::feeds::now_ms();
    for id in 1..=60 {
        conn.execute(
            "INSERT INTO articles(id,feed_id,title,score,published_at,created_at)
            VALUES(?1,1,'Current report',10,?2,?2)",
            params![id, now - 1],
        )
        .unwrap();
    }
    for n in 0..10 {
        conn.execute("INSERT INTO exposures(article_id,surface,position,score,interest_boost,rank_score,variant,session_id,shown_at)
            VALUES(1,'focus',0,0,0,0,'baseline',?1,?2)",params![format!("popular-{n}"),now-1]).unwrap();
    }
    for id in 100..=106 {
        conn.execute(
            "INSERT INTO articles(id,feed_id,title,published_at,created_at)
            VALUES(?1,2,'Trial report',?2,?2)",
            params![id, now - (107 - id) * 3600000],
        )
        .unwrap();
    }
    conn.execute_batch(
        "UPDATE articles SET score=-10 WHERE id=103;
        UPDATE articles SET read=1 WHERE id=104;
        INSERT INTO article_events(article_id,kind,at) VALUES(105,'down',1);
        INSERT INTO feedback(article_id,kind,created_at) VALUES(106,'down',1);",
    )
    .unwrap();
    db::set_setting(&conn, db::HIDDEN_SPOT_CHECK_RATE_KEY, "0").unwrap();
    db::set_setting_enabled(&conn, db::CLUSTER_GROUP_ENABLED_KEY, false).unwrap();
    (dir, conn)
}

fn page(conn: &Connection, seen: &[i64], scope: FeedScope) -> Vec<ArticleCard> {
    articles::list_focus_page(conn, 20, None, seen, &[], scope)
        .unwrap()
        .0
}

fn trials(cards: &[ArticleCard]) -> Vec<i64> {
    cards.iter().filter(|c| c.explore).map(|c| c.id).collect()
}

#[test]
fn off_is_unchanged_and_only_ordinary_first_pages_explore() {
    let (_dir, conn) = fixture();
    let baseline = page(&conn, &[], FeedScope::All);
    assert!(trials(&baseline).is_empty());
    store::set_switch(&conn, store::FITTED_ENABLED_KEY, true).unwrap();
    assert_eq!(trials(&page(&conn, &[], FeedScope::All)).len(), 1);
    assert!(trials(&page(&conn, &[1], FeedScope::All)).is_empty());
    assert!(
        trials(
            &articles::list_focus_page(&conn, 20, None, &[], &[1], FeedScope::All)
                .unwrap()
                .0
        )
        .is_empty()
    );
    assert!(trials(&page(&conn, &[], FeedScope::Folder(1))).is_empty());
    articles::set_list_scope(&conn, articles::ListScope::Starred).unwrap();
    conn.execute("UPDATE articles SET starred=1", []).unwrap();
    assert!(trials(&page(&conn, &[], FeedScope::All)).is_empty());
    articles::set_list_scope(&conn, articles::ListScope::All).unwrap();
    conn.execute("UPDATE articles SET starred=0", []).unwrap();
    store::set_switch(&conn, store::FITTED_ENABLED_KEY, false).unwrap();
    assert_eq!(
        serde_json::to_value(page(&conn, &[], FeedScope::All)).unwrap(),
        serde_json::to_value(baseline).unwrap()
    );
}

#[test]
fn daily_trial_is_latest_eligible_preserves_order_and_has_no_duplicate() {
    let (_dir, conn) = fixture();
    let baseline = page(&conn, &[], FeedScope::All);
    store::set_switch(&conn, store::FITTED_ENABLED_KEY, true).unwrap();
    for scope in [articles::ListScope::All, articles::ListScope::Unread] {
        articles::set_list_scope(&conn, scope).unwrap();
        let first = page(&conn, &[], FeedScope::All);
        assert_eq!(trials(&first), vec![102]);
        assert_eq!(
            serde_json::to_value(&first).unwrap(),
            serde_json::to_value(page(&conn, &[], FeedScope::All)).unwrap()
        );
        let position = first.iter().position(|c| c.explore).unwrap();
        assert!((4..=14).contains(&position));
        assert_eq!(
            first
                .iter()
                .filter(|c| !c.explore)
                .map(|c| c.id)
                .collect::<Vec<_>>(),
            baseline.iter().map(|c| c.id).collect::<Vec<_>>()
        );
        assert_eq!(first.iter().filter(|c| c.id == 102).count(), 1);
        assert!(!first[position].spot_check);
        assert_eq!(first[position].cluster_id, None);
    }
    articles::set_focus_filter(&conn, articles::FocusFilter::ShowAll).unwrap();
    assert_eq!(trials(&page(&conn, &[], FeedScope::All)), vec![102]);
    let all = page(&conn, &[], FeedScope::All);
    let mut short = all[..3].to_vec();
    inject(&conn, &mut short, &[102], -10, crate::feeds::now_ms()).unwrap();
    assert!(trials(&short).is_empty());
    conn.execute_batch("UPDATE articles SET read=1 WHERE feed_id=2;")
        .unwrap();
    assert!(trials(&page(&conn, &[], FeedScope::All)).is_empty());
}

#[test]
fn admission_threshold_counts_recent_baseline_and_trial_focus_exposures() {
    let (_dir, conn) = fixture();
    store::set_switch(&conn, store::FITTED_ENABLED_KEY, true).unwrap();
    let now = crate::feeds::now_ms();
    // High-exposure news must not take the trial when the rare feed is excluded.
    for (surface, variant, at, count) in [
        ("focus", "baseline", now - 1, 6),
        ("focus", "explore", now - 1, 3),
        ("focus", "spot_check", now - 1, 10),
        ("feeds", "baseline", now - 1, 10),
        ("focus", "baseline", now - 31 * DAY_MS, 10),
        ("focus", "baseline", now + DAY_MS, 10),
    ] {
        for n in 0..count {
            conn.execute("INSERT INTO exposures(article_id,surface,position,score,interest_boost,rank_score,variant,session_id,shown_at)
                VALUES(100,?1,0,0,0,0,?2,?3,?4)", params![surface,variant,format!("{surface}-{variant}-{at}-{n}"),at]).unwrap();
        }
    }
    assert_eq!(trials(&page(&conn, &[], FeedScope::All)), vec![102]);
    conn.execute("INSERT INTO exposures(article_id,surface,position,score,interest_boost,rank_score,variant,session_id,shown_at)
        VALUES(100,'focus',0,0,0,0,'explore','tenth',?1)", [now-1]).unwrap();
    // Past trials count too, so one rare feed is not explored indefinitely.
    assert!(trials(&page(&conn, &[], FeedScope::All)).is_empty());
}

#[test]
fn thompson_uses_article_outcomes_and_reading_from_any_entry() {
    let (_dir, conn) = fixture();
    conn.execute("DELETE FROM exposures", []).unwrap();
    let baseline = page(&conn, &[], FeedScope::All);
    let now = crate::feeds::now_ms();
    for id in 51..=55 {
        conn.execute("UPDATE articles SET fulltext_md='body' WHERE id=?1", [id])
            .unwrap();
        conn.execute("INSERT INTO exposures(article_id,surface,position,score,interest_boost,rank_score,variant,session_id,shown_at)
            VALUES(?1,'focus',4,0,0,0,'baseline',?2,?3)",params![id,format!("read-{id}"),now-7200000]).unwrap();
        conn.execute("INSERT INTO read_sessions(article_id,opened_at,active_ms,idle_ms,read_chars,skimmed_chars,total_chars,max_block,reached_end,translated,ended_by,via)
            VALUES(?1,?2,200000,0,900,0,1000,0,1,0,'back','feeds')",params![id,now-3600000]).unwrap();
    }
    // Four trials of one unread article are one outcome, not four negatives.
    for n in 0..4 {
        conn.execute("INSERT INTO exposures(article_id,surface,position,score,interest_boost,rank_score,variant,session_id,shown_at)
            VALUES(100,'focus',4,0,0,0,'explore',?1,?2)", params![format!("skip-{n}"),now-7200000]).unwrap();
    }
    let mut read_source_wins = 0;
    for day in 0..16 {
        let at = now + day * DAY_MS;
        let seed_day = at.div_euclid(DAY_MS).to_string();
        let expected = if beta(spot_check::daily_hash(1, &seed_day), 5, 0)
            > beta(spot_check::daily_hash(2, &seed_day), 0, 1)
        {
            1
        } else {
            2
        };
        let mut cards = baseline.clone();
        inject(&conn, &mut cards, &[1, 102], -10, at).unwrap();
        let trial = cards.iter().find(|c| c.explore).unwrap();
        assert_eq!(trial.feed_id, expected);
        read_source_wins += usize::from(expected == 1);
    }
    println!(
        "thompson known_posteriors n=16 read_source_wins={read_source_wins} reads=5 skipped_articles=1"
    );
}

#[test]
fn integer_beta_sampler_recovers_mean_and_variance() {
    for (reads, skips) in [(0, 0), (2, 6)] {
        let samples: Vec<_> = (1..=10000)
            .map(|feed| beta(spot_check::daily_hash(feed, "20261120"), reads, skips))
            .collect();
        assert!(samples.iter().all(|x| *x > 0.0 && *x < 1.0));
        let mean = samples.iter().sum::<f64>() / samples.len() as f64;
        let variance =
            samples.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / samples.len() as f64;
        let (a, b) = ((reads + 1) as f64, (skips + 1) as f64);
        assert!((mean - a / (a + b)).abs() < 0.01);
        assert!((variance - a * b / ((a + b).powi(2) * (a + b + 1.0))).abs() < 0.003);
        println!(
            "beta reads={reads} skips={skips} n={} mean={mean:.6} variance={variance:.6}",
            samples.len()
        );
    }
}
