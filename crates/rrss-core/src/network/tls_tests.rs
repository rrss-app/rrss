//! 代理矩阵里需要 TLS 的部分（规格 23 · 8.2 / P5）：HTTP 代理经 CONNECT 转发 HTTPS 目标、
//! HTTPS 代理（到代理这一段走 TLS）转发 HTTP 与 HTTPS 目标。证书是 `testdata/tls` 里仅供测试的
//! 自签 CA 与叶子证书（`proxy.test`、`target.test`、127.0.0.1，有效期一百年），不联网。

use super::*;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;
use tokio_rustls::rustls::ServerConfig;
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

const CA: &[u8] = include_bytes!("../../../../testdata/tls/ca.der");
const LEAF: &[u8] = include_bytes!("../../../../testdata/tls/leaf.der");
const LEAF_KEY: &[u8] = include_bytes!("../../../../testdata/tls/leaf.key.der");

fn acceptor() -> TlsAcceptor {
    let provider = Arc::new(tokio_rustls::rustls::crypto::ring::default_provider());
    let config = ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![CertificateDer::from(LEAF.to_vec())],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(LEAF_KEY.to_vec())),
        )
        .unwrap();
    TlsAcceptor::from(Arc::new(config))
}

fn client(proxy: &str) -> Client {
    ProxyConfig {
        global: ProxyRule::custom(proxy),
        ..Default::default()
    }
    .build_client_with_env(
        NetworkScope::Content,
        Client::builder()
            .timeout(Duration::from_secs(5))
            .add_root_certificate(reqwest::Certificate::from_der(CA).unwrap()),
        &ProxyEnvironment::default(),
    )
    .unwrap()
}

async fn read_head<S: AsyncRead + Unpin>(stream: &mut S) -> String {
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        head.push(stream.read_u8().await.unwrap());
        assert!(head.len() < 16_384);
    }
    String::from_utf8(head).unwrap().to_ascii_lowercase()
}

async fn respond<S: AsyncWrite + Unpin>(stream: &mut S, body: &str) {
    stream
        .write_all(
            format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    stream.flush().await.unwrap();
}

/// 代理收到 CONNECT 后回 200，再在隧道里以 `target.test` 的身份完成 TLS 并应答；
/// 返回（CONNECT 请求头，隧道内请求头）。
async fn tunnel<S: AsyncRead + AsyncWrite + Unpin>(mut stream: S) -> (String, String) {
    let connect = read_head(&mut stream).await;
    stream
        .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
        .await
        .unwrap();
    let mut inner = acceptor().accept(stream).await.unwrap();
    let request = read_head(&mut inner).await;
    respond(&mut inner, "secure").await;
    let _ = inner.shutdown().await;
    (connect, request)
}

#[tokio::test]
async fn https_targets_tunnel_through_http_and_https_proxies() {
    for proxy_tls in [false, true] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let scheme = if proxy_tls { "https" } else { "http" };
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            if proxy_tls {
                tunnel(acceptor().accept(stream).await.unwrap()).await
            } else {
                tunnel(stream).await
            }
        });
        let client = client(&format!("{scheme}://user:pass@{addr}"));
        let body = client
            .get("https://target.test/secure")
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert_eq!(body, "secure", "{scheme} proxy");
        let (connect, request) = tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap();
        // 目标域名交给代理，认证只出现在发给代理的 CONNECT 里，不进隧道内的请求。
        assert!(
            connect.starts_with("connect target.test:443 http/1.1"),
            "{connect}"
        );
        assert!(
            connect.contains("proxy-authorization: basic dxnlcjpwyxnz"),
            "{connect}"
        );
        assert!(request.starts_with("get /secure http/1.1"), "{request}");
        assert!(!request.contains("proxy-authorization"), "{request}");
    }
}

#[tokio::test]
async fn https_proxy_forwards_plain_http_targets() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut stream = acceptor().accept(stream).await.unwrap();
        let head = read_head(&mut stream).await;
        respond(&mut stream, "plain").await;
        let _ = stream.shutdown().await;
        head
    });
    let body = client(&format!("https://{addr}"))
        .get("http://target.test/plain")
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(body, "plain");
    let head = tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();
    assert!(
        head.starts_with("get http://target.test/plain http/1.1"),
        "{head}"
    );
}

#[tokio::test]
async fn untrusted_https_proxy_fails_without_falling_back_to_direct() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        // 客户端不信任测试 CA 时握手失败；服务端只需看到有人连上。
        let _ = acceptor().accept(stream).await;
    });
    let client = ProxyConfig {
        global: ProxyRule::custom(format!("https://user:pass@{addr}")),
        ..Default::default()
    }
    .build_client_with_env(
        NetworkScope::Content,
        Client::builder().timeout(Duration::from_secs(5)),
        &ProxyEnvironment::default(),
    )
    .unwrap();
    let error = client
        .get("http://target.test/plain")
        .send()
        .await
        .unwrap_err();
    let safe = request_error(&error).to_string();
    assert_eq!(error_kind(&error), ErrorKind::TlsCert);
    assert!(!safe.contains("pass"), "{safe}");
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();
}
