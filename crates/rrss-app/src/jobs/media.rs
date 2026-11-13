//! 阅读媒体预取编排。宿主只通过类型化回调转发既有事件。
use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use rrss_core::{articles, db, media};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use crate::views::{EmbedCoverEvent, MediaReadyEvent, MediaRef};
use crate::{document, embed_fetch, image_fetch, queries};

pub async fn prefetch_images(
    db_path: &Path,
    data_dir: &Path,
    network: &crate::network::NetworkClients,
    article_id: i64,
    emit: impl Fn(MediaReadyEvent) + Send + Sync + 'static,
) -> Result<Vec<MediaRef>> {
    let conn = db::open_at(db_path)?;
    let mut pending = Vec::new();
    for (url, referer) in queries::pending_images(&conn, article_id) {
        if media::cached_path(&conn, article_id, &url)
            .ok()
            .flatten()
            .is_some()
        {
            if let Some(row) = media::lookup(&conn, article_id, &url)? {
                emit(MediaReadyEvent {
                    article_id,
                    media: queries::cached_media_ref(&row, &url),
                });
            }
        } else {
            pending.push((url, referer));
        }
    }
    drop(conn);
    if pending.is_empty() {
        return article_media(db_path, data_dir, article_id);
    }
    let client = network
        .client(rrss_core::network::NetworkScope::Content)
        .await?;
    let emit = Arc::new(emit);
    let started = std::time::Instant::now();
    let count = pending.len();
    let limit = Arc::new(Semaphore::new(4));
    let mut set = JoinSet::new();
    for (url, referer) in pending {
        let client = client.clone();
        let db_path = db_path.to_path_buf();
        let data_dir = data_dir.to_path_buf();
        let limit = limit.clone();
        let emit = emit.clone();
        set.spawn(async move {
            let Ok(_permit) = limit.acquire().await else {
                return false;
            };
            let fetched = image_fetch::fetch_and_store(
                &client, &db_path, &data_dir, article_id, &url, &referer,
            )
            .await;
            let media = match fetched {
                Ok(_) => {
                    let Ok(conn) = db::open_at(&db_path) else {
                        return false;
                    };
                    let Ok(Some(row)) = media::lookup(&conn, article_id, &url) else {
                        return false;
                    };
                    queries::cached_media_ref(&row, &url)
                }
                Err(fail) => queries::failed_media_ref(&url, fail.label()),
            };
            let succeeded = !media.href.is_empty();
            emit(MediaReadyEvent { article_id, media });
            succeeded
        });
    }
    log_prefetch(&mut set, article_id, "images", count, started).await;
    article_media(db_path, data_dir, article_id)
}

/// 文章配图的当前状态；没缓存下来的带上本次运行里记下的失败原因。
fn article_media(db_path: &Path, data_dir: &Path, article_id: i64) -> Result<Vec<MediaRef>> {
    let conn = db::open_at(db_path)?;
    let mut refs = queries::article_media(&conn, article_id)?;
    for item in refs.iter_mut().filter(|item| item.href.is_empty()) {
        item.error = image_fetch::failure_label(data_dir, &item.src);
    }
    Ok(refs)
}

pub async fn retry_image(
    db_path: &Path,
    data_dir: &Path,
    network: &crate::network::NetworkClients,
    article_id: i64,
    url: &str,
) -> Result<MediaRef> {
    image_fetch::clear_failures(data_dir, url);
    let client = network
        .client(rrss_core::network::NetworkScope::Content)
        .await?;
    let referer = article_referer(db_path, article_id);
    match image_fetch::fetch_and_store(&client, db_path, data_dir, article_id, url, &referer).await
    {
        Ok(_) => {
            let conn = db::open_at(db_path)?;
            let row = media::lookup(&conn, article_id, url)?
                .ok_or_else(|| anyhow::anyhow!("stored image has no media row"))?;
            Ok(queries::cached_media_ref(&row, url))
        }
        Err(fail) => Ok(queries::failed_media_ref(url, fail.label())),
    }
}

fn article_referer(db_path: &Path, article_id: i64) -> String {
    db::open_at(db_path)
        .ok()
        .and_then(|conn| articles::get_article(&conn, article_id).ok())
        .and_then(|article| article.canonical_url)
        .unwrap_or_default()
}

pub async fn prefetch_embed_covers(
    db_path: &Path,
    data_dir: &Path,
    network: &crate::network::NetworkClients,
    article_id: i64,
    emit: impl Fn(EmbedCoverEvent) + Send + Sync + 'static,
) -> Result<()> {
    let conn = db::open_at(db_path)?;
    let article = articles::get_article(&conn, article_id)?;
    let mut doc = document::load_document(&article);
    embed_fetch::present_document(&mut doc);
    let targets = embed_fetch::official_open_urls(&doc);
    let mut pending = Vec::new();
    let cached = embed_fetch::cached_covers(&conn, article_id, &doc);
    for open_url in targets {
        if let Some(cover) = cached.iter().find(|cover| cover.open_url == open_url) {
            emit(EmbedCoverEvent {
                article_id,
                cover: cover.clone(),
            });
        } else {
            pending.push(open_url);
        }
    }
    drop(conn);
    if pending.is_empty() {
        return Ok(());
    }
    let client = network
        .client(rrss_core::network::NetworkScope::Content)
        .await?;
    let emit = Arc::new(emit);
    let started = std::time::Instant::now();
    let count = pending.len();
    let limit = Arc::new(Semaphore::new(4));
    let mut set = JoinSet::new();
    for open_url in pending {
        let client = client.clone();
        let db_path = db_path.to_path_buf();
        let data_dir = data_dir.to_path_buf();
        let limit = limit.clone();
        let emit = emit.clone();
        set.spawn(async move {
            let Ok(_permit) = limit.acquire().await else {
                return false;
            };
            let cover = match embed_fetch::fetch_official_cover(
                &client, &db_path, &data_dir, article_id, &open_url,
            )
            .await
            {
                Ok(cover) => cover,
                Err(_) => {
                    let site = match rrss_render::classify_for_read(
                        &open_url,
                        rrss_render::EmbedKind::Iframe,
                    ) {
                        rrss_render::EmbedClass::Bilibili { .. } => "bilibili",
                        _ => "youtube",
                    };
                    embed_fetch::failed_cover(&open_url, site)
                }
            };
            let succeeded = !cover.failed;
            emit(EmbedCoverEvent { article_id, cover });
            succeeded
        });
    }
    log_prefetch(&mut set, article_id, "covers", count, started).await;
    Ok(())
}

async fn log_prefetch(
    set: &mut JoinSet<bool>,
    article_id: i64,
    kind: &'static str,
    count: usize,
    started: std::time::Instant,
) {
    let mut succeeded = 0;
    while let Some(result) = set.join_next().await {
        match result {
            Ok(true) => succeeded += 1,
            Ok(false) => {}
            Err(error) => {
                tracing::error!(article_id, error = %crate::log::error(&error), "media prefetch worker failed")
            }
        }
    }
    tracing::info!(
        article_id,
        kind,
        count,
        succeeded,
        failed = count.saturating_sub(succeeded),
        elapsed_ms = started.elapsed().as_millis() as u64,
        "media prefetch finished"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn seed(dir: &Path, html: &str) -> std::path::PathBuf {
        let db_path = dir.join("test.db");
        let conn = db::open_at(&db_path).unwrap();
        conn.execute(
            "INSERT INTO feeds (id, url, created_at) VALUES (1, 'https://f.test/rss', 1)",
            [],
        )
        .unwrap();
        conn.execute("INSERT INTO articles (id, feed_id, title, hash, created_at, canonical_url, content_html) VALUES (1, 1, 'title', 'h', 1, 'https://f.test/article', ?1)", [html]).unwrap();
        db_path
    }

    #[tokio::test]
    async fn images_emit_only_successes_and_return_all_refs_in_document_order() {
        use tracing::instrument::WithSubscriber;
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/good.png"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Content-Type", "image/png")
                    .set_body_bytes(
                        include_bytes!("../../../../testdata/render/cover.png").to_vec(),
                    ),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/bad.png"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let good = format!("{}/good.png", server.uri());
        let bad = format!("{}/bad.png", server.uri());
        let db_path = seed(
            dir.path(),
            &format!("<img src='{good}'/><img src='{bad}'/>"),
        );
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink = events.clone();
        let (subscriber, handle) = crate::log::subscriber(
            crate::log::Mode::Desktop {
                log_dir: dir.path().join("logs"),
            },
            "",
        );
        let refs = prefetch_images(
            &db_path,
            dir.path(),
            &crate::network::NetworkClients::new(&db_path),
            1,
            move |event| sink.lock().unwrap().push(event),
        )
        .with_subscriber(subscriber)
        .await
        .unwrap();
        handle.end();
        let log = std::fs::read_to_string(dir.path().join("logs/rrss.log")).unwrap();
        assert_eq!(log.matches("media prefetch finished").count(), 1, "{log}");
        assert!(log.contains("count=2 succeeded=1 failed=1"), "{log}");
        assert!(
            !log.contains("good.png") && !log.contains("bad.png"),
            "{log}"
        );
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].src, good);
        assert!(!refs[0].href.is_empty());
        assert!(refs[0].href.contains("?v="));
        assert!(refs[0].error.is_empty());
        assert_eq!(refs[1].src, bad);
        assert!(refs[1].href.is_empty());
        assert_eq!(refs[1].error, "HTTP 5xx");
        {
            let events = events.lock().unwrap();
            assert_eq!(events.len(), 2);
            assert!(events.iter().all(|event| event.article_id == 1));
            let good_event = events.iter().find(|event| event.media.src == good).unwrap();
            let bad_event = events.iter().find(|event| event.media.src == bad).unwrap();
            assert_eq!(
                serde_json::to_value(&good_event.media).unwrap(),
                serde_json::to_value(&refs[0]).unwrap()
            );
            assert_eq!(bad_event.media.error, "HTTP 5xx");
            assert!(bad_event.media.href.is_empty());
        }
        assert!(
            server.received_requests().await.unwrap().iter().all(|r| r
                .headers
                .get("referer")
                .unwrap()
                == "https://f.test/article")
        );
    }

    #[tokio::test]
    async fn retry_clears_decode_failure_then_stores_the_image() {
        let server = MockServer::start().await;
        let png = include_bytes!("../../../../testdata/render/cover.png").to_vec();
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(b"<html>".to_vec(), "text/html"))
            .with_priority(1)
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Content-Type", "image/png")
                    .set_body_bytes(png),
            )
            .with_priority(5)
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let url = format!("{}/photo.avif", server.uri());
        let db_path = seed(dir.path(), &format!("<img src='{url}' alt='图'/>"));
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink = events.clone();
        let network = crate::network::NetworkClients::new(&db_path);
        let refs = prefetch_images(&db_path, dir.path(), &network, 1, move |event| {
            sink.lock().unwrap().push(event)
        })
        .await
        .unwrap();
        assert_eq!(refs.len(), 1);
        assert!(refs[0].href.is_empty());
        assert_eq!(refs[0].error, "解码失败");
        {
            let events = events.lock().unwrap();
            assert_eq!(events.len(), 1);
            assert_eq!(events[0].media.error, "解码失败");
            assert!(events[0].media.href.is_empty());
        }
        let conn = db::open_at(&db_path).unwrap();
        assert_eq!(
            conn.query_row("SELECT count(*) FROM media", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            0
        );
        drop(conn);
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
        let client = network
            .client(rrss_core::network::NetworkScope::Content)
            .await
            .unwrap();
        assert_eq!(
            image_fetch::fetch_and_store(&client, &db_path, dir.path(), 1, &url, "").await,
            Err(image_fetch::FetchFail::Decode)
        );
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
        let again = retry_image(&db_path, dir.path(), &network, 1, &url)
            .await
            .unwrap();
        assert!(again.error.is_empty());
        assert!(again.href.contains("?v="));
        assert_eq!(again.src, url);
        assert_eq!(server.received_requests().await.unwrap().len(), 2);
        let conn = db::open_at(&db_path).unwrap();
        let stored = queries::article_media(&conn, 1).unwrap();
        assert_eq!(stored[0].href, again.href);
        assert!(stored[0].error.is_empty());
        let mime: String = conn
            .query_row("SELECT COALESCE(mime, '') FROM media", [], |row| row.get(0))
            .unwrap();
        assert_eq!(mime, "image/png");
        assert!(image_fetch::failure_label(dir.path(), &url).is_empty());
    }

    #[tokio::test]
    async fn no_targets_make_no_requests_or_events_and_missing_articles_still_fail() {
        let server = MockServer::start().await;
        let dir = tempfile::tempdir().unwrap();
        let db_path = seed(dir.path(), "<p>No media</p>");
        let client = crate::network::NetworkClients::new(&db_path);
        let conn = db::open_at(&db_path).unwrap();
        rrss_core::network::save(
            &conn,
            &rrss_core::network::ProxyConfig {
                content: rrss_core::network::ProxyRule {
                    mode: rrss_core::network::ProxyMode::Custom,
                    url: server.uri(),
                    no_proxy: String::new(),
                },
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            prefetch_images(&db_path, dir.path(), &client, 1, |_| panic!(
                "unexpected event"
            ))
            .await
            .unwrap()
            .is_empty()
        );
        prefetch_embed_covers(&db_path, dir.path(), &client, 1, |_| {
            panic!("unexpected event")
        })
        .await
        .unwrap();
        assert!(
            prefetch_images(&db_path, dir.path(), &client, 99, |_| {})
                .await
                .is_err()
        );
        assert!(
            prefetch_embed_covers(&db_path, dir.path(), &client, 99, |_| {})
                .await
                .is_err()
        );
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn covers_emit_cached_success_and_failed_site_fallback_without_external_network() {
        let dir = tempfile::tempdir().unwrap();
        let youtube = "https://www.youtube.com/watch?v=abcdefghijk";
        let bilibili = "https://www.bilibili.com/video/BV1xx411c7mD";
        let db_path = seed(
            dir.path(),
            &format!(
                "<iframe src='{youtube}'></iframe><iframe src='{youtube}'></iframe><iframe src='{bilibili}'></iframe>"
            ),
        );
        let conn = db::open_at(&db_path).unwrap();
        let thumb = "https://thumb.test/cached.png";
        media::store_image(
            &conn,
            dir.path(),
            1,
            thumb,
            include_bytes!("../../../../testdata/render/cover.png"),
        )
        .unwrap();
        let meta = serde_json::json!({"site": "youtube", "title": "cached", "author": "author", "duration_secs": null, "open_url": youtube, "thumbnail_url": thumb});
        let meta_path = dir.path().join("cover-meta");
        std::fs::write(&meta_path, meta.to_string()).unwrap();
        media::store_sidecar(
            &conn,
            dir.path(),
            1,
            youtube,
            &meta_path,
            Some("application/json"),
            1,
        )
        .unwrap();
        // 所有非缓存的 HTTPS 请求只到本地 HTTP 代理，CONNECT 被立即拒绝。
        let server = MockServer::start().await;
        Mock::given(method("CONNECT"))
            .respond_with(ResponseTemplate::new(502))
            .mount(&server)
            .await;
        let client = crate::network::NetworkClients::new(&db_path);
        let conn = db::open_at(&db_path).unwrap();
        rrss_core::network::save(
            &conn,
            &rrss_core::network::ProxyConfig {
                content: rrss_core::network::ProxyRule {
                    mode: rrss_core::network::ProxyMode::Custom,
                    url: server.uri(),
                    no_proxy: String::new(),
                },
                ..Default::default()
            },
        )
        .unwrap();
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink = events.clone();
        prefetch_embed_covers(&db_path, dir.path(), &client, 1, move |event| {
            sink.lock().unwrap().push(event)
        })
        .await
        .unwrap();
        let events = events.lock().unwrap();
        assert_eq!(events.len(), 2, "official targets are deduplicated");
        assert!(events.iter().all(|e| e.article_id == 1));
        let cached = events.iter().find(|e| e.cover.site == "youtube").unwrap();
        assert!(!cached.cover.failed);
        assert_eq!(cached.cover.title, "cached");
        assert!(cached.cover.cover_href.contains("?v="));
        let failed = events.iter().find(|e| e.cover.site == "bilibili").unwrap();
        assert!(failed.cover.failed);
        assert_eq!(failed.cover.open_url, format!("{bilibili}/"));
        assert!(failed.cover.cover_href.is_empty());
    }
}
