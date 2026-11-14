//! 正文配图使用独立的浏览器形态 UA；代理与连接池由 Content 客户端提供。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use crate::scheduler::model_capacity::{Capacity, Priority};
use reqwest::header::{HeaderValue, REFERER, USER_AGENT};
use rrss_core::db;

pub const IMAGE_UA: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36";

pub async fn fetch_image_bytes(
    client: &reqwest::Client,
    url: &str,
    referer: &str,
) -> Result<(Vec<u8>, Option<String>), FetchFail> {
    let resp = send_image_get(client, url, referer).await?;
    let status = resp.status();
    if status.is_success() {
        return read_image_body(resp).await;
    }
    let code = status.as_u16();
    if !referer.is_empty() && (code == 401 || code == 403) {
        tracing::debug!(host = %rrss_core::logsafe::host_str(url), status = code, "retry image without referer");
        let retry = send_image_get(client, url, "").await?;
        if retry.status().is_success() {
            return read_image_body(retry).await;
        }
        return Err(FetchFail::Http(retry.status().as_u16()));
    }
    Err(FetchFail::Http(code))
}

async fn send_image_get(
    client: &reqwest::Client,
    url: &str,
    referer: &str,
) -> Result<reqwest::Response, FetchFail> {
    let mut builder = client
        .get(url)
        .header(USER_AGENT, IMAGE_UA)
        .timeout(std::time::Duration::from_secs(15));
    if !referer.is_empty()
        && let Ok(value) = HeaderValue::from_str(referer)
    {
        builder = builder.header(REFERER, value);
    }
    builder
        .send()
        .await
        .map_err(|err| FetchFail::Network(rrss_core::network::request_error(&err).to_string()))
}

async fn read_image_body(
    mut resp: reqwest::Response,
) -> Result<(Vec<u8>, Option<String>), FetchFail> {
    if resp
        .content_length()
        .is_some_and(|n| n > rrss_core::media::MAX_IMAGE_BYTES as u64)
    {
        return Err(FetchFail::TooLarge);
    }
    let mime = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.split(';').next().unwrap_or(s).trim().to_string());
    let mut bytes = Vec::new();
    loop {
        let chunk = resp.chunk().await.map_err(|err| {
            FetchFail::Network(rrss_core::network::request_error(&err).to_string())
        })?;
        let Some(chunk) = chunk else { break };
        if bytes.len() + chunk.len() > rrss_core::media::MAX_IMAGE_BYTES {
            return Err(FetchFail::TooLarge);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok((bytes, mime))
}

pub async fn fetch_and_store(
    client: &reqwest::Client,
    db_path: &Path,
    data_dir: &Path,
    article_id: i64,
    url: &str,
    referer: &str,
) -> Result<PathBuf, FetchFail> {
    fetch_and_store_with_priority(
        client,
        db_path,
        data_dir,
        article_id,
        url,
        referer,
        Priority::Interactive,
    )
    .await
}

pub(crate) async fn fetch_and_store_with_priority(
    client: &reqwest::Client,
    db_path: &Path,
    data_dir: &Path,
    article_id: i64,
    url: &str,
    referer: &str,
    priority: Priority,
) -> Result<PathBuf, FetchFail> {
    if let Some(path) = cached_on_disk(db_path, article_id, url) {
        clear_failures(data_dir, url);
        return Ok(path);
    }
    if let Some(fail) = remembered(data_dir, url) {
        return Err(fail);
    }
    if !claim_url(url) {
        for _ in 0..80 {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            if let Some(fail) = remembered(data_dir, url) {
                return Err(fail);
            }
            if let Some(path) = cached_on_disk(db_path, article_id, url) {
                clear_failures(data_dir, url);
                return Ok(path);
            }
        }
        if let Some(path) = cached_on_disk(db_path, article_id, url) {
            clear_failures(data_dir, url);
            return Ok(path);
        }
        if !claim_url(url) {
            let fail = FetchFail::Network("in-flight timeout".into());
            remember(data_dir, url, &fail);
            return Err(fail);
        }
    }
    struct Claim<'a>(&'a str);
    impl Drop for Claim<'_> {
        fn drop(&mut self) {
            release_url(self.0);
        }
    }
    let _claim = Claim(url);
    let result = fetch_and_store_inner(
        client, db_path, data_dir, article_id, url, referer, priority,
    )
    .await;
    match &result {
        Ok(_) => clear_failures(data_dir, url),
        Err(fail @ (FetchFail::Decode | FetchFail::TooLarge)) => {
            if let Ok(mut failures) = permanent_failures().lock() {
                failures.insert(failure_key(data_dir, url), fail.clone());
            }
            remember(data_dir, url, fail);
        }
        Err(fail) => remember(data_dir, url, fail),
    }
    result
}

// Bad content and oversized responses are terminal for this cache during this run.
// Keep failures out of media rows so an existing successful entry is never damaged.
// The latest label stays in this process, keyed by data dir and url, for the reader.
type FailureKey = (PathBuf, String);
type ImageFailures = HashMap<FailureKey, FetchFail>;

fn failure_key(data_dir: &Path, url: &str) -> FailureKey {
    (data_dir.to_path_buf(), url.to_string())
}

fn permanent_failures() -> &'static Mutex<ImageFailures> {
    static FAILURES: OnceLock<Mutex<ImageFailures>> = OnceLock::new();
    FAILURES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn recent_labels() -> &'static Mutex<HashMap<FailureKey, String>> {
    static LABELS: OnceLock<Mutex<HashMap<FailureKey, String>>> = OnceLock::new();
    LABELS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn failure_label(data_dir: &Path, url: &str) -> String {
    recent_labels()
        .lock()
        .ok()
        .and_then(|labels| labels.get(&failure_key(data_dir, url)).cloned())
        .unwrap_or_default()
}

pub fn clear_failures(data_dir: &Path, url: &str) {
    let key = failure_key(data_dir, url);
    if let Ok(mut failures) = permanent_failures().lock() {
        failures.remove(&key);
    }
    if let Ok(mut labels) = recent_labels().lock() {
        labels.remove(&key);
    }
}

fn remember(data_dir: &Path, url: &str, fail: &FetchFail) {
    if let Ok(mut labels) = recent_labels().lock() {
        labels.insert(failure_key(data_dir, url), fail.label().to_string());
    }
}

fn remembered(data_dir: &Path, url: &str) -> Option<FetchFail> {
    let fail = permanent_failures()
        .lock()
        .ok()?
        .get(&failure_key(data_dir, url))
        .cloned()?;
    remember(data_dir, url, &fail);
    Some(fail)
}

fn cached_on_disk(db_path: &Path, article_id: i64, url: &str) -> Option<PathBuf> {
    let conn = db::open_at(db_path).ok()?;
    rrss_core::media::cached_path(&conn, article_id, url)
        .ok()
        .flatten()
}

fn in_flight() -> &'static Mutex<HashSet<String>> {
    static SET: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    SET.get_or_init(|| Mutex::new(HashSet::new()))
}

fn claim_url(url: &str) -> bool {
    in_flight()
        .lock()
        .map(|mut set| set.insert(url.to_string()))
        .unwrap_or(true)
}

fn release_url(url: &str) {
    if let Ok(mut set) = in_flight().lock() {
        set.remove(url);
    }
}

async fn fetch_and_store_inner(
    client: &reqwest::Client,
    db_path: &Path,
    data_dir: &Path,
    article_id: i64,
    url: &str,
    referer: &str,
    priority: Priority,
) -> Result<PathBuf, FetchFail> {
    let capacity = Capacity::for_database(db_path);
    let permit = capacity
        .content
        .clone()
        .acquire(priority)
        .await
        .map_err(|_| FetchFail::Network("content capacity busy".into()))?;
    let bytes = fetch_image_bytes(client, url, referer).await;
    drop(permit);
    match bytes {
        Ok((bytes, _)) => {
            let stored = db::open_at(db_path).and_then(|conn| {
                rrss_core::media::store_image(&conn, data_dir, article_id, url, &bytes)
            });
            match stored {
                Ok(path) => Ok(path),
                Err(rrss_core::Error::UnsupportedImage) => Err(FetchFail::Decode),
                Err(error) => {
                    tracing::debug!(host = %rrss_core::logsafe::host_str(url), error = %rrss_core::logsafe::error(&error), "store image failed");
                    Err(FetchFail::Store)
                }
            }
        }
        Err(fail) => {
            tracing::debug!(host = %rrss_core::logsafe::host_str(url), error = %fail, "fetch image");
            Err(fail)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchFail {
    Http(u16),
    TooLarge,
    Network(String),
    Decode,
    Store,
}

impl FetchFail {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Http(403) | Self::Http(401) => "image_http_403",
            Self::Http(404) => "image_http_404",
            Self::Http(status) if *status >= 500 => "image_http_5xx",
            Self::Http(_) => "image_download_failed",
            Self::TooLarge => "image_too_large",
            Self::Network(_) => "image_network_failed",
            Self::Decode => "image_decode_failed",
            Self::Store => "image_store_failed",
        }
    }
}

impl std::fmt::Display for FetchFail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Http(code) => write!(f, "http {code}"),
            Self::TooLarge => write!(f, "too large"),
            Self::Network(msg) => write!(f, "{msg}"),
            Self::Decode => write!(f, "decode"),
            Self::Store => write!(f, "store"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{FetchFail, fetch_image_bytes};
    use wiremock::matchers::{header_exists, method};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn unrecognized_content_is_a_terminal_decode_failure() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(b"<html>".to_vec(), "image/png"))
            .expect(1)
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let conn = rrss_core::db::open_at(&db_path).unwrap();
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        for _ in 0..2 {
            assert_eq!(
                super::fetch_and_store(&client, &db_path, dir.path(), 1, &server.uri(), "").await,
                Err(FetchFail::Decode)
            );
        }
        assert_eq!(
            conn.query_row("SELECT count(*) FROM media", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert!(!rrss_core::media::cache_dir(dir.path()).exists());
    }

    #[tokio::test]
    async fn oversized_chunked_image_stops_before_response_finishes() {
        use tokio::io::AsyncWriteExt;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/image", listener.local_addr().unwrap());
        let (release, wait) = tokio::sync::oneshot::channel::<()>();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            crate::test_net::read_request_head(&mut stream).await;
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n")
                .await
                .unwrap();
            let chunk = vec![b'x'; 64 * 1024];
            for _ in 0..=rrss_core::media::MAX_IMAGE_BYTES / chunk.len() {
                if stream.write_all(b"10000\r\n").await.is_err()
                    || stream.write_all(&chunk).await.is_err()
                    || stream.write_all(b"\r\n").await.is_err()
                {
                    return;
                }
            }
            // Keep the response unfinished: a whole-body read would wait here.
            let _ = wait.await;
        });
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            fetch_image_bytes(&client, &url, ""),
        )
        .await;
        let _ = release.send(());
        server.await.unwrap();
        assert_eq!(
            result.expect("must stop before EOF"),
            Err(FetchFail::TooLarge)
        );
    }

    #[test]
    fn labels_cover_common_failures() {
        assert_eq!(FetchFail::Http(403).label(), "image_http_403");
        assert_eq!(FetchFail::Http(404).label(), "image_http_404");
        assert_eq!(FetchFail::Http(502).label(), "image_http_5xx");
        assert_eq!(FetchFail::TooLarge.label(), "image_too_large");
        assert_eq!(FetchFail::Decode.label(), "image_decode_failed");
    }

    #[tokio::test]
    async fn rejected_referer_retries_on_same_proxy_with_image_ua() {
        use super::IMAGE_UA;
        use rrss_core::network::{NetworkScope, ProxyConfig, ProxyRule};
        let proxy = MockServer::start().await;
        Mock::given(method("GET"))
            .and(header_exists("referer"))
            .respond_with(ResponseTemplate::new(403))
            .with_priority(1)
            .expect(1)
            .mount(&proxy)
            .await;
        Mock::given(method("GET"))
            .and(|request: &wiremock::Request| {
                request
                    .headers
                    .get("user-agent")
                    .is_some_and(|value| value == IMAGE_UA)
            })
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"image"))
            .with_priority(2)
            .expect(1)
            .mount(&proxy)
            .await;
        let client = ProxyConfig {
            content: ProxyRule::custom(proxy.uri()),
            ..Default::default()
        }
        .build_client(NetworkScope::Content, reqwest::Client::builder())
        .unwrap();
        let result = fetch_image_bytes(
            &client,
            "http://image.invalid/p.png",
            "http://image.invalid/article",
        )
        .await;
        let requests = proxy.received_requests().await.unwrap();
        let (bytes, _) = result.unwrap_or_else(|error| panic!("{error:?}; requests: {requests:?}"));
        assert_eq!(bytes, b"image");
        assert_eq!(requests.len(), 2);
        assert!(!requests[1].headers.contains_key("referer"));
    }

    #[tokio::test]
    async fn sends_referer_then_reads_body() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(403))
            .with_priority(5)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(header_exists("referer"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "image/png")
                    .set_body_raw(
                        [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A],
                        "image/png",
                    ),
            )
            .with_priority(1)
            .mount(&server)
            .await;

        let (bytes, mime) = fetch_image_bytes(
            &reqwest::Client::builder().no_proxy().build().unwrap(),
            &server.uri(),
            "https://www.v2ex.com/t/1237147",
        )
        .await
        .expect("image with referer");
        assert_eq!(mime.as_deref(), Some("image/png"));
        assert!(!bytes.is_empty());
    }
}
