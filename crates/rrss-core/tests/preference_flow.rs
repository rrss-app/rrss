//! 阶段 2 集成：黄金馈源入库 → 训练规则 → 重打分 → 滑杆 → 解释。
//! 不访问网络，不启动桌面 UI。

use rrss_core::articles::{self, FocusFilter};
use rrss_core::features::extract_features;
use rrss_core::feedback::{FeedbackKind, feedback_for_article, rate_article};
use rrss_core::feeds::{SubscribeRequest, subscribe};
use rrss_core::ingest::{FetchRequest, FetchResponse, HttpFetcher, RefreshStatus, refresh_feed};
use rrss_core::rules::{
    Dimension, NewRule, Polarity, RuleFilter, RuleSource, insert_rule, list_rules,
};
use rrss_core::score::{parse_reasons, rescore_all, tags_for_article};
use rrss_core::{Result, db};

const RSS2: &str = include_str!("../../../testdata/feeds/rss2-sample.xml");

struct MockFeed;

impl HttpFetcher for MockFeed {
    fn get(&self, _req: FetchRequest) -> Result<FetchResponse> {
        Ok(FetchResponse {
            status: 200,
            etag: Some("\"golden\"".into()),
            last_modified: None,
            retry_after: None,
            body: RSS2.as_bytes().to_vec(),
        })
    }
}

fn open_temp() -> (tempfile::TempDir, rrss_core::db::PooledConnection) {
    let dir = tempfile::tempdir().unwrap();
    let conn = db::open_at(&dir.path().join("rrss.db")).unwrap();
    (dir, conn)
}

fn subscribe_news(conn: &rusqlite::Connection) -> i64 {
    subscribe(
        conn,
        SubscribeRequest {
            url: "https://news.example.com/rss.xml".into(),
            folder_id: None,
            as_rsshub: false,
            rsshub_instance_id: None,
        },
    )
    .unwrap()
}

fn titles(conn: &rusqlite::Connection) -> Vec<String> {
    articles::list_focus(conn, 20)
        .unwrap()
        .into_iter()
        .map(|c| c.title)
        .collect()
}

#[test]
fn ingest_train_rescore_slider_and_reasons() {
    let (_dir, conn) = open_temp();
    let feed_id = subscribe_news(&conn);
    let result = refresh_feed(&conn, &MockFeed, feed_id).unwrap();
    assert_eq!(
        result.status,
        RefreshStatus::Updated {
            inserted: 2,
            skipped: 0,
            derived_degraded: false,
        }
    );

    assert_eq!(
        articles::focus_filter(&conn).unwrap(),
        FocusFilter::HideFaded
    );
    let cards = articles::list_focus(&conn, 20).unwrap();
    assert_eq!(cards.len(), 2);
    assert!(cards.iter().all(|c| c.score == 0));
    assert_eq!(
        titles(&conn),
        vec![
            "Storm delays trains".to_string(),
            "City opens new library".to_string()
        ]
    );

    let library = cards.iter().find(|c| c.title.contains("library")).unwrap();
    let tags = tags_for_article(&conn, library.id).unwrap();
    assert!(tags.iter().any(|t| t == "Library"));
    assert!(tags.iter().any(|t| t == "City"));

    let article = articles::get_article(&conn, library.id).unwrap();
    let chips = extract_features(
        article.author.as_deref(),
        &tags,
        &article.title,
        article.canonical_url.as_deref(),
        None,
    );
    assert!(
        chips
            .iter()
            .any(|c| c.dimension == Dimension::Author && c.pattern == "Ada Example")
    );

    let rated = rate_article(&conn, library.id, FeedbackKind::Down).unwrap();
    assert!(rated.suggested_rule_id.is_none());
    assert_eq!(
        feedback_for_article(&conn, library.id).unwrap(),
        Some(FeedbackKind::Down)
    );
    assert!(
        list_rules(&conn, &RuleFilter::default())
            .unwrap()
            .is_empty()
    );

    insert_rule(
        &conn,
        NewRule::manual_global(Polarity::Dislike, Dimension::Author, "Ada Example"),
    )
    .unwrap();
    assert_eq!(rescore_all(&conn).unwrap(), 2);

    let faded = titles(&conn);
    assert_eq!(faded, vec!["Storm delays trains".to_string()]);

    articles::set_focus_filter(&conn, FocusFilter::ShowAll).unwrap();
    let all = articles::list_focus(&conn, 20).unwrap();
    assert_eq!(all.len(), 2);
    let ada = all.iter().find(|c| c.title.contains("library")).unwrap();
    assert_eq!(ada.score, -10);

    articles::set_focus_filter(&conn, FocusFilter::FocusOnly).unwrap();
    assert!(articles::list_focus(&conn, 20).unwrap().is_empty());

    let explained = articles::get_article(&conn, library.id).unwrap();
    let reasons = parse_reasons(explained.reasons_json.as_deref());
    assert_eq!(reasons.len(), 1);
    assert_eq!(reasons[0].dimension, Dimension::Author);
    assert_eq!(reasons[0].snippet, "Ada Example");
    assert_eq!(reasons[0].weight, -10);
}

#[test]
fn existing_rule_scores_on_ingest_without_rescore() {
    let (_dir, conn) = open_temp();
    let feed_id = subscribe_news(&conn);
    insert_rule(
        &conn,
        NewRule::manual_global(Polarity::Like, Dimension::TitleKw, "library"),
    )
    .unwrap();
    refresh_feed(&conn, &MockFeed, feed_id).unwrap();

    articles::set_focus_filter(&conn, FocusFilter::FocusOnly).unwrap();
    let focus = articles::list_focus(&conn, 20).unwrap();
    assert_eq!(focus.len(), 1);
    assert_eq!(focus[0].title, "City opens new library");
    assert_eq!(focus[0].score, 10);

    let again = refresh_feed(&conn, &MockFeed, feed_id).unwrap();
    assert_eq!(
        again.status,
        RefreshStatus::Updated {
            inserted: 0,
            skipped: 2,
            derived_degraded: false,
        }
    );
    let still = articles::get_article(&conn, focus[0].id).unwrap();
    assert_eq!(still.score, 10);
    assert_eq!(
        list_rules(
            &conn,
            &RuleFilter {
                source: Some(RuleSource::Manual),
                ..RuleFilter::default()
            }
        )
        .unwrap()
        .len(),
        1
    );
}
