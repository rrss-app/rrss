use rrss_core::{db, feed_health, feeds, ingest, opml};

fn setup() -> (tempfile::TempDir, rrss_core::db::PooledConnection) {
    let dir = tempfile::tempdir().unwrap();
    let conn = db::open_at(&dir.path().join("web.db")).unwrap();
    (dir, conn)
}

#[test]
fn manual_feed_is_stable_and_never_due_or_fetched() {
    let (_dir, conn) = setup();
    let id = feeds::manual_feed(&conn).unwrap();
    assert_eq!(feeds::manual_feed(&conn).unwrap(), id);
    feeds::update_feed_url(&conn, id, "https://manual.test/edited").unwrap();
    assert_eq!(feeds::manual_feed(&conn).unwrap(), id);
    // The built-in source has no stored name; the UI supplies one.
    assert_eq!(feeds::get_feed(&conn, id).unwrap().title, None);
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
