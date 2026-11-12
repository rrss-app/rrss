use rrss_core::{
    articles, cluster, db, digest, feed_health, feeds, ingest, interests, opml, rag, rank, search,
    signals, timeline,
};
use rusqlite::params;

fn setup() -> (tempfile::TempDir, rrss_core::db::PooledConnection) {
    let dir = tempfile::tempdir().unwrap();
    let conn = db::open_at(&dir.path().join("web.db")).unwrap();
    (dir, conn)
}

#[test]
fn temporary_articles_are_excluded_from_every_automatic_entry() {
    let (_dir, conn) = setup();
    let feed = feeds::manual_feed(&conn).unwrap();
    for (id, expiry) in [(1, Some(99999)), (2, None)] {
        conn.execute("INSERT INTO articles(id,feed_id,title,content_html,score,created_at,published_at,expires_at) VALUES(?1,?2,'Library AI news','<p>Library AI body</p>',10,1,1,?3)", params![id, feed, expiry]).unwrap();
    }
    conn.execute("UPDATE articles SET created_at=?1", [feeds::now_ms()])
        .unwrap();
    articles::set_focus_filter(&conn, articles::FocusFilter::ShowAll).unwrap();
    interests::set_cluster_group_enabled(&conn, false).unwrap();
    assert_eq!(
        articles::list_focus(&conn, 20)
            .unwrap()
            .iter()
            .map(|r| r.id)
            .collect::<Vec<_>>(),
        vec![2]
    );
    let (rows, _) = timeline::list_feeds_page(
        &conn,
        rrss_core::feed_scope::FeedScope::All,
        articles::ListScope::All,
        None,
        20,
    )
    .unwrap();
    assert_eq!(rows.iter().map(|r| r.id).collect::<Vec<_>>(), vec![2]);
    assert_eq!(timeline::feed_counts(&conn).unwrap().total, 1);
    // >= 3 chars exercises FTS, two chars exercises LIKE fallback.
    for query in ["Library", "AI"] {
        assert_eq!(
            search::search_articles(&conn, query, 20)
                .unwrap()
                .iter()
                .map(|r| r.id)
                .collect::<Vec<_>>(),
            vec![2]
        );
        assert_eq!(
            search::search_articles_for_retrieval(&conn, query, 20)
                .unwrap()
                .iter()
                .map(|r| r.id)
                .collect::<Vec<_>>(),
            vec![2]
        );
    }
    let window = digest::DigestWindow {
        since_ms: 0,
        read_limit: 0,
        exclude_daily_before: None,
    };
    assert_eq!(
        digest::digest_candidates(&conn, 20, &window)
            .unwrap()
            .iter()
            .map(|r| r.article_id)
            .collect::<Vec<_>>(),
        vec![2]
    );
    let summaries = articles::due_focus_summaries(&conn, 20).unwrap();
    assert_eq!(summaries.iter().map(|r| r.id).collect::<Vec<_>>(), vec![2]);
    assert_eq!(
        rag::due_embed_articles(&conn, 20)
            .unwrap()
            .iter()
            .map(|r| r.id)
            .collect::<Vec<_>>(),
        vec![2]
    );
    let fake = rag::DueEmbedArticle {
        id: 1,
        title: "Library".into(),
        summary: None,
        blocks_json: None,
        content_html: Some("<p>body</p>".into()),
        tier: 1,
    };
    assert!(
        rag::sync_article_chunks(&conn, &fake, fake.tier)
            .unwrap()
            .is_empty()
    );
    for article in rag::due_embed_articles(&conn, 20).unwrap() {
        rag::sync_article_chunks(&conn, &article, article.tier).unwrap();
    }
    let count = |table: &str, id: i64| {
        conn.query_row(
            &format!("SELECT COUNT(*) FROM {table} WHERE article_id=?1"),
            [id],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
    };
    assert_eq!(count("chunks", 1), 0);
    assert!(count("chunks", 2) > 0);
    for id in [1, 2] {
        let ids = signals::record_exposures(
            &conn,
            &[signals::Exposure {
                article_id: id,
                surface: signals::Surface::Focus,
                position: 1,
                variant: "test".into(),
                session_id: "session".into(),
                shown_at: 2,
            }],
        )
        .unwrap();
        assert_eq!(ids[0].is_some(), id == 2);
        assert_eq!(
            signals::record_events(
                &conn,
                &[signals::ArticleEvent {
                    article_id: id,
                    kind: signals::EventKind::Open,
                    exposure_id: ids[0],
                    at: 3
                }]
            )
            .unwrap(),
            usize::from(id == 2)
        );
        assert_eq!(
            signals::record_read_sessions(
                &conn,
                &[signals::ReadSession {
                    article_id: id,
                    exposure_id: ids[0],
                    opened_at: 3,
                    active_ms: 1000,
                    idle_ms: 0,
                    read_chars: 10,
                    skimmed_chars: 0,
                    total_chars: 20,
                    max_block: 0,
                    reached_end: true,
                    translated: false,
                    ended_by: signals::EndedBy::Back,
                    via: signals::Via::Focus
                }]
            )
            .unwrap(),
            usize::from(id == 2)
        );
        rrss_core::feedback::rate_article(&conn, id, rrss_core::feedback::FeedbackKind::Up)
            .unwrap();
        assert_eq!(count("feedback", id), i64::from(id == 2));
    }
    rank::store::rebuild(&conn, feeds::now_ms()).unwrap();
    assert_eq!(count("article_rank", 1), 0);
    assert_eq!(count("article_rank", 2), 1);
    cluster::backfill_simhash(&conn, 20).unwrap();
    cluster::assign_articles(&conn, &[1, 2]).unwrap();
    assert_eq!(cluster::assign_article(&conn, 1).unwrap(), 0);
    let cluster_id = |id| {
        conn.query_row("SELECT cluster_id FROM articles WHERE id=?1", [id], |r| {
            r.get::<_, Option<i64>>(0)
        })
        .unwrap()
    };
    assert!(cluster_id(1).is_none());
    assert!(cluster_id(2).is_some());
    assert_eq!(
        signals::load_session_summaries(&conn, 0, 100)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        timeline::mark_scope_read(&conn, rrss_core::feed_scope::FeedScope::All).unwrap(),
        vec![2]
    );
    assert!(!articles::get_article(&conn, 1).unwrap().read);
    // Direct reader access remains available.
    assert!(articles::get_article(&conn, 1).is_ok());
}

#[test]
fn manual_feed_is_stable_and_never_due_or_fetched() {
    let (_dir, conn) = setup();
    let id = feeds::manual_feed(&conn).unwrap();
    assert_eq!(feeds::manual_feed(&conn).unwrap(), id);
    feeds::update_feed_url(&conn, id, "https://manual.test/edited").unwrap();
    assert_eq!(feeds::manual_feed(&conn).unwrap(), id);
    assert_eq!(
        feeds::get_feed(&conn, id).unwrap().title.as_deref(),
        Some("手动收录")
    );
    assert!(ingest::due_feeds(&conn, i64::MAX / 2).unwrap().is_empty());
    struct NeverFetch;
    impl ingest::HttpFetcher for NeverFetch {
        fn get(&self, _: ingest::FetchRequest) -> rrss_core::Result<ingest::FetchResponse> {
            panic!("manual feed was fetched")
        }
    }
    assert_eq!(
        ingest::refresh_feed(&conn, &NeverFetch, id).unwrap().status,
        ingest::RefreshStatus::NotModified
    );
}

#[test]
fn manual_feed_has_no_health_warning() {
    let (_dir, conn) = setup();
    let id = feeds::manual_feed(&conn).unwrap();
    conn.execute(
        "UPDATE feeds SET created_at=0,fail_count=100,failing_since=0,error='failed' WHERE id=?1",
        [id],
    )
    .unwrap();
    assert!(
        feed_health::feed_health(&conn, 100 * 86_400_000)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn opml_does_not_export_manual_feed_even_in_folder() {
    let (_dir, conn) = setup();
    let id = feeds::manual_feed(&conn).unwrap();
    let folder = feeds::create_folder(&conn, "Saved", None).unwrap();
    feeds::set_folder(&conn, id, Some(folder)).unwrap();
    let xml = opml::export_opml(&conn).unwrap();
    assert!(!xml.contains(feeds::MANUAL_URL));
    assert!(!xml.contains("手动收录"));
}
