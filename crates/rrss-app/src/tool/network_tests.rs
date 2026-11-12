use super::*;
use rrss_core::network::{self, ProxyConfig, ProxyRule};
use std::path::Path;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn invoke(path: &Path, command: &str, url: &str) -> Result<String> {
    let path = path.to_path_buf();
    let args: Vec<_> = ["rrss", "tool", command, "--url", url]
        .into_iter()
        .map(str::to_owned)
        .collect();
    tokio::task::spawn_blocking(move || {
        let conn = db::open_at(&path)?;
        run_with_connection(&args, &conn, QUERY_EMBED_TIMEOUT)
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn fetch_url_reads_current_database_content_proxy_and_keeps_permission_gate() {
    let origin = MockServer::start().await;
    let first = MockServer::start().await;
    let next = MockServer::start().await;
    let unused = MockServer::start().await;
    for (server, text) in [(&first, "first"), (&next, "next")] {
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string(format!("<p>{text}</p>")))
            .expect(1)
            .mount(server)
            .await;
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tool.db");
    let conn = db::open_at(&path).unwrap();
    let mut config = ProxyConfig {
        global: ProxyRule::custom(unused.uri()),
        content: ProxyRule::custom(first.uri()),
        api: ProxyRule::custom(unused.uri()),
        pi: ProxyRule::custom(unused.uri()),
        ..Default::default()
    };
    network::save(&conn, &config).unwrap();
    assert!(
        invoke(&path, "fetch_url", &origin.uri())
            .await
            .unwrap_err()
            .to_string()
            .contains("disabled")
    );
    assert!(first.received_requests().await.unwrap().is_empty());
    db::set_setting(&conn, FETCH_URL_ENABLED_KEY, "1").unwrap();
    // 目标不能用回环地址：默认绕过 localhost / 127.0.0.1 / ::1，回环目标不会走代理。
    let target = "http://content.example/article";
    let value: Value =
        serde_json::from_str(&invoke(&path, "fetch_url", target).await.unwrap()).unwrap();
    assert_eq!(value["text"], "first");
    assert_eq!(value["url"], target);
    config.content = ProxyRule::custom(next.uri());
    network::save(&conn, &config).unwrap();
    let value: Value =
        serde_json::from_str(&invoke(&path, "fetch_url", target).await.unwrap()).unwrap();
    assert_eq!(value["text"], "next");
    db::set_setting(&conn, network::SETTING_KEY, "broken-private-secret").unwrap();
    let error = invoke(&path, "fetch_url", target).await.unwrap_err();
    assert!(!format!("{error:#?} {error:#}").contains("private-secret"));
    // 网络配置损坏仍允许纯本地工具运行。
    assert_eq!(invoke(&path, "list_feeds", "").await.unwrap().trim(), "[]");
    assert!(origin.received_requests().await.unwrap().is_empty());
    assert!(unused.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn fetch_url_proxy_auth_failure_never_returns_response_body() {
    let proxy = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(407).set_body_string("proxy-user:proxy-password"))
        .expect(1)
        .mount(&proxy)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tool.db");
    let conn = db::open_at(&path).unwrap();
    db::set_setting(&conn, FETCH_URL_ENABLED_KEY, "1").unwrap();
    network::save(
        &conn,
        &ProxyConfig {
            content: ProxyRule::custom(proxy.uri().replacen(
                "http://",
                "http://proxy-user:proxy-password@",
                1,
            )),
            ..Default::default()
        },
    )
    .unwrap();
    let error = invoke(&path, "fetch_url", "http://content.invalid/")
        .await
        .unwrap_err();
    let text = format!("{error:#} {error:#?}");
    assert!(text.contains("407"));
    assert!(!text.contains("proxy-user"));
    assert!(!text.contains("proxy-password"));
}
