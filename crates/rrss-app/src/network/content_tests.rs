use super::*;
use rrss_core::network::ProxyRule;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const PNG: &[u8] = include_bytes!("../../../../testdata/render/cover.png");

fn seed(dir: &std::path::Path, html: &str) -> PathBuf {
    let path = dir.join("content.db");
    let conn = db::open_at(&path).unwrap();
    conn.execute(
        "INSERT INTO feeds (id, url, created_at) VALUES (1, 'http://content.invalid/rss', 1)",
        [],
    )
    .unwrap();
    conn.execute("INSERT INTO articles (id, feed_id, title, hash, created_at, canonical_url, content_html, starred) VALUES (1, 1, 'Title', 'h', 1, 'http://content.invalid/article', ?1, 1)", [html]).unwrap();
    path
}

#[tokio::test]
async fn reader_downloads_use_content_and_keep_headers_cache_and_next_task_switching() {
    let first = MockServer::start().await;
    let next = MockServer::start().await;
    let unused = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let image = "http://content.invalid/image.png";
    let video = "http://content.invalid/movie.mp4";
    let html = format!(
        "<article><p>{}</p><img src='{image}'/><video src='{video}'></video></article>",
        "Local article. ".repeat(25)
    );
    let db_path = seed(dir.path(), &html);
    let conn = db::open_at(&db_path).unwrap();
    let mut config = ProxyConfig {
        global: ProxyRule::custom(&unused.uri()),
        content: ProxyRule::custom(&first.uri()),
        api: ProxyRule::custom(&unused.uri()),
        pi: ProxyRule::custom(&unused.uri()),
        ..Default::default()
    };
    network::save(&conn, &config).unwrap();
    let clients = NetworkClients::new(&db_path);
    Mock::given(method("GET"))
        .and(path("/article"))
        .and(|request: &wiremock::Request| {
            request
                .headers
                .get("user-agent")
                .is_some_and(|value| value == crate::image_fetch::IMAGE_UA)
        })
        .and(header("referer", "http://content.invalid/article"))
        .respond_with(ResponseTemplate::new(200).set_body_string(html))
        .expect(1)
        .mount(&first)
        .await;
    Mock::given(method("GET"))
        .and(path("/image.png"))
        .and(|request: &wiremock::Request| {
            request
                .headers
                .get("user-agent")
                .is_some_and(|value| value == crate::image_fetch::IMAGE_UA)
        })
        .and(header("referer", "http://content.invalid/article"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(PNG))
        .expect(1)
        .mount(&first)
        .await;
    crate::fulltext::fetch_and_store(&clients, &db_path, 1)
        .await
        .unwrap();
    let refs = crate::jobs::media::prefetch_images(&db_path, dir.path(), &clients, 1, |_| {})
        .await
        .unwrap();
    assert_eq!(refs.len(), 1);
    assert!(!refs[0].href.is_empty());
    assert!(!refs[0].href.starts_with("http://content.invalid"));

    config.content = ProxyRule::custom(&next.uri());
    network::save(&conn, &config).unwrap();
    Mock::given(method("GET"))
        .and(path("/movie.mp4"))
        .and(|request: &wiremock::Request| {
            request
                .headers
                .get("user-agent")
                .is_some_and(|value| value == crate::image_fetch::IMAGE_UA)
        })
        .and(header("referer", "http://content.invalid/article"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(b"local media", "video/mp4"))
        .expect(1)
        .mount(&next)
        .await;
    let media = crate::embed_fetch::download_direct(
        &clients,
        &db_path,
        dir.path(),
        1,
        video,
        "http://content.invalid/article",
    )
    .await
    .unwrap();
    assert!(!media.href.is_empty());
    assert!(!media.href.starts_with("http://content.invalid"));
    // 本地已缓存媒体无需网络配置；坏配置不使已下载内容失效。
    db::set_setting(&conn, network::SETTING_KEY, "broken-private-secret").unwrap();
    let cached = crate::embed_fetch::download_direct(&clients, &db_path, dir.path(), 1, video, "")
        .await
        .unwrap();
    assert_eq!(cached.href, media.href);
    let expected = refs[0].href.clone();
    let again =
        crate::jobs::media::prefetch_images(&db_path, dir.path(), &clients, 1, move |event| {
            assert_eq!(event.media.href, expected);
        })
        .await
        .unwrap();
    assert_eq!(again[0].href, refs[0].href);
    let error = crate::fulltext::fetch_and_store(&clients, &db_path, 1)
        .await
        .unwrap_err();
    assert!(!format!("{error:?} {error}").contains("private-secret"));
    assert_eq!(first.received_requests().await.unwrap().len(), 2);
    assert_eq!(next.received_requests().await.unwrap().len(), 1);
    assert!(unused.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn image_preload_uses_content_and_skips_cached_images() {
    let proxy = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let db_path = seed(
        dir.path(),
        "<img src='http://preload.invalid/cover.png'/><img src='file:///private/image.png'/>",
    );
    let conn = db::open_at(&db_path).unwrap();
    network::save(
        &conn,
        &ProxyConfig {
            content: ProxyRule::custom(&proxy.uri()),
            ..Default::default()
        },
    )
    .unwrap();
    Mock::given(method("GET"))
        .and(path("/cover.png"))
        .and(|request: &wiremock::Request| {
            request
                .headers
                .get("user-agent")
                .is_some_and(|value| value == crate::image_fetch::IMAGE_UA)
        })
        .respond_with(ResponseTemplate::new(200).set_body_bytes(PNG))
        .expect(1)
        .mount(&proxy)
        .await;
    let clients = NetworkClients::new(&db_path);
    crate::image_preload::run(&db_path, dir.path(), &clients).await;
    assert!(
        rrss_core::media::cached_path(&conn, 1, "http://preload.invalid/cover.png")
            .unwrap()
            .is_some()
    );
    crate::image_preload::run(&db_path, dir.path(), &clients).await;
    assert_eq!(proxy.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn proxy_auth_bodies_are_not_stored_as_fulltext_or_media() {
    let proxy = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(407).set_body_string("proxy-user:proxy-password ".repeat(20)),
        )
        .expect(3)
        .mount(&proxy)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let db_path = seed(
        dir.path(),
        "<video src='http://content.invalid/movie.mp4'></video>",
    );
    let conn = db::open_at(&db_path).unwrap();
    network::save(
        &conn,
        &ProxyConfig {
            content: ProxyRule::custom(&proxy.uri().replacen(
                "http://",
                "http://proxy-user:proxy-password@",
                1,
            )),
            ..Default::default()
        },
    )
    .unwrap();
    let clients = NetworkClients::new(&db_path);
    let fulltext = crate::fulltext::fetch_and_store(&clients, &db_path, 1)
        .await
        .unwrap_err();
    let client = clients.client(NetworkScope::Content).await.unwrap();
    let image =
        crate::image_fetch::fetch_image_bytes(&client, "http://content.invalid/image.png", "")
            .await
            .unwrap_err();
    let media = crate::embed_fetch::download_direct(
        &clients,
        &db_path,
        dir.path(),
        1,
        "http://content.invalid/movie.mp4",
        "",
    )
    .await
    .unwrap_err();
    let text = format!("{fulltext:?} {image:?} {media:?}");
    assert!(!text.contains("proxy-user"));
    assert!(!text.contains("proxy-password"));
    assert!(text.contains("407"));
    let article = rrss_core::articles::get_article(&conn, 1).unwrap();
    assert!(article.fulltext_md.is_none());
    let count: i64 = conn
        .query_row("SELECT count(*) FROM media", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn extracted_lazy_images_use_local_refs_and_failed_downloads_never_hotlink() {
    let proxy = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let db_path = seed(dir.path(), "<p>Feed teaser without images</p>");
    let conn = db::open_at(&db_path).unwrap();
    network::save(
        &conn,
        &ProxyConfig {
            content: ProxyRule::custom(proxy.uri()),
            ..Default::default()
        },
    )
    .unwrap();
    let page = include_str!("../../../../testdata/web/english.html")
        .replace("</head>", "<base href='/pictures/'></head>")
        .replace(
            "</article>",
            "<img data-src='good.png' alt='Good'><img data-original='bad.png' alt='Bad'></article>",
        );
    Mock::given(path("/article"))
        .respond_with(ResponseTemplate::new(200).set_body_string(page))
        .expect(1)
        .mount(&proxy)
        .await;
    Mock::given(path("/pictures/good.png"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(PNG))
        .expect(1)
        .mount(&proxy)
        .await;
    Mock::given(path("/pictures/bad.png"))
        .respond_with(ResponseTemplate::new(500))
        .expect(1)
        .mount(&proxy)
        .await;
    let network = NetworkClients::new(&db_path);
    crate::fulltext::fetch_and_store(&network, &db_path, 1)
        .await
        .unwrap();
    let before = crate::queries::article_media(&conn, 1).unwrap();
    assert_eq!(before.len(), 2);
    assert!(before.iter().all(|item| item.href.is_empty()));
    let refs = crate::jobs::media::prefetch_images(&db_path, dir.path(), &network, 1, |_| {})
        .await
        .unwrap();
    assert_eq!(refs.len(), 2);
    assert_eq!(refs[0].src, "http://content.invalid/pictures/good.png");
    assert!(!refs[0].href.is_empty());
    assert!(
        refs[0].href.starts_with(&crate::queries::media_href(
            rrss_core::media::lookup(&conn, 1, &refs[0].src)
                .unwrap()
                .unwrap()
                .id
        ))
    );
    assert_eq!(refs[1].src, "http://content.invalid/pictures/bad.png");
    assert!(
        refs[1].href.is_empty(),
        "failed image must leave a placeholder, not a remote href"
    );
}
