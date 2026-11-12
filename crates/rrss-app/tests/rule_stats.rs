use rrss_app::queries;
use rrss_core::{articles, db, feeds, rules};
use rusqlite::{Connection, params};

const DAY_MS: i64 = 24 * 60 * 60 * 1000;

fn setup() -> (tempfile::TempDir, db::PooledConnection) {
    let dir = tempfile::tempdir().unwrap();
    let conn = db::open_at(&dir.path().join("stats.db")).unwrap();
    conn.execute_batch(
        "INSERT INTO feeds(id,url,created_at) VALUES
        (1,'https://one.invalid/feed',0),(2,'https://two.invalid/feed',0);",
    )
    .unwrap();
    (dir, conn)
}

fn article(conn: &Connection, id: i64, feed: i64, at: i64, reasons: &str) {
    conn.execute(
        "INSERT INTO articles(id,feed_id,created_at,published_at,score,reasons_json)
        VALUES(?1,?2,?3,?1,-10,?4)",
        params![id, feed, at, reasons],
    )
    .unwrap();
}

#[test]
fn hidden_counts_match_rule_tags_across_scope_threshold_release_age_and_limit() {
    let (_dir, conn) = setup();
    let negative = rules::insert_rule(
        &conn,
        rules::NewRule::manual_global(rules::Polarity::Dislike, rules::Dimension::TitleKw, "Rust"),
    )
    .unwrap();
    let positive = rules::insert_rule(
        &conn,
        rules::NewRule::manual_global(rules::Polarity::Like, rules::Dimension::TitleKw, "Rust"),
    )
    .unwrap();
    let reasons = serde_json::json!([
        {"rule_id":negative,"dimension":"title_kw","snippet":"Rust","weight":-10},
        {"rule_id":negative,"dimension":"title_kw","snippet":"Rust","weight":-10},
        {"rule_id":positive,"dimension":"title_kw","snippet":"Rust","weight":10}
    ])
    .to_string();
    let now = feeds::now_ms();
    for id in 1..=204 {
        article(&conn, id, if id % 2 == 0 { 2 } else { 1 }, now, &reasons);
    }
    conn.execute_batch(
        "UPDATE articles SET read=1 WHERE id=201;
        UPDATE articles SET starred=1 WHERE id=200;
        UPDATE articles SET released_at=1 WHERE id=202;
        UPDATE articles SET score=0 WHERE id=204;",
    )
    .unwrap();
    conn.execute(
        "UPDATE articles SET created_at=?1 WHERE id=203",
        [now - 8 * DAY_MS],
    )
    .unwrap();
    let check = |expected| {
        let page = queries::hidden_page(&conn, None, None).unwrap();
        let stats = queries::rule_stats(&conn).unwrap();
        assert_eq!(page.articles.len() as i64, expected);
        for stat in stats.rules {
            let count = page
                .rule_counts
                .iter()
                .find(|count| count.rule_id == stat.rule_id)
                .map(|count| count.count)
                .unwrap_or(0);
            assert_eq!(stat.hidden_count, count);
            assert_eq!(
                stat.hidden_count,
                if stat.rule_id == negative {
                    expected
                } else {
                    0
                }
            );
        }
    };
    check(200);
    articles::set_list_scope(&conn, articles::ListScope::Unread).unwrap();
    check(200);
    articles::set_list_scope(&conn, articles::ListScope::Starred).unwrap();
    check(1);
    assert!(
        queries::hidden_page(&conn, Some("feed:1"), None)
            .unwrap()
            .articles
            .is_empty()
    );
    articles::set_hide_threshold(&conn, -11).unwrap();
    check(0);
}

fn exposure(conn: &Connection, article_id: i64, at: i64, variant: &str, session: &str) {
    conn.execute("INSERT INTO exposures(article_id,surface,position,score,interest_boost,rank_score,variant,session_id,shown_at)
        VALUES(?1,'focus',1,-10,0,-10,?2,?3,?4)",params![article_id,variant,session,at]).unwrap();
}

fn event(conn: &Connection, article_id: i64, at: i64, kind: &str) {
    conn.execute(
        "INSERT INTO article_events(article_id,kind,at) VALUES(?1,?2,?3)",
        params![article_id, kind, at],
    )
    .unwrap();
}

#[test]
fn hidden_spot_liked_and_count_require_open_or_up_after_spot_exposure() {
    let (_dir, conn) = setup();
    let now = feeds::now_ms();
    for id in 1..=9 {
        article(&conn, id, if id == 2 { 2 } else { 1 }, now, "[]");
    }
    for id in [1, 2, 3, 4, 8, 9] {
        exposure(&conn, id, now - 10, "spot_check", "first");
    }
    exposure(&conn, 1, now, "spot_check", "repeat");
    event(&conn, 1, now - 9, "open");
    event(&conn, 2, now - 10, "up");
    event(&conn, 3, now - 11, "up");
    event(&conn, 4, now - 9, "down");
    exposure(&conn, 5, now - 10, "default", "ordinary");
    event(&conn, 5, now, "up");
    exposure(&conn, 6, now - 31 * DAY_MS, "spot_check", "old");
    event(&conn, 6, now - 30 * DAY_MS, "open");
    event(&conn, 8, now, "up");
    event(&conn, 8, now, "down");
    event(&conn, 9, now, "open");
    conn.execute("UPDATE articles SET released_at=1 WHERE id=9", [])
        .unwrap();
    let page = queries::hidden_page(&conn, None, None).unwrap();
    let liked: Vec<_> = page
        .articles
        .iter()
        .filter(|article| article.spot_liked)
        .map(|article| article.id)
        .collect();
    assert_eq!(liked, [8, 6, 2, 1]);
    assert_eq!(page.spot_liked_count, 4);
    let page = queries::hidden_page(&conn, Some("feed:1"), None).unwrap();
    assert_eq!(page.spot_liked_count, 3);
    let stats = queries::rule_stats(&conn).unwrap().spot_check;
    assert_eq!(
        (
            stats.exposed,
            stats.liked,
            stats.disliked,
            stats.insufficient
        ),
        (6, 4, 2, true)
    );
    conn.execute("UPDATE articles SET read=1 WHERE id=1", [])
        .unwrap();
    articles::set_list_scope(&conn, articles::ListScope::Unread).unwrap();
    assert_eq!(
        queries::hidden_page(&conn, None, None)
            .unwrap()
            .spot_liked_count,
        3
    );
    let value = serde_json::to_value(queries::hidden_page(&conn, None, None).unwrap()).unwrap();
    assert_eq!(value["spot_liked_count"], 3);
    assert!(value["articles"][0]["spot_liked"].is_boolean());
}
