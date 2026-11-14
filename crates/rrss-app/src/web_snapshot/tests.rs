use super::*;
use rrss_core::{articles, db, media};
use rusqlite::params;

const PAGE: &str = "https://snapshot.example.test/story";
const QP: &[u8] = include_bytes!("../../../../testdata/web/snapshot-quoted-printable.mhtml");
const B64: &[u8] = include_bytes!("../../../../testdata/web/snapshot-base64.mhtml");
const GBK: &[u8] = include_bytes!("../../../../testdata/web/snapshot-gbk.mhtml");
const QP_SOFT_BREAK: &[u8] =
    include_bytes!("../../../../testdata/web/snapshot-quoted-printable-soft-break.mhtml");

fn archive(html: &str, image: Option<(&str, &[u8])>) -> Vec<u8> {
    let mut raw = format!(
        "MIME-Version: 1.0\r\nContent-Type: multipart/related;\r\n boundary=\"test-boundary\"\r\n\r\n--test-boundary\r\nContent-Type: text/html; charset=utf-8\r\nContent-Transfer-Encoding: base64\r\n\r\n{}\r\n",
        base64::engine::general_purpose::STANDARD.encode(html),
    );
    if let Some((url, bytes)) = image {
        raw += &format!(
            "--test-boundary\r\nContent-Type: image/png\r\nContent-Location: {url}\r\nContent-Transfer-Encoding: base64\r\n\r\n{}\r\n",
            base64::engine::general_purpose::STANDARD.encode(bytes)
        );
    }
    raw += "--test-boundary--\r\n";
    raw.into_bytes()
}

#[test]
fn browser_fixtures_select_main_html_and_decode_resources() {
    let url = Url::parse(PAGE).unwrap();
    let qp = parse_mhtml(QP, &url).unwrap();
    let base64 = parse_mhtml(B64, &url).unwrap();
    assert_eq!(qp.html, base64.html);
    assert!(qp.html.contains("Snapshot 测试"));
    assert!(!qp.html.contains("Subframe"));
    assert_eq!(qp.images.len(), 1);
    assert_eq!(qp.images[0].url, "https://snapshot.example.test/image.png");
    assert_eq!(qp.images[0].bytes, base64.images[0].bytes);
    assert_eq!(media::sniff_image_ext(&qp.images[0].bytes), Some("png"));
    assert!(parse_mhtml(GBK, &url).unwrap().html.contains("中文测试"));
    // Location-less HTML falls back to the first HTML; location-less images
    // cannot acquire an invented remote cache key.
    let missing = String::from_utf8(B64.to_vec())
        .unwrap()
        .replace(&format!("Content-Location: {PAGE}\r\n"), "")
        .replace(
            "Content-Location: https://snapshot.example.test/image.png\r\n",
            "",
        );
    let missing = parse_mhtml(missing.as_bytes(), &url).unwrap();
    assert_eq!(missing.html, qp.html);
    assert!(missing.images.is_empty());
}

#[test]
fn quoted_printable_accepts_nonstandard_soft_line_breaks() {
    let snapshot = parse_mhtml(QP_SOFT_BREAK, &PAGE.parse().unwrap()).unwrap();
    assert_eq!(
        snapshot.html,
        "<html><body><p>Snapshot softline</p></body></html>"
    );
    assert_eq!(snapshot.skipped_images, 0);
}

#[test]
fn damaged_archives_are_errors_without_panics() {
    let url = Url::parse(PAGE).unwrap();
    for raw in [
        b"not MIME".as_slice(),
        b"Content-Type: multipart/related\r\n\r\n",
        b"Content-Type: text/html\r\nContent-Transfer-Encoding: base64\r\n\r\n%%%",
        b"Content-Type: text/html\r\nContent-Transfer-Encoding: unknown\r\n\r\nhello",
        b"Content-Type: image/png\r\n\r\nno HTML",
        &B64[..B64.len() - 30],
    ] {
        assert!(parse_mhtml(raw, &url).is_err());
    }
    let mut nested = b"Content-Type: text/html\r\n\r\ntext".to_vec();
    for depth in 0..10 {
        nested = [
            format!("Content-Type: multipart/related; boundary=b{depth}\r\n\r\n--b{depth}\r\n")
                .into_bytes(),
            nested,
            format!("\r\n--b{depth}--\r\n").into_bytes(),
        ]
        .concat();
    }
    assert_eq!(
        parse_mhtml(&nested, &url).unwrap_err().code(),
        "web_snapshot_mime_limit"
    );
}

#[test]
fn snapshot_image_size_and_count_limits() {
    let url = Url::parse(PAGE).unwrap();
    let mut raw = archive("<p>test</p>", None);
    raw.resize(MAX_SNAPSHOT_BYTES, b' ');
    assert!(parse_mhtml(&raw, &url).is_ok());
    raw.push(b' ');
    assert_eq!(
        parse_mhtml(&raw, &url).unwrap_err().code(),
        "web_snapshot_too_large"
    );
    drop(raw);
    let image = vec![0; media::MAX_IMAGE_BYTES];
    let snapshot = parse_mhtml(&archive("test", Some(("/big.png", &image))), &url).unwrap();
    assert_eq!(snapshot.images.len(), 1);
    assert_eq!(snapshot.skipped_images, 0);
    let image = vec![0; media::MAX_IMAGE_BYTES + 1];
    let mut raw = archive("test", Some(("/big.png", &image)));
    raw.truncate(raw.len() - b"--test-boundary--\r\n".len());
    raw.extend_from_slice(b"--test-boundary\r\nContent-Type: image/png\r\nContent-Location: /small.png\r\nContent-Transfer-Encoding: base64\r\n\r\naGk=\r\n--test-boundary--\r\n");
    let snapshot = parse_mhtml(&raw, &url).unwrap();
    assert_eq!(snapshot.html, "test");
    assert_eq!(snapshot.images.len(), 1);
    assert_eq!(
        snapshot.images[0].url,
        "https://snapshot.example.test/small.png"
    );
    assert_eq!(snapshot.skipped_images, 1);
    let image = "--many\r\nContent-Type: image/png\r\nContent-Location: /small.png\r\nContent-Transfer-Encoding: base64\r\n\r\naGk=\r\n";
    let prefix = "Content-Type: multipart/related; boundary=many\r\n\r\n";
    for count in [200, 201, 203] {
        // The main HTML comes after the images: skipping must keep scanning MIME.
        let raw = format!(
            "{prefix}{}--many\r\nContent-Type: text/html\r\n\r\ntext\r\n--many--\r\n",
            image.repeat(count)
        );
        let snapshot = parse_mhtml(raw.as_bytes(), &url).unwrap();
        assert_eq!(snapshot.html, "text");
        assert_eq!(snapshot.images.len(), 200);
        assert_eq!(snapshot.skipped_images, count - 200);
    }
}

fn setup() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("snapshot.db");
    let conn = db::open_at(&path).unwrap();
    rrss_core::network::save(&conn, &rrss_core::network::ProxyConfig::default()).unwrap();
    (dir, path)
}

fn article(title: &str, image: &str) -> String {
    let paragraph =
        "The town library opened a public exhibition with a new research catalogue. ".repeat(30);
    format!(
        "<html><title>Browser title</title><article class='entry'><h1>{title}</h1><p>{paragraph}</p><p>{paragraph}</p><p>{paragraph}</p><img src='{image}'></article></html>"
    )
}

#[test]
fn snapshots_reuse_rules_dedup_expiry_collection_and_record_refresh_time() {
    let (dir, path) = setup();
    let conn = db::open_at(&path).unwrap();
    conn.execute("INSERT INTO site_rules(domain,body,title,updated_at) VALUES('snapshot.example.test','.entry','h1',0)", []).unwrap();
    let first = store_snapshot(
        &path,
        dir.path(),
        PAGE,
        &archive(&article("First", "/image.png"), None),
    )
    .unwrap();
    assert_eq!(first.layer, crate::WebLinkLayer::UserRule);
    assert!(first.temporary);
    assert!(first.web_extracted_at.unwrap() > 0);
    let reader = crate::queries::reader_view(&conn, first.article_id).unwrap();
    assert_eq!(reader.title, "First");
    assert_eq!(reader.web_extracted_at, first.web_extracted_at);
    let first_hash = reader.source_hash;
    conn.execute(
        "UPDATE articles SET expires_at=1,web_extracted_at=1 WHERE id=?1",
        [first.article_id],
    )
    .unwrap();
    let second = store_snapshot(
        &path,
        dir.path(),
        PAGE,
        &archive(&article("Second", "/image.png"), None),
    )
    .unwrap();
    assert_eq!(first.article_id, second.article_id);
    assert!(second.web_extracted_at.unwrap() > 1);
    let reader = crate::queries::reader_view(&conn, second.article_id).unwrap();
    assert_eq!(reader.title, "Second");
    assert_ne!(reader.source_hash, first_hash);
    let expiry: i64 = conn
        .query_row(
            "SELECT expires_at FROM articles WHERE id=?1",
            [second.article_id],
            |r| r.get(0),
        )
        .unwrap();
    assert!(expiry >= second.web_extracted_at.unwrap() + 7 * 86_400_000);
    crate::web::collect_article(&conn, second.article_id).unwrap();
    let collected = store_snapshot(
        &path,
        dir.path(),
        PAGE,
        &archive(&article("Third", "/image.png"), None),
    )
    .unwrap();
    assert!(!collected.temporary);
    let http = crate::web::store_page(
        &path,
        PAGE,
        &article("HTTP", "/image.png"),
        &PAGE.parse().unwrap(),
        None,
    )
    .unwrap();
    assert_eq!(http.article_id, first.article_id);
    assert_eq!(http.web_extracted_at, None);
    assert_eq!(
        crate::queries::reader_view(&conn, http.article_id)
            .unwrap()
            .web_extracted_at,
        None
    );
}

#[test]
fn subscription_articles_are_reused_without_rewriting_content_or_metadata() {
    let (dir, path) = setup();
    let conn = db::open_at(&path).unwrap();
    conn.execute(
        "INSERT INTO feeds(url,created_at) VALUES('https://feed.example.test/rss',0)",
        [],
    )
    .unwrap();
    let feed = conn.last_insert_rowid();
    conn.execute("INSERT INTO articles(feed_id,canonical_url,title,fulltext_md,created_at) VALUES(?1,?2,'Subscription','Original',0)", params![feed, PAGE]).unwrap();
    let id = conn.last_insert_rowid();
    let result = store_snapshot(
        &path,
        dir.path(),
        PAGE,
        &archive(&article("Snapshot", "/image.png"), None),
    )
    .unwrap();
    assert_eq!(result.article_id, id);
    assert!(!result.temporary);
    assert_eq!(result.web_extracted_at, None);
    let stored = articles::get_article(&conn, id).unwrap();
    assert_eq!(stored.title, "Subscription");
    assert_eq!(stored.fulltext_md.as_deref(), Some("Original"));
}

#[test]
fn shield_suspicion_and_bad_images_do_not_commit_articles() {
    let (dir, path) = setup();
    for raw in [
        archive("<title>Just a moment</title><p>Checking</p>", None),
        archive("<html><p>short</p></html>", None),
        archive(
            &article("Invalid image", "/image.png"),
            Some(("/image.png", b"not an image")),
        ),
    ] {
        assert!(store_snapshot(&path, dir.path(), PAGE, &raw).is_err());
        let conn = db::open_at(&path).unwrap();
        assert_eq!(
            conn.query_row("SELECT count(*) FROM articles", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
}

#[tokio::test]
async fn prefetch_images_hits_snapshot_cache_without_any_network_request() {
    use wiremock::{Mock, MockServer, ResponseTemplate, matchers::any};
    let (dir, path) = setup();
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;
    let url = format!("{}/story", server.uri());
    let image_url = format!("{}/image.png", server.uri());
    let png = parse_mhtml(B64, &PAGE.parse().unwrap())
        .unwrap()
        .images
        .remove(0)
        .bytes;
    let result = store_snapshot(
        &path,
        dir.path(),
        &url,
        &archive(&article("Cached", &image_url), Some((&image_url, &png))),
    )
    .unwrap();
    let network = crate::network::NetworkClients::new(&path);
    let images =
        crate::jobs::media::prefetch_images(&path, dir.path(), &network, result.article_id, |_| {})
            .await
            .unwrap();
    assert_eq!(images.len(), 1);
    assert!(!images[0].href.is_empty());
    assert_ne!(images[0].href, image_url);
    let conn = db::open_at(&path).unwrap();
    let cached = media::cached_path(&conn, result.article_id, &image_url)
        .unwrap()
        .unwrap();
    assert_eq!(std::fs::read(cached).unwrap(), png);
    assert!(server.received_requests().await.unwrap().is_empty());
}
