//! RSS-only HTTP policy. The CLI/web-page fetcher and shared media clients keep their contracts.
use super::{FailureClass, FetchRequest, FetchResponse};
use crate::{Error, Result};
use reqwest::{Url, header};
use std::{future::Future, time::Duration};

pub const TOTAL_TIMEOUT: Duration = Duration::from_secs(30);
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
pub const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;
const MAX_REDIRECTS: usize = 10;
// Shared by the two fixed 407 producers and the IPC classifier during migration.
pub const PROXY_AUTH_MESSAGE: &str = "代理认证失败，请检查网络代理设置";

#[derive(Clone, Copy)]
pub struct FeedPolicy {
    pub timeout: Duration,
    pub max_body_bytes: usize,
}
impl Default for FeedPolicy {
    fn default() -> Self {
        Self {
            timeout: TOTAL_TIMEOUT,
            max_body_bytes: MAX_BODY_BYTES,
        }
    }
}

pub fn client_builder() -> reqwest::ClientBuilder {
    super::http_client_builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
}

fn error(message: &str) -> Error {
    Error::Http {
        status: 0,
        message: message.into(),
    }
}

/// Keep transport classification before redacting reqwest's diagnostic. HTTP
/// status/parse/policy failures do not participate in offline detection.
#[derive(Debug)]
pub struct FetchFailure {
    pub error: Error,
    pub class: FailureClass,
    pub network: bool,
    pub http_response: bool,
    pub timeout: bool,
    pub kind: crate::network::ErrorKind,
    response_status: Option<u16>,
}

impl From<Error> for FetchFailure {
    fn from(error: Error) -> Self {
        let response_status = match &error {
            Error::Http { status, .. } | Error::Network { status, .. } if *status > 0 => {
                Some(*status)
            }
            _ => None,
        };
        let kind = response_status.map_or(
            crate::network::ErrorKind::Other,
            crate::network::ErrorKind::http,
        );
        Self {
            error,
            class: FailureClass::Fast,
            network: false,
            http_response: response_status.is_some(),
            timeout: false,
            kind,
            response_status,
        }
    }
}

fn failure_class(elapsed: Duration, timeout: bool) -> FailureClass {
    if timeout || elapsed >= CONNECT_TIMEOUT {
        FailureClass::Slow
    } else {
        FailureClass::Fast
    }
}

impl FetchFailure {
    pub fn status(&self) -> Option<u16> {
        self.response_status
    }

    fn with_kind(mut self, kind: crate::network::ErrorKind) -> Self {
        self.kind = kind;
        self
    }

    fn transport(error: &reqwest::Error, elapsed: Duration) -> Self {
        Self {
            class: failure_class(elapsed, error.is_timeout()),
            network: !error.is_builder()
                && !error.is_status()
                && !error.is_redirect()
                && !error.is_decode(),
            error: crate::network::request_error(error),
            http_response: false,
            timeout: error.is_timeout(),
            kind: crate::network::error_kind(error),
            response_status: error.status().map(|status| status.as_u16()),
        }
    }
}

fn response_failure(error: Error) -> FetchFailure {
    FetchFailure {
        http_response: true,
        ..error.into()
    }
}

/// A permit covers one hop through the decompressed response body. Drop it before
/// acquiring the redirect target's permit; the absolute deadline covers all hops.
/// `body_read` reports decompressed bytes retained in the response, so callers can
/// apply dispatch backpressure while an unfinished download is still reading.
pub async fn fetch<P, F, Fut, B>(
    client: &reqwest::Client,
    request: FetchRequest,
    policy: FeedPolicy,
    acquire: F,
    body_read: B,
) -> Result<FetchResponse>
where
    F: FnMut(Url) -> Fut,
    Fut: Future<Output = Result<P>>,
    B: FnMut(usize),
{
    fetch_classified(client, request, policy, acquire, body_read)
        .await
        .map_err(|failure| failure.error)
}

pub async fn fetch_classified<P, F, Fut, B>(
    client: &reqwest::Client,
    request: FetchRequest,
    policy: FeedPolicy,
    acquire: F,
    body_read: B,
) -> std::result::Result<FetchResponse, FetchFailure>
where
    F: FnMut(Url) -> Fut,
    Fut: Future<Output = Result<P>>,
    B: FnMut(usize),
{
    fetch_controlled(client, request, policy, acquire, body_read, |_| async {
        Ok(())
    })
    .await
}

#[derive(Clone)]
pub struct ResponseHead {
    pub url: Url,
    pub visited: Vec<Url>,
    pub status: u16,
    pub retry_after: Option<String>,
}

/// The header callback runs while the final hop still owns its permit, before
/// body reads or release. The scheduler can persist a cooldown before allowing
/// any new same-host admission.
pub async fn fetch_controlled<P, F, Fut, B, H, HFut>(
    client: &reqwest::Client,
    request: FetchRequest,
    policy: FeedPolicy,
    mut acquire: F,
    mut body_read: B,
    mut headers: H,
) -> std::result::Result<FetchResponse, FetchFailure>
where
    F: FnMut(Url) -> Fut,
    Fut: Future<Output = Result<P>>,
    B: FnMut(usize),
    H: FnMut(ResponseHead) -> HFut,
    HFut: Future<Output = Result<()>>,
{
    let started = tokio::time::Instant::now();
    let mut transferring = false;
    let mut received_http = false;
    let mut response_status = None;
    let mut recording = false;
    let result = tokio::time::timeout(policy.timeout, async {
        let mut url = Url::parse(&request.url).map_err(|_| error("无效的订阅地址"))?;
        let mut visited = Vec::new();
        for hop in 0..=MAX_REDIRECTS {
            if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
                return Err(
                    FetchFailure::from(error("订阅重定向地址必须是 HTTP 或 HTTPS"))
                        .with_kind(crate::network::ErrorKind::Redirect),
                );
            }
            let permit = acquire(url.clone()).await?;
            visited.push(url.clone());
            transferring = true;
            response_status = None;
            let mut builder = client.get(url.clone());
            // Keep conditional requests across redirects, as the original reqwest path did.
            if let Some(value) = &request.etag {
                builder = builder.header(header::IF_NONE_MATCH, value);
            }
            if let Some(value) = &request.last_modified {
                builder = builder.header(header::IF_MODIFIED_SINCE, value);
            }
            let mut response = builder
                .send()
                .await
                .map_err(|e| FetchFailure::transport(&e, started.elapsed()))?;
            let status = response.status().as_u16();
            response_status = Some(status);
            received_http = true;
            if status == 407 {
                return Err(Error::Http {
                    status,
                    message: PROXY_AUTH_MESSAGE.into(),
                }
                .into());
            }
            if matches!(status, 301 | 302 | 303 | 307 | 308) {
                if hop == MAX_REDIRECTS {
                    return Err(response_failure(error("订阅重定向次数过多"))
                        .with_kind(crate::network::ErrorKind::Redirect));
                }
                let location = response
                    .headers()
                    .get(header::LOCATION)
                    .and_then(|v| v.to_str().ok())
                    .ok_or_else(|| {
                        response_failure(error("订阅重定向缺少有效地址"))
                            .with_kind(crate::network::ErrorKind::Redirect)
                    })?;
                let mut next = url.join(location).map_err(|_| {
                    response_failure(error("无效的订阅重定向地址"))
                        .with_kind(crate::network::ErrorKind::Redirect)
                })?;
                if next.origin() != url.origin() {
                    let _ = next.set_username("");
                    let _ = next.set_password(None);
                }
                let mut route = visited.clone();
                route.push(next.clone());
                transferring = false;
                recording = true;
                headers(ResponseHead {
                    url: url.clone(),
                    visited: route,
                    status,
                    retry_after: None,
                })
                .await
                .map_err(response_failure)?;
                recording = false;
                drop(response);
                drop(permit);
                transferring = false;
                url = next;
                continue;
            }
            let value = |name| {
                response
                    .headers()
                    .get(name)
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_owned)
            };
            let mut out = FetchResponse {
                status,
                etag: value(header::ETAG),
                last_modified: value(header::LAST_MODIFIED),
                retry_after: value(header::RETRY_AFTER),
                body: Vec::new(),
            };
            transferring = false;
            recording = true;
            headers(ResponseHead {
                url: url.clone(),
                visited: visited.clone(),
                status,
                retry_after: out.retry_after.clone(),
            })
            .await
            .map_err(response_failure)?;
            recording = false;
            transferring = true;
            if response.status().is_success() {
                while let Some(chunk) = response.chunk().await.map_err(|e| FetchFailure {
                    http_response: true,
                    ..FetchFailure::transport(&e, started.elapsed())
                })? {
                    if chunk.len() > policy.max_body_bytes.saturating_sub(out.body.len()) {
                        return Err(response_failure(error("订阅正文超过大小上限"))
                            .with_kind(crate::network::ErrorKind::Body));
                    }
                    out.body.extend_from_slice(&chunk);
                    body_read(chunk.len());
                }
            }
            return Ok(out);
        }
        unreachable!()
    })
    .await;
    match result {
        Ok(result) => result,
        Err(_) if recording => Err(response_failure(Error::FetchDeferred {
            until: None,
            message: "正在保存订阅限流记录，稍后继续抓取".into(),
        })),
        Err(_) if !transferring => Err(FetchFailure {
            // Redirect admission waits share the overall deadline, but no
            // request is using that time. Report local congestion without
            // changing the source's health, failure streak or due date.
            error: Error::FetchDeferred {
                until: None,
                message: "本地抓取队列繁忙，等待下载名额超时，请稍后重试".into(),
            },
            class: FailureClass::Fast,
            network: false,
            http_response: received_http,
            timeout: true,
            kind: crate::network::ErrorKind::Timeout,
            response_status,
        }),
        Err(_) => Err(FetchFailure {
            error: error("订阅请求超时"),
            class: FailureClass::Slow,
            network: transferring,
            http_response: received_http,
            timeout: true,
            kind: crate::network::ErrorKind::Timeout,
            response_status,
        }),
    }
    .map_err(|mut failure| {
        failure.response_status = failure.response_status.or(response_status);
        failure
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn classification_preserves_timeouts_and_elapsed_resource_cost() {
        assert_eq!(
            failure_class(Duration::from_millis(1), false),
            FailureClass::Fast
        );
        assert_eq!(failure_class(CONNECT_TIMEOUT, false), FailureClass::Slow);
        assert_eq!(
            failure_class(Duration::from_millis(1), true),
            FailureClass::Slow
        );
    }

    #[tokio::test]
    async fn transport_failures_are_classified_before_redaction_but_policy_errors_are_local() {
        let client = client_builder().no_proxy().build().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let refused = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);
        let request = |url| FetchRequest {
            url,
            etag: None,
            last_modified: None,
        };
        let policy = FeedPolicy {
            timeout: Duration::from_secs(1),
            ..FeedPolicy::default()
        };
        let failure = fetch_classified(
            &client,
            request(refused),
            policy,
            |_| async { Ok(()) },
            |_| {},
        )
        .await
        .unwrap_err();
        assert_eq!(failure.class, FailureClass::Fast);
        assert_eq!(failure.kind, crate::network::ErrorKind::Refused);
        assert!(failure.network);
        let (url, task) = server(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\nx", true).await;
        let failure = fetch_classified(&client, request(url), policy, |_| async { Ok(()) }, |_| {})
            .await
            .unwrap_err();
        assert_eq!(failure.class, FailureClass::Slow);
        assert_eq!(failure.kind, crate::network::ErrorKind::Timeout);
        assert!(failure.network);
        task.abort();
        let (url, task) = server(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\n123", false).await;
        let failure = fetch_classified(
            &client,
            request(url),
            FeedPolicy {
                max_body_bytes: 1,
                ..policy
            },
            |_| async { Ok(()) },
            |_| {},
        )
        .await
        .unwrap_err();
        assert_eq!(failure.class, FailureClass::Fast);
        assert!(!failure.network);
        task.abort();
    }

    async fn server(reply: &'static [u8], hang: bool) -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/rss", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut head = Vec::new();
            while !head.windows(4).any(|tail| tail == b"\r\n\r\n") {
                let mut chunk = [0; 1024];
                let read = socket.read(&mut chunk).await.unwrap();
                if read == 0 {
                    return;
                }
                head.extend_from_slice(&chunk[..read]);
                assert!(head.len() <= 16 * 1024, "fixture request headers too large");
            }
            socket.write_all(reply).await.unwrap();
            if hang {
                std::future::pending::<()>().await;
            }
        });
        (url, task)
    }

    #[tokio::test]
    async fn body_read_failure_preserves_received_status_when_converted() {
        let (url, task) = server(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\nx", false).await;
        let error = super::super::fetch_http(
            &client_builder().no_proxy().build().unwrap(),
            FetchRequest {
                url,
                etag: None,
                last_modified: None,
            },
        )
        .await
        .unwrap_err();
        task.await.unwrap();
        assert!(matches!(error, Error::Network { status: 200, .. }));
        let original = Error::Http {
            status: 200,
            message: match &error {
                Error::Network { message, .. } => message.to_string(),
                _ => unreachable!(),
            },
        };
        assert_eq!(error.to_string(), original.to_string());
        assert_eq!(
            crate::logsafe::error(&error),
            crate::logsafe::error(&original)
        );
        let failure = FetchFailure::from(error);
        assert!(failure.http_response);
        assert_eq!(failure.kind, crate::network::ErrorKind::http(200));
        assert_eq!(failure.status(), Some(200));
        assert_eq!(failure.class, FailureClass::Fast);
        assert!(!failure.network && !failure.timeout);
    }
    // Only stall cases expect the deadline. Every other case gets a generous
    // one, so a loaded CI runner cannot turn its expected error into a timeout.
    const STALL: Duration = Duration::from_millis(100);
    const ANSWER: Duration = Duration::from_secs(10);
    async fn get(
        reply: &'static [u8],
        hang: bool,
        max: usize,
        deadline: Duration,
    ) -> Result<FetchResponse> {
        let (url, task) = server(reply, hang).await;
        let result = fetch(
            &client_builder().no_proxy().build().unwrap(),
            FetchRequest {
                url,
                etag: None,
                last_modified: None,
            },
            FeedPolicy {
                timeout: deadline,
                max_body_bytes: max,
            },
            |_| async { Ok(()) },
            |_| {},
        )
        .await;
        task.abort();
        result
    }
    #[tokio::test]
    async fn bounded_chunked_body_and_stalled_body() {
        let err = get(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n8\r\n12345678\r\n8\r\n12345678\r\n0\r\n\r\n", false, 10, ANSWER).await.unwrap_err();
        assert!(err.to_string().contains("大小上限"), "{err}");
        let err = get(
            b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\nx",
            true,
            100,
            STALL,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("超时"), "{err}");
    }
    #[tokio::test]
    async fn body_limit_counts_decompressed_bytes_and_headers_share_the_deadline() {
        let err = get(b"\x48\x54\x54\x50\x2f\x31\x2e\x31\x20\x32\x30\x30\x20\x4f\x4b\x0d\x0a\x43\x6f\x6e\x74\x65\x6e\x74\x2d\x45\x6e\x63\x6f\x64\x69\x6e\x67\x3a\x20\x67\x7a\x69\x70\x0d\x0a\x43\x6f\x6e\x74\x65\x6e\x74\x2d\x4c\x65\x6e\x67\x74\x68\x3a\x20\x32\x34\x0d\x0a\x0d\x0a\x1f\x8b\x08\x00\x00\x00\x00\x00\x02\x03\x4b\x4c\xa4\x3d\x00\x00\x64\x7a\x70\xaf\x64\x00\x00\x00", false, 32, ANSWER).await.unwrap_err();
        assert!(err.to_string().contains("大小上限"), "{err}");
        let err = get(b"", true, 32, STALL).await.unwrap_err();
        assert!(err.to_string().contains("超时"), "{err}");
    }
    #[tokio::test]
    async fn not_modified_and_errors_do_not_wait_for_body_and_proxy_auth_is_redacted() {
        for reply in [
            &b"HTTP/1.1 304 Not Modified\r\nContent-Length: 999999\r\nETag: same\r\n\r\n"[..],
            &b"HTTP/1.1 503 Unavailable\r\nContent-Length: 999999\r\nRetry-After: 60\r\n\r\n"[..],
        ] {
            let out = get(reply, true, 1, ANSWER).await.unwrap();
            assert!(out.body.is_empty());
            if out.status == 304 {
                assert_eq!(out.etag.as_deref(), Some("same"));
            } else {
                assert_eq!(out.retry_after.as_deref(), Some("60"));
            }
        }
        let err = get(
            b"HTTP/1.1 407 Proxy Authentication Required\r\nContent-Length: 999\r\n\r\nsecret",
            true,
            100,
            ANSWER,
        )
        .await
        .unwrap_err();
        assert!(!format!("{err:?}").contains("secret"));
        assert!(err.to_string().contains("代理认证"));
    }
}
