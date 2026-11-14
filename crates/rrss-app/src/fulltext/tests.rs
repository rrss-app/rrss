use super::*;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const GBK_PAGE: &[u8] = include_bytes!("../../../../testdata/web/gbk.html");

fn clients(dir: &Path) -> crate::network::NetworkClients {
    let path = dir.join("test.db");
    let conn = rrss_core::db::open_at(&path).unwrap();
    rrss_core::network::save(&conn, &rrss_core::network::ProxyConfig::default()).unwrap();
    crate::network::NetworkClients::new(&path)
}

#[test]
fn detects_cloudflare_challenge() {
    assert!(detect_waf(
        "<html><title>Just a moment...</title></html>",
        200
    ));
}

#[test]
fn gbk_fixture_decodes_and_extracts() {
    let html = decode_page(GBK_PAGE, "text/html");
    assert!(html.contains("公园发布秋季自然观察报告"));
    let article =
        crate::extract::extract_article(&html, &"https://example.test/news".parse().unwrap(), None)
            .unwrap();
    assert!(article.suspicious.is_empty());
    assert!(rrss_render::document_to_plain(&article.document).contains("调查持续了数周"));
}

#[test]
fn charset_precedence_aliases_and_invalid_labels() {
    let source = "<meta charset=big5><p>简体中文新闻</p>";
    let (bytes, _, errors) = encoding_rs::GBK.encode(source);
    assert!(!errors);
    assert_eq!(decode_page(&bytes, "text/html; CHARSET = 'gb2312'"), source);
    for (encoding, label, text) in [
        (encoding_rs::BIG5, "big5", "繁體中文新聞"),
        (encoding_rs::GB18030, "gb18030", "中文扩展𠀀"),
    ] {
        let source = format!(
            "<meta http-equiv='CONTENT-TYPE' content='text/html; charset={label}'><p>{text}</p>"
        );
        let (bytes, _, errors) = encoding.encode(&source);
        assert!(!errors);
        assert_eq!(decode_page(&bytes, "text/html; charset=unknown"), source);
    }
    let source = "<meta charset=unknown><p>UTF-8 默认编码</p>";
    assert_eq!(decode_page(source.as_bytes(), "text/html"), source);
}

#[test]
fn meta_scan_is_bounded_and_ignores_comments_and_scripts() {
    for prefix in [
        "<!-- <meta charset=gbk> -->".to_string(),
        "<script>const fake = '<meta charset=gbk>';</script>".to_string(),
        format!("{}<meta charset=gbk>", " ".repeat(CHARSET_SCAN_BYTES)),
    ] {
        let source = format!("{prefix}<p>保留正确的默认编码</p>");
        assert_eq!(decode_page(source.as_bytes(), ""), source);
    }
}

#[tokio::test]
async fn fetch_page_decodes_response_charset() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/gbk"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(GBK_PAGE)
                .insert_header("Content-Type", "text/html; charset=gbk"),
        )
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let html = fetch_page(&clients(dir.path()), &format!("{}/gbk", server.uri()))
        .await
        .unwrap();
    assert!(html.contains("公园发布秋季自然观察报告"));
}

#[tokio::test]
async fn html_limit_accepts_exact_size_and_rejects_oversize_and_gzip() {
    use std::io::Write;
    let server = MockServer::start().await;
    let bytes = vec![b' '; MAX_HTML_BYTES];
    Mock::given(path("/exact"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(bytes))
        .mount(&server)
        .await;
    Mock::given(path("/large"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![b'x'; MAX_HTML_BYTES + 1]))
        .mount(&server)
        .await;
    let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    gzip.write_all(&vec![b'x'; MAX_HTML_BYTES + 1]).unwrap();
    Mock::given(path("/gzip"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(gzip.finish().unwrap())
                .insert_header("Content-Encoding", "gzip"),
        )
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let network = clients(dir.path());
    assert_eq!(
        fetch_page(&network, &format!("{}/exact", server.uri()))
            .await
            .unwrap()
            .len(),
        MAX_HTML_BYTES
    );
    for path in ["large", "gzip"] {
        let error = fetch_page(&network, &format!("{}/{path}", server.uri()))
            .await
            .unwrap_err();
        assert_eq!(error.kind, FulltextErrorKind::Network);
        assert_eq!(error.ui_error().code(), "fulltext_too_large");
    }
}

#[tokio::test]
async fn oversized_chunked_page_stops_before_eof() {
    use tokio::io::AsyncWriteExt;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/page", listener.local_addr().unwrap());
    let (release, wait) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        crate::test_net::read_request_head(&mut stream).await;
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n")
            .await
            .unwrap();
        let chunk = vec![b'x'; 64 * 1024];
        for _ in 0..=MAX_HTML_BYTES / chunk.len() {
            if stream.write_all(b"10000\r\n").await.is_err()
                || stream.write_all(&chunk).await.is_err()
                || stream.write_all(b"\r\n").await.is_err()
            {
                return;
            }
        }
        let _ = wait.await;
    });
    let dir = tempfile::tempdir().unwrap();
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        fetch_page(&clients(dir.path()), &url),
    )
    .await;
    let _ = release.send(());
    server.await.unwrap();
    let error = result.expect("must stop before EOF").unwrap_err();
    assert_eq!(error.kind, FulltextErrorKind::Network);
    assert_eq!(error.ui_error().code(), "fulltext_too_large");
}

#[tokio::test]
async fn waf_pages_are_rejected_before_storage() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let network = clients(dir.path());
    let conn = rrss_core::db::open_at(&dir.path().join("test.db")).unwrap();
    conn.execute(
        "INSERT INTO feeds (id, url, created_at) VALUES (1, 'https://example.test/rss', 1)",
        [],
    )
    .unwrap();
    conn.execute("INSERT INTO articles (id, feed_id, title, hash, created_at, canonical_url) VALUES (1, 1, 'Title', 'hash', 1, ?1)", [format!("{}/challenge", server.uri())]).unwrap();
    for status in [200, 403, 429, 503] {
        server.reset().await;
        Mock::given(path("/challenge"))
            .respond_with(
                ResponseTemplate::new(status).set_body_string("<title>Just a moment...</title>"),
            )
            .mount(&server)
            .await;
        assert_eq!(
            fetch_and_store(&network, &dir.path().join("test.db"), 1)
                .await
                .unwrap_err()
                .kind,
            FulltextErrorKind::Waf
        );
        let article = articles::get_article(&conn, 1).unwrap();
        assert!(article.fulltext_md.is_none());
        assert!(article.blocks_json.is_none());
    }
}

#[test]
fn fulltext_errors_have_one_typed_ui_projection() {
    use crate::ui_error::web::WebError;
    for (error, code) in [
        (WebError::FulltextProxyAuth.into(), "fulltext_proxy_auth"),
        (WebError::FulltextTooLarge.into(), "fulltext_too_large"),
    ] {
        let fulltext = FulltextError::coded(FulltextErrorKind::Network, Some(error));
        assert_eq!(crate::UiError::from(fulltext.clone()).code(), code);
        assert_eq!(fulltext.clone().ui_error().code(), code);
        let wire = serde_json::to_value(fulltext).unwrap();
        assert_eq!(wire["error"]["code"], code);
        assert!(wire.get("message").is_none());
    }
    for (fulltext, code, detail) in [
        (
            FulltextError::new(FulltextErrorKind::Waf, "http 403"),
            "reader_fulltext_waf",
            "http 403",
        ),
        (
            FulltextError::network("fixture diagnostic"),
            "reader_fulltext_network",
            "fixture diagnostic",
        ),
    ] {
        let error = crate::UiError::from(fulltext);
        assert_eq!(error.code(), code);
        assert_eq!(error.detail, detail);
    }
}
