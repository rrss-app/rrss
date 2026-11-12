use super::*;
use crate::views::CitationView;
use rrss_core::{retention, threads};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path as route},
};

fn setup() -> (
    tempfile::TempDir,
    std::path::PathBuf,
    crate::network::NetworkClients,
) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("web.db");
    let conn = db::open_at(&path).unwrap();
    rrss_core::network::save(&conn, &rrss_core::network::ProxyConfig::default()).unwrap();
    let network = crate::network::NetworkClients::new(&path);
    (dir, path, network)
}

#[test]
fn safety_fixtures() {
    for (url, text, mixed, mismatch, insecure) in [
        ("https://pаypal.com", "继续阅读", true, false, false), // Cyrillic а
        (
            "https://paypal.com",
            "www.paypal.com/path",
            false,
            false,
            false,
        ),
        ("https://中文.中国", "中文页面", false, false, false),
        ("https://中文.com", "中文页面", false, false, false),
        (
            "https://evil.test/path",
            "https://www.good.test/other",
            false,
            true,
            false,
        ),
        ("https://evil.test/path", "阅读原文", false, false, false),
        ("http://example.test", "example.test", false, false, true),
        (
            "https://www.example.test",
            "EXAMPLE.TEST/path",
            false,
            false,
            false,
        ),
    ] {
        let safety = link_safety(url, text).unwrap();
        assert_eq!(
            safety
                .warnings
                .iter()
                .any(|w| matches!(w, LinkWarning::MixedScript { .. })),
            mixed,
            "{url}"
        );
        assert_eq!(
            safety
                .warnings
                .iter()
                .any(|w| matches!(w, LinkWarning::HostMismatch { .. })),
            mismatch,
            "{url}"
        );
        assert_eq!(
            safety.warnings.contains(&LinkWarning::NonHttps),
            insecure,
            "{url}"
        );
        if mixed {
            assert!(
                matches!(&safety.warnings[0], LinkWarning::MixedScript { unicode, punycode } if unicode == "pаypal.com" && punycode.starts_with("xn--"))
            );
        }
    }
}

fn page(title: &str, text: &str) -> String {
    format!(
        "<html><title>{title}</title><article><h1>{title}</h1><p>{}</p><p>{}</p><p>{}</p><img src='/image.png'></article></html>",
        text.repeat(30),
        text.repeat(30),
        text.repeat(30)
    )
}

#[tokio::test]
async fn open_deduplicates_extends_collects_and_refreshes_reader_source() {
    let (dir, path, network) = setup();
    let server = MockServer::start().await;
    // A rule verifies metadata and layer reporting use the existing W2 path.
    let conn = db::open_at(&path).unwrap();
    conn.execute("INSERT INTO site_rules(domain,body,title,author,date,updated_at) VALUES('127.0.0.1','article','h1','.author','time',0)", []).unwrap();
    let html = page(
        "First title",
        "The town opened a public library with a research room. ",
    )
    .replace(
        "<article>",
        "<article><span class='author'>A Writer</span><time>2026-10-20T10:00:00Z</time>",
    );
    Mock::given(method("GET"))
        .and(route("/story"))
        .respond_with(ResponseTemplate::new(200).set_body_string(html.clone()))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(route("/image.png"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(include_bytes!("../../../../testdata/render/cover.png").to_vec()),
        )
        .mount(&server)
        .await;
    let url = format!("{}/story?utm_source=first#top", server.uri());
    let before = feeds::now_ms();
    let first = open_web_link(&network, &path, &url).await.unwrap();
    assert!(first.temporary);
    assert_eq!(first.layer, WebLinkLayer::UserRule);
    assert_eq!(first.actual_host, "127.0.0.1");
    assert_eq!(first.image_count, 1);
    assert!(first.text_chars > 200);
    assert!(
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .all(|request| request.url.path() != "/image.png")
    );
    let media = crate::queries::article_media(&conn, first.article_id).unwrap();
    assert_eq!(media.len(), 1);
    assert!(media[0].media_id.is_none() && media[0].href.is_empty());
    crate::jobs::media::prefetch_images(&path, dir.path(), &network, first.article_id, |_| {})
        .await
        .unwrap();
    let article = articles::get_article(&conn, first.article_id).unwrap();
    assert_eq!(article.author.as_deref(), Some("A Writer"));
    assert_eq!(article.published_at, Some(1792490400000));
    let media = crate::queries::article_media(&conn, first.article_id).unwrap();
    assert_eq!(media.len(), 1);
    let media_id = media[0].media_id.unwrap();
    assert!(
        media[0]
            .href
            .starts_with(&crate::queries::media_href(media_id))
    );
    let expiry: i64 = conn
        .query_row(
            "SELECT expires_at FROM articles WHERE id=?1",
            [first.article_id],
            |r| r.get(0),
        )
        .unwrap();
    assert!(expiry >= before + TEMP_LIFETIME_MS && expiry <= feeds::now_ms() + TEMP_LIFETIME_MS);
    conn.execute(
        "UPDATE articles SET expires_at=1,summary='cached',summary_hash=hash WHERE id=?1",
        [first.article_id],
    )
    .unwrap();
    let second = open_web_link(
        &network,
        &path,
        &format!("{}/story?utm_medium=again", server.uri()),
    )
    .await
    .unwrap();
    assert_eq!(second.article_id, first.article_id);
    assert!(articles::is_temporary(&conn, first.article_id).unwrap());
    assert_eq!(
        articles::get_article(&conn, first.article_id)
            .unwrap()
            .summary
            .as_deref(),
        Some("cached")
    );
    let renewed: i64 = conn
        .query_row(
            "SELECT expires_at FROM articles WHERE id=?1",
            [first.article_id],
            |r| r.get(0),
        )
        .unwrap();
    assert!(renewed >= expiry);
    crate::actions::open_article(&conn, first.article_id).unwrap();
    assert!(articles::is_temporary(&conn, first.article_id).unwrap());
    collect_article(&conn, first.article_id).unwrap();
    assert_eq!(
        rrss_core::search::search_articles(&conn, "First title", 10).unwrap()[0].id,
        first.article_id
    );
    server.reset().await;
    Mock::given(method("GET"))
        .and(route("/story"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(
                page(
                    "Updated title",
                    "The museum released new archives and a public catalogue. ",
                )
                .replace("<article>", "<article><time>2026-10-21</time>"),
            ),
        )
        .mount(&server)
        .await;
    let third = open_web_link(&network, &path, &url).await.unwrap();
    assert!(!third.temporary);
    assert_eq!(third.article_id, first.article_id);
    let current = articles::get_article(&conn, first.article_id).unwrap();
    assert_eq!(current.title, "Updated title");
    assert_eq!(current.published_at, Some(1792540800000));
    assert_ne!(
        crate::document::source_hash(&article.title, &crate::document::load_document(&article)),
        crate::document::source_hash(&current.title, &crate::document::load_document(&current))
    );
    assert!(current.summary.is_none());
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM articles", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert!(crate::queries::reader_view(&conn, first.article_id).is_ok());
}

#[tokio::test]
async fn opens_existing_subscribed_article_without_modifying_it() {
    let (_dir, path, network) = setup();
    let server = MockServer::start().await;
    let url = format!("{}/news", server.uri());
    let conn = db::open_at(&path).unwrap();
    conn.execute_batch("INSERT INTO feeds(id,url,created_at) VALUES(1,'https://feed.test/rss',0);")
        .unwrap();
    conn.execute(
        "INSERT INTO articles(id,feed_id,title,canonical_url,created_at) VALUES(1,1,'old',?1,0)",
        [&url],
    )
    .unwrap();
    conn.execute_batch(
        "UPDATE articles SET author='Original author',published_at=123,
        content_html='<p>Feed body</p>',content_md='Feed body',fulltext_md='Cached fulltext',
        hash='feed-hash',summary='Cached summary',summary_json='{}',summary_hash='summary-hash'
        WHERE id=1;",
    )
    .unwrap();
    let snapshot = || {
        conn.query_row("SELECT * FROM articles WHERE id=1", [], |row| {
            (0..row.as_ref().column_count())
                .map(|index| row.get::<_, rusqlite::types::Value>(index))
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .unwrap()
    };
    let before = snapshot();
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string(page(
            "News",
            "A new report on the public collection has been released. ",
        )))
        .mount(&server)
        .await;
    let result = open_web_link(&network, &path, &url).await.unwrap();
    assert_eq!(snapshot(), before);
    assert_eq!(result.actual_host, "127.0.0.1");
    assert!(result.text_chars > 200);
    assert_eq!(result.image_count, 1);
    assert_eq!(result.article_id, 1);
    assert!(!result.temporary);
    assert_eq!(articles::get_article(&conn, 1).unwrap().feed_id, 1);
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM feeds WHERE kind='manual'", [], |r| {
            r.get::<_, i64>(0)
        })
        .unwrap(),
        0
    );
}

#[tokio::test]
async fn open_errors_are_typed_and_do_not_insert_rows() {
    let (_dir, path, network) = setup();
    let server = MockServer::start().await;
    for (endpoint, response, kind) in [
        ("/waf", ResponseTemplate::new(403), FulltextErrorKind::Waf),
        (
            "/empty",
            ResponseTemplate::new(200).set_body_string("<html></html>"),
            FulltextErrorKind::Empty,
        ),
        (
            "/list",
            ResponseTemplate::new(200)
                .set_body_string(include_str!("../../../../testdata/web/listing.html")),
            FulltextErrorKind::NotArticle,
        ),
        (
            "/network",
            ResponseTemplate::new(407),
            FulltextErrorKind::Network,
        ),
    ] {
        Mock::given(route(endpoint))
            .respond_with(response)
            .mount(&server)
            .await;
        let error = open_web_link(&network, &path, &format!("{}{endpoint}", server.uri()))
            .await
            .unwrap_err();
        assert_eq!(error.kind, kind, "{endpoint}");
        assert!(serde_json::to_value(&error).unwrap().get("kind").is_some());
    }
    assert!(
        open_web_link(&network, &path, "file:///tmp/page.html")
            .await
            .is_err()
    );
    let conn = db::open_at(&path).unwrap();
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM articles", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn citation_and_chat_persistence_collect_before_expiry_cleanup() {
    let (dir, path, _) = setup();
    let conn = db::open_at(&path).unwrap();
    let feed = feeds::manual_feed(&conn).unwrap();
    for id in 1..=5 {
        conn.execute("INSERT INTO articles(id,feed_id,title,created_at,expires_at) VALUES(?1,?2,'quote',0,1)", params![id, feed]).unwrap();
    }
    crate::actions::cite_article(&conn, 1).unwrap();
    crate::actions::cite_selection(&conn, 2, "excerpt", 0).unwrap();
    let thread = threads::create_thread(&conn, None).unwrap();
    // Direct CitationView input bypasses the cite actions, as a restored composer can do.
    let citation = CitationView {
        kind: "span".into(),
        article_id: 3,
        title: "quote".into(),
        feed_title: "manual".into(),
        url: String::new(),
        excerpt: "excerpt".into(),
        block_id: 0,
    };
    let message_id =
        crate::actions::insert_user_message(&conn, thread.id, "question", &[citation]).unwrap();
    threads::get_or_create_anchored_thread(&conn, "quote", "article", 4).unwrap();
    assert_eq!(
        crate::actions::run_retention(&conn, dir.path(), 2, false).unwrap(),
        (1, false)
    );
    let snaps = rrss_core::cite::parse_citations_json(
        threads::get_message(&conn, message_id)
            .unwrap()
            .citations_json
            .as_deref(),
    );
    for id in 1..=4 {
        assert!(!articles::is_temporary(&conn, id).unwrap());
        assert!(articles::get_article(&conn, id).is_ok());
    }
    assert_eq!(snaps[0].article_id, 3);
    assert_eq!(retention::delete_expired(&conn, 2).unwrap(), 0);
}

#[test]
fn retention_deletes_only_expired_temporary_rows_with_cascades_even_when_disabled() {
    let (dir, path, _) = setup();
    let conn = db::open_at(&path).unwrap();
    let feed = feeds::manual_feed(&conn).unwrap();
    for (id, expiry) in [(1, Some(1)), (2, Some(3)), (3, None)] {
        conn.execute("INSERT INTO articles(id,feed_id,title,summary,created_at,expires_at) VALUES(?1,?2,'Library news','summary',0,?3)", params![id, feed, expiry]).unwrap();
    }
    conn.execute_batch("INSERT INTO chunks(article_id,ordinal,text) VALUES(1,0,'body');
        INSERT INTO article_translations(article_id,target_lang,source_hash,blocks_json,created_at) VALUES(1,'zh','hash','[]',0);
").unwrap();
    let image_dir = dir.path().join("cache/media");
    std::fs::create_dir_all(&image_dir).unwrap();
    let image_path = image_dir.join("expired.png");
    std::fs::write(&image_path, b"cached image").unwrap();
    conn.execute("INSERT INTO media(article_id,url,local_path,created_at) VALUES(1,'https://example.test/image',?1,0)", [image_path.to_str().unwrap()]).unwrap();
    // Also exercise an article whose body was already pruned.
    conn.execute("UPDATE articles SET pruned_at=0 WHERE id=1", [])
        .unwrap();
    assert!(!retention::active(&conn).unwrap());
    assert_eq!(
        crate::actions::run_retention(&conn, dir.path(), 2, false).unwrap(),
        (1, false)
    );
    assert!(!image_path.exists());
    for table in ["chunks", "article_translations", "media"] {
        assert_eq!(
            conn.query_row(
                &format!("SELECT COUNT(*) FROM {table} WHERE article_id=1"),
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
    }
    assert!(articles::get_article(&conn, 1).is_err());
    assert!(articles::get_article(&conn, 2).is_ok());
    assert!(articles::get_article(&conn, 3).is_ok());
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM articles_fts WHERE rowid=1", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn redirect_uses_actual_host_for_rules_metadata_and_deduplication() {
    let (_dir, path, network) = setup();
    let server = MockServer::start().await;
    let final_url = format!("{}/final", server.uri().replace("127.0.0.1", "localhost"));
    let conn = db::open_at(&path).unwrap();
    conn.execute(
        "INSERT INTO site_rules(domain,body,title,updated_at) VALUES('localhost','article','h1',0)",
        [],
    )
    .unwrap();
    Mock::given(route("/redirect"))
        .respond_with(ResponseTemplate::new(302).insert_header("location", final_url.as_str()))
        .mount(&server)
        .await;
    Mock::given(route("/final"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(
                page(
                    "Final",
                    "The community library hosts a new public exhibition. ",
                )
                .replace("<img src='/image.png'>", ""),
            ),
        )
        .mount(&server)
        .await;
    let result = open_web_link(&network, &path, &format!("{}/redirect", server.uri()))
        .await
        .unwrap();
    assert_eq!(result.actual_host, "localhost");
    assert_eq!(result.actual_url, final_url);
    assert_eq!(result.layer, WebLinkLayer::UserRule);
    let second = open_web_link(&network, &path, &final_url).await.unwrap();
    assert_eq!(result.article_id, second.article_id);
    // The same requested URL must keep its row even if its redirect destination changes.
    server.reset().await;
    let next_url = format!("{}/next", server.uri());
    Mock::given(route("/redirect"))
        .respond_with(ResponseTemplate::new(302).insert_header("location", next_url.as_str()))
        .mount(&server)
        .await;
    Mock::given(route("/next"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(
                page(
                    "Next",
                    "The community library updates the exhibition catalogue. ",
                )
                .replace("<img src='/image.png'>", ""),
            ),
        )
        .mount(&server)
        .await;
    let next = open_web_link(&network, &path, &format!("{}/redirect", server.uri()))
        .await
        .unwrap();
    assert_eq!(result.article_id, next.article_id);
    assert_eq!(next.actual_url, next_url);
}
