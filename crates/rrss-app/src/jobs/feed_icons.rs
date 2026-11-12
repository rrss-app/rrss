//! 刷新后下载到期的网站图标；检查时间在联网前认领，失败也冷却七天。
use super::spawn_db;
use anyhow::{Result, bail};
use reqwest::Url;
use rrss_core::feed_icon;
use rrss_core::network::Redacted;
use std::{path::Path, time::Duration};

pub async fn run(db_path: &Path, client: &reqwest::Client) {
    let result = run_inner(db_path, client).await;
    if let Err(err) = result {
        tracing::warn!(error = %err, "feed icon job failed");
    }
}

async fn run_inner(db_path: &Path, client: &reqwest::Client) -> Result<()> {
    let rows = spawn_db(db_path.to_path_buf(), |conn| {
        if !feed_icon::enabled(conn)? { return Ok(Vec::new()); }
        let mut stmt = conn.prepare("SELECT id, COALESCE(NULLIF(site_url, ''), url), NULLIF(site_url, '') IS NOT NULL FROM feeds WHERE icon_checked_at IS NULL OR icon_checked_at <= ?1")?;
        let rows = stmt.query_map([rrss_core::feeds::now_ms() - feed_icon::RETRY_MS], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, bool>(2)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }).await?;
    for (id, home, has_site) in rows {
        let claimed = spawn_db(db_path.to_path_buf(), move |conn| {
            let now = rrss_core::feeds::now_ms();
            if !feed_icon::enabled(conn)? { return Ok(false); }
            Ok(conn.execute("UPDATE feeds SET icon_checked_at=?1 WHERE id=?2 AND (icon_checked_at IS NULL OR icon_checked_at<=?3)", rusqlite::params![now, id, now-feed_icon::RETRY_MS])? != 0)
        }).await?;
        if !claimed {
            continue;
        }
        let home = if has_site {
            home
        } else {
            Url::parse(&home)
                .and_then(|url| url.join("/"))
                .map(|url| url.to_string())
                .unwrap_or(home)
        };
        if let Err(err) = fetch(db_path, client, id, &home).await {
            tracing::debug!(feed_id = id, error = %err, "feed icon unavailable");
        }
    }
    Ok(())
}

async fn read(mut response: reqwest::Response, max: usize) -> Result<Vec<u8>> {
    response.error_for_status_ref().redacted()?;
    if response
        .content_length()
        .is_some_and(|len| len > max as u64)
    {
        bail!("icon response too large");
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.redacted()? {
        if bytes.len() + chunk.len() > max {
            bail!("icon response too large");
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

async fn fetch(db_path: &Path, client: &reqwest::Client, id: i64, home: &str) -> Result<()> {
    use crate::scheduler::model_capacity::{Capacity, Priority};
    let capacity = Capacity::for_database(db_path);
    let home = Url::parse(home)?;
    if !rrss_core::media::allowed_url(home.as_str()) {
        bail!("invalid homepage URL");
    }
    let permit = capacity
        .content
        .clone()
        .acquire(Priority::Background)
        .await?;
    let response = client
        .get(home.clone())
        .timeout(Duration::from_secs(15))
        .send()
        .await;
    let (base, html) = match response {
        Ok(response) => {
            let base = response.url().clone();
            (
                base,
                read(response, 1024 * 1024)
                    .await
                    .ok()
                    .map(|b| String::from_utf8_lossy(&b).into_owned())
                    .unwrap_or_default(),
            )
        }
        Err(_) => (home, String::new()),
    };
    drop(permit);
    let url = feed_icon::select_url(&base, &html).ok_or_else(|| anyhow::anyhow!("no icon URL"))?;
    let permit = capacity
        .content
        .clone()
        .acquire(Priority::Background)
        .await?;
    let response = client
        .get(url.clone())
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .redacted()?;
    let mime = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let bytes = read(response, feed_icon::MAX_BYTES).await?;
    drop(permit);
    let data_dir = db_path.parent().unwrap_or(Path::new(".")).to_path_buf();
    spawn_db(db_path.to_path_buf(), move |conn| {
        feed_icon::store(conn, &data_dir, id, url.as_str(), &bytes, mime.as_deref())
    })
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rrss_core::db;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path},
    };
    #[tokio::test]
    async fn caches_local_fixture_throttles_failure_and_obeys_switches() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(include_str!("../../../../testdata/feeds/icons.html")),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/icon64.png"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                include_bytes!("../../../../testdata/render/cover.png").to_vec(),
                "image/png",
            ))
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let conn = db::open_at(&db_path).unwrap();
        let id = rrss_core::feeds::subscribe(
            &conn,
            rrss_core::feeds::SubscribeRequest {
                url: format!("{}/rss", server.uri()),
                folder_id: None,
                as_rsshub: false,
                rsshub_instance_id: None,
            },
        )
        .unwrap();
        conn.execute(
            "UPDATE feeds SET site_url=?1 WHERE id=?2",
            rusqlite::params![format!("{}/", server.uri()), id],
        )
        .unwrap();
        db::set_setting(&conn, feed_icon::SETTING_KEY, "0").unwrap();
        run_inner(&db_path, &reqwest::Client::new()).await.unwrap();
        assert!(server.received_requests().await.unwrap().is_empty());
        db::set_setting(&conn, feed_icon::SETTING_KEY, "1").unwrap();
        // 「自动下载对话引擎」与网站图标无关，关掉它不影响图标。
        rrss_core::pi_store::set_pi_auto_download(&conn, false).unwrap();
        run_inner(&db_path, &reqwest::Client::new()).await.unwrap();
        run_inner(&db_path, &reqwest::Client::new()).await.unwrap();
        assert_eq!(server.received_requests().await.unwrap().len(), 2);
        assert_eq!(
            rrss_core::storage::media_usage(&conn, dir.path())
                .unwrap()
                .image_bytes,
            include_bytes!("../../../../testdata/render/cover.png").len() as i64
        );
        let cached: String = conn
            .query_row("SELECT local_path FROM media WHERE feed_id=?1", [id], |r| {
                r.get(0)
            })
            .unwrap();
        let tree = crate::queries::feed_tree(&conn).unwrap();
        let href = crate::queries::media_href(
            conn.query_row("SELECT id FROM media WHERE feed_id=?1", [id], |r| r.get(0))
                .unwrap(),
        );
        assert_eq!(tree.uncategorized[0].icon_href, href);
        // 阅读器与搜索结果用同一份本地图标，不只焦点卡片和订阅列表。
        let entry = rrss_core::parse::ParsedEntry {
            guid: "icon".into(),
            canonical_url: Some("https://example.test/icon".into()),
            title: "图标同步".into(),
            author: None,
            published_at: Some(1),
            content_html: Some("<p>正文</p>".into()),
            hash: "icon".into(),
            tags: vec![],
        };
        rrss_core::ingest::upsert_entries(&conn, id, &[entry], 1).unwrap();
        let article: i64 = conn
            .query_row("SELECT id FROM articles WHERE feed_id=?1", [id], |r| {
                r.get(0)
            })
            .unwrap();
        let search = crate::views::SearchQuery {
            query: "图标同步".into(),
            ..Default::default()
        };
        assert_eq!(
            crate::queries::reader_view(&conn, article)
                .unwrap()
                .feed_icon_href,
            href
        );
        assert_eq!(
            crate::queries::search_hits(&conn, &search).unwrap()[0].feed_icon_href,
            href
        );
        db::set_setting(&conn, feed_icon::SETTING_KEY, "0").unwrap();
        assert!(
            crate::queries::feed_tree(&conn).unwrap().uncategorized[0]
                .icon_href
                .is_empty()
        );
        assert!(
            crate::queries::reader_view(&conn, article)
                .unwrap()
                .feed_icon_href
                .is_empty()
        );
        db::set_setting(&conn, feed_icon::SETTING_KEY, "1").unwrap();
        rrss_core::feeds::unsubscribe(&conn, id).unwrap();
        assert!(!Path::new(&cached).exists());
        let failure = rrss_core::feeds::subscribe(
            &conn,
            rrss_core::feeds::SubscribeRequest {
                url: format!("{}/missing", server.uri()),
                folder_id: None,
                as_rsshub: false,
                rsshub_instance_id: None,
            },
        )
        .unwrap();
        conn.execute(
            "UPDATE feeds SET site_url=?1 WHERE id=?2",
            rusqlite::params![format!("{}/missing", server.uri()), failure],
        )
        .unwrap();
        run_inner(&db_path, &reqwest::Client::new()).await.unwrap();
        let n = server.received_requests().await.unwrap().len();
        run_inner(&db_path, &reqwest::Client::new()).await.unwrap();
        assert_eq!(server.received_requests().await.unwrap().len(), n);
        assert!(
            conn.query_row(
                "SELECT icon_checked_at FROM feeds WHERE id=?1",
                [failure],
                |r| r.get::<_, Option<i64>>(0)
            )
            .unwrap()
            .is_some()
        );
    }
}
