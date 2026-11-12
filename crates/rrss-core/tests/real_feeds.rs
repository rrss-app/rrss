//! 真实馈源快照：解析、OPML 导入、入库。不访问网络。

use rrss_core::articles::{self, FocusFilter};
use rrss_core::feeds::{SubscribeRequest, subscribe};
use rrss_core::ingest::{FetchRequest, FetchResponse, HttpFetcher, RefreshStatus, refresh_feed};
use rrss_core::opml::import_opml;
use rrss_core::{FeedKind, Result, db, parse_feed_bytes};

const HNRSS: &str = include_str!("../../../testdata/feeds/real/hnrss-frontpage.xml");
const SOLIDOT: &str = include_str!("../../../testdata/feeds/real/solidot.xml");
const SSPAI: &str = include_str!("../../../testdata/feeds/real/sspai.xml");
const LWN: &str = include_str!("../../../testdata/feeds/real/lwn.xml");
const RUANYIFENG: &str = include_str!("../../../testdata/feeds/real/ruanyifeng-atom.xml");
const V2EX: &str = include_str!("../../../testdata/feeds/real/v2ex-atom.xml");
const RSSHUB_DIV_BR: &str = include_str!("../../../testdata/feeds/real/rsshub-div-br.xml");
const JSONFEED: &str = include_str!("../../../testdata/feeds/real/jsonfeed-org.json");
const REAL_OPML: &str = include_str!("../../../testdata/feeds/opml/real.opml");

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
fn parses_real_rss_atom_json() {
    let hn = parse_feed_bytes(HNRSS.as_bytes(), Some("https://hnrss.org/frontpage")).unwrap();
    assert_eq!(hn.wire_kind, FeedKind::Rss);
    assert!(hn.title.as_deref().unwrap_or("").contains("Hacker News"));
    assert!(hn.entries.len() >= 10);
    assert!(hn.entries.iter().any(|e| e.canonical_url.is_some()));

    let solidot = parse_feed_bytes(
        SOLIDOT.as_bytes(),
        Some("https://www.solidot.org/index.rss"),
    )
    .unwrap();
    assert_eq!(solidot.wire_kind, FeedKind::Rss);
    assert!(solidot.title.as_deref().unwrap_or("").contains("Solidot"));
    assert!(solidot.entries.len() >= 10);
    let html = solidot.entries[0].content_html.as_deref().unwrap_or("");
    assert!(html.contains('<') || html.contains("&lt;"));

    let sspai = parse_feed_bytes(SSPAI.as_bytes(), Some("https://sspai.com/feed")).unwrap();
    assert_eq!(sspai.wire_kind, FeedKind::Rss);
    assert_eq!(sspai.title.as_deref(), Some("少数派"));
    assert!(sspai.entries.len() >= 5);

    let lwn = parse_feed_bytes(LWN.as_bytes(), Some("https://lwn.net/headlines/rss")).unwrap();
    assert_eq!(lwn.wire_kind, FeedKind::Rss);
    assert!(lwn.title.as_deref().unwrap_or("").contains("LWN"));
    assert!(lwn.entries.len() >= 5);

    let ruanyifeng = parse_feed_bytes(
        RUANYIFENG.as_bytes(),
        Some("https://www.ruanyifeng.com/blog/atom.xml"),
    )
    .unwrap();
    assert_eq!(ruanyifeng.wire_kind, FeedKind::Atom);
    assert!(ruanyifeng.title.as_deref().unwrap_or("").contains("阮一峰"));
    assert!(!ruanyifeng.entries.is_empty());
    let html = ruanyifeng.entries[0].content_html.as_deref().unwrap_or("");
    assert!(html.contains("<p") || html.contains("<P") || html.contains("&lt;p"));

    let v2ex = parse_feed_bytes(V2EX.as_bytes(), Some("https://www.v2ex.com/index.xml")).unwrap();
    assert_eq!(v2ex.wire_kind, FeedKind::Atom);
    assert_eq!(v2ex.title.as_deref(), Some("V2EX"));
    assert!(v2ex.entries.len() >= 10);

    let json = parse_feed_bytes(
        JSONFEED.as_bytes(),
        Some("https://www.jsonfeed.org/feed.json"),
    )
    .unwrap();
    assert_eq!(json.wire_kind, FeedKind::Json);
    assert_eq!(json.title.as_deref(), Some("JSON Feed"));
    assert_eq!(json.entries.len(), 2);
    assert_eq!(json.entries[0].title, "JSON Feed version 1.1");
}

#[test]
fn rsshub_div_br_description_splits_into_paragraphs() {
    let feed = parse_feed_bytes(
        RSSHUB_DIV_BR.as_bytes(),
        Some("https://rsshub.example/feed.xml"),
    )
    .unwrap();
    assert_eq!(feed.entries.len(), 1);
    let html = feed.entries[0].content_html.as_deref().unwrap_or("");
    assert!(
        html.contains("<div") || html.contains("第一段内容"),
        "{html}"
    );
    let doc = rrss_render::html_to_document(html, &rrss_render::RenderOpts::default());
    let plains: Vec<String> = doc
        .blocks
        .iter()
        .filter_map(|b| match b {
            rrss_render::Block::Paragraph { inlines } => {
                Some(rrss_render::inlines_to_plain(inlines))
            }
            _ => None,
        })
        .collect();
    assert!(
        plains.len() >= 3,
        "RSSHub-style div/br should split: {plains:?}"
    );
    assert!(
        plains.iter().any(|p| p.contains("第一段内容")),
        "{plains:?}"
    );
    assert!(
        plains.iter().any(|p| p.contains("第三段在双 br 之后")),
        "{plains:?}"
    );
}

#[test]
fn imports_real_opml_tree() {
    let (_dir, conn) = open_temp();
    let report = import_opml(&conn, REAL_OPML).unwrap();
    assert_eq!(report.imported, 10);
    assert_eq!(report.folders, 3);
    assert_eq!(report.skipped_same_url, 0);
    let feeds = rrss_core::feeds::list_feeds(&conn).unwrap();
    assert!(
        feeds
            .iter()
            .any(|f| f.url == "https://www.solidot.org/index.rss")
    );
    assert!(
        feeds
            .iter()
            .any(|f| f.url == "https://www.jsonfeed.org/feed.json")
    );
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
    let cards = articles::list_focus(&conn, 50).unwrap();
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
