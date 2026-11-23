//! 真实馈源快照入库后进入焦点页。不访问网络；快照与解析、OPML 的测试在 core 的 `real_feeds`。

use rrss_core::articles::{self, FocusFilter};
use rrss_core::feeds::{SubscribeRequest, subscribe};
use rrss_core::ingest::{FetchRequest, FetchResponse, HttpFetcher, RefreshStatus, refresh_feed};
use rrss_core::{Result, db};

const SOLIDOT: &str = include_str!("../../../testdata/feeds/real/solidot.xml");

struct Snapshot {
    body: &'static str,
}

impl HttpFetcher for Snapshot {
    fn get(&self, _req: FetchRequest) -> Result<FetchResponse> {
        Ok(FetchResponse {
            status: 200,
            etag: Some("\"real\"".into()),
            last_modified: None,
            retry_after: None,
            body: self.body.as_bytes().to_vec(),
        })
    }
}

fn open_temp() -> (tempfile::TempDir, rrss_core::db::PooledConnection) {
    let dir = tempfile::tempdir().unwrap();
    let conn = db::open_at(&dir.path().join("rrss.db")).unwrap();
    (dir, conn)
}

#[test]
fn ingesting_solidot_snapshot_fills_focus() {
    let (_dir, conn) = open_temp();
    articles::set_focus_filter(&conn, FocusFilter::ShowAll).unwrap();
    let id = subscribe(
        &conn,
        SubscribeRequest {
            url: "https://www.solidot.org/index.rss".into(),
            folder_id: None,
            as_rsshub: false,
            rsshub_instance_id: None,
        },
    )
    .unwrap();
    let result = refresh_feed(&conn, &Snapshot { body: SOLIDOT }, id).unwrap();
    match result.status {
        RefreshStatus::Updated {
            inserted, skipped, ..
        } => {
            assert!(inserted >= 10, "inserted={inserted}");
            assert_eq!(skipped, 0);
        }
        other => panic!("unexpected status {other:?}"),
    }
    // Preserve the archived source and relative publication times, but replay
    // this temporary database in the current recommendation window.
    let latest: i64 = conn
        .query_row("SELECT MAX(published_at) FROM articles", [], |r| r.get(0))
        .unwrap();
    conn.execute(
        "UPDATE articles SET published_at=published_at+?1",
        [rrss_core::feeds::now_ms() - 60_000 - latest],
    )
    .unwrap();
    let cards = rrss_focus::list_focus(&conn, 50).unwrap();
    assert!(cards.len() >= 10);
    assert!(cards.iter().any(|c| !c.title.is_empty()));
    let article = articles::get_article(&conn, cards[0].id).unwrap();
    assert!(article.content_html.as_deref().unwrap_or("").len() > 10);
    assert!(
        article
            .blocks_json
            .as_deref()
            .unwrap_or("")
            .starts_with('{')
            || article
                .blocks_json
                .as_deref()
                .unwrap_or("")
                .starts_with('[')
    );
}
