use rrss_app::{actions, queries};
use rrss_core::{articles, db, feeds, rules};
use rusqlite::{Connection, params};

fn setup() -> (tempfile::TempDir, db::PooledConnection) {
    let dir = tempfile::tempdir().unwrap();
    let conn = db::open_at(&dir.path().join("hidden.db")).unwrap();
    conn.execute_batch(
        "INSERT INTO feeds(id,url,title,created_at) VALUES
        (1,'https://one.invalid/feed','One',0), (2,'https://two.invalid/feed',NULL,0);",
    )
    .unwrap();
    (dir, conn)
}

fn insert(conn: &Connection, id: i64, feed: i64, score: i64, created: i64) {
    conn.execute(
        "INSERT INTO articles(id,feed_id,title,score,created_at,published_at)
        VALUES(?1,?2,'Rust news',?3,?4,?1)",
        params![id, feed, score, created],
    )
    .unwrap();
}

#[test]
fn release_and_undo_change_focus_without_changing_scores_or_rules() {
    let (_dir, conn) = setup();
    insert(&conn, 1, 1, -10, feeds::now_ms());
    let ids = || {
        articles::list_focus(&conn, 10)
            .unwrap()
            .iter()
            .map(|a| a.id)
            .collect::<Vec<_>>()
    };
    assert!(ids().is_empty());
    actions::release_article(&conn, 1).unwrap();
    assert_eq!(ids(), [1]);
    assert!(
        queries::hidden_page(&conn, None, None)
            .unwrap()
            .articles
            .is_empty()
    );
    let released: i64 = conn
        .query_row("SELECT released_at FROM articles WHERE id=1", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert!(released > 0);
    articles::set_focus_filter(&conn, articles::FocusFilter::FocusOnly).unwrap();
    assert!(ids().is_empty()); // Release bypasses hiding, not focus membership.
    conn.execute("UPDATE articles SET interest_boost=20 WHERE id=1", [])
        .unwrap();
    assert_eq!(ids(), [1]);
    actions::unrelease_article(&conn, 1).unwrap();
    assert!(ids().is_empty());
    assert_eq!(
        queries::hidden_page(&conn, None, None)
            .unwrap()
            .articles
            .len(),
        1
    );
    assert_eq!(articles::get_article(&conn, 1).unwrap().score, -10);
    assert!(
        rules::list_rules(&conn, &rules::RuleFilter::default())
            .unwrap()
            .is_empty()
    );
    assert!(matches!(
        actions::release_article(&conn, 999),
        Err(rrss_core::Error::ArticleNotFound)
    ));
    assert!(matches!(
        actions::unrelease_article(&conn, 999),
        Err(rrss_core::Error::ArticleNotFound)
    ));
}

#[test]
fn hidden_range_matches_feed_and_list_scope_and_only_returns_negative_reasons() {
    let (_dir, conn) = setup();
    let now = feeds::now_ms();
    for (id, feed, score, age) in [
        (1, 1, -10, 0),
        (2, 1, -11, 0),
        (3, 2, -12, 0),
        (4, 1, -10, 8 * 24 * 60 * 60 * 1000),
        (5, 1, -9, 0),
        (6, 1, -10, 0),
        (7, 1, -10, 0),
    ] {
        insert(&conn, id, feed, score, now - age);
    }
    conn.execute_batch(
        "UPDATE articles SET read=1,starred=1 WHERE id=2;
        UPDATE articles SET released_at=1 WHERE id=6;
        UPDATE articles SET reasons_json='invalid' WHERE id=7;",
    )
    .unwrap();
    conn.execute(
        "UPDATE articles SET reasons_json=?1 WHERE id=1",
        [r#"[
        {"rule_id":10,"dimension":"title_kw","snippet":"Rust","weight":-15},
        {"rule_id":11,"dimension":"author","snippet":"A","weight":5},
        {"rule_id":12,"dimension":"tag","snippet":"zero","weight":0}]"#],
    )
    .unwrap();
    let ids = |scope: Option<&str>| {
        queries::hidden_page(&conn, scope, None)
            .unwrap()
            .articles
            .iter()
            .map(|a| a.id)
            .collect::<Vec<_>>()
    };
    assert_eq!(ids(None), [7, 3, 2, 1]);
    assert_eq!(ids(Some("feed:1")), [7, 2, 1]);
    let page = queries::hidden_page(&conn, Some("feed:1"), None).unwrap();
    assert!(page.articles[0].reasons.is_empty());
    let article = &page.articles[2];
    assert_eq!(
        (article.id, article.score, article.published_at),
        (1, -10, Some(1))
    );
    assert_eq!(
        (article.title.as_str(), article.feed_title.as_str()),
        ("Rust news", "One")
    );
    assert_eq!(article.reasons.len(), 1);
    assert_eq!(
        (article.reasons[0].rule_id, article.reasons[0].weight),
        (10, -15)
    );
    assert_eq!(article.reasons[0].label, "标题：Rust");
    assert_eq!(
        queries::hidden_page(&conn, Some("feed:2"), None)
            .unwrap()
            .articles[0]
            .feed_title,
        "https://two.invalid/feed"
    );
    for (scope, expected) in [
        (articles::ListScope::Unread, vec![7, 1]),
        (articles::ListScope::Starred, vec![2]),
        (articles::ListScope::All, vec![7, 2, 1]),
    ] {
        articles::set_list_scope(&conn, scope).unwrap();
        assert_eq!(ids(Some("feed:1")), expected);
    }
    articles::set_hide_threshold(&conn, -11).unwrap();
    assert_eq!(ids(Some("feed:1")), [2]);
}

#[test]
fn hidden_rule_counts_count_articles_once_in_the_capped_list() {
    let (_dir, conn) = setup();
    let rule = rules::insert_rule(
        &conn,
        rules::NewRule::manual_global(rules::Polarity::Dislike, rules::Dimension::TitleKw, "Rust"),
    )
    .unwrap();
    let reasons = serde_json::json!([
        {"rule_id":rule,"dimension":"title_kw","snippet":"Rust","weight":-10},
        {"rule_id":rule,"dimension":"title_kw","snippet":"Rust","weight":-10},
        {"rule_id":99,"dimension":"author","snippet":"old","weight":-1},
        {"rule_id":100,"dimension":"tag","snippet":"positive","weight":1}])
    .to_string();
    for id in 1..=201 {
        insert(&conn, id, 1, -10, feeds::now_ms());
        conn.execute(
            "UPDATE articles SET reasons_json=?1 WHERE id=?2",
            params![reasons, id],
        )
        .unwrap();
    }
    let page = queries::hidden_page(&conn, None, None).unwrap();
    assert_eq!(page.articles.len(), 200);
    assert_eq!((page.articles[0].id, page.articles[199].id), (201, 2));
    assert_eq!(page.rule_counts.len(), 2);
    assert_eq!(
        (page.rule_counts[0].rule_id, page.rule_counts[0].count),
        (rule, 200)
    );
    assert_eq!(page.rule_counts[0].label, "标题包含『Rust』");
    assert_eq!(
        (page.rule_counts[1].rule_id, page.rule_counts[1].count),
        (99, 200)
    );
    assert_eq!(page.rule_counts[1].label, "作者：old");
}

#[test]
fn allow_rule_for_feed_copies_opposite_manual_rule_and_rescores_only_its_scope() {
    let (_dir, conn) = setup();
    for id in [1, 2] {
        insert(&conn, id, id, 0, feeds::now_ms());
    }
    let mut original =
        rules::NewRule::manual_global(rules::Polarity::Dislike, rules::Dimension::TitleKw, "Rust");
    original.weight = 17;
    original.pack_id = Some("pack".into());
    let rule_id = rules::insert_rule(&conn, original).unwrap();
    actions::rescore(&conn).unwrap();
    let allow_id = actions::allow_rule_for_feed(&conn, rule_id, 1).unwrap();
    let allow = rules::get_rule(&conn, allow_id).unwrap();
    assert_eq!(
        (allow.dimension, allow.pattern.as_str(), allow.weight),
        (rules::Dimension::TitleKw, "Rust", 17)
    );
    assert_eq!(
        (allow.polarity, allow.scope_kind, allow.scope_id),
        (rules::Polarity::Like, rules::ScopeKind::Feed, Some(1))
    );
    assert_eq!(allow.source, rules::RuleSource::Manual);
    assert!(allow.enabled);
    assert!(allow.pack_id.is_none());
    assert_eq!(articles::get_article(&conn, 1).unwrap().score, 0);
    assert_eq!(articles::get_article(&conn, 2).unwrap().score, -17);
    let reverse_id = actions::allow_rule_for_feed(&conn, allow_id, 1).unwrap();
    assert_eq!(
        rules::get_rule(&conn, reverse_id).unwrap().polarity,
        rules::Polarity::Dislike
    );
    let before = rules::list_rules(&conn, &rules::RuleFilter::default()).unwrap();
    assert!(matches!(
        actions::allow_rule_for_feed(&conn, 999, 1),
        Err(rrss_core::Error::RuleNotFound)
    ));
    assert!(matches!(
        actions::allow_rule_for_feed(&conn, rule_id, 999),
        Err(rrss_core::Error::FeedNotFound)
    ));
    assert_eq!(
        rules::list_rules(&conn, &rules::RuleFilter::default()).unwrap(),
        before
    );
}
