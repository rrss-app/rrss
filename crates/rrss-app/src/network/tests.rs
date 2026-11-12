use super::*;
use rrss_core::network::{ProxyMode, ProxyRule};
use serde_json::json;
use tokio::io::AsyncWriteExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn changed_configuration_affects_next_client_but_not_an_inflight_request() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let conn = db::open_at(&db_path).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut config = ProxyConfig {
        global: ProxyRule::custom(&format!("http://{}", listener.local_addr().unwrap())),
        ..Default::default()
    };
    network::save(&conn, &config).unwrap();
    let clients = NetworkClients::new(&db_path);
    let before = clients.client(NetworkScope::Api).await.unwrap();
    let (started, began) = tokio::sync::oneshot::channel();
    let (release, released) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        crate::test_net::read_request_head(&mut socket).await;
        started.send(()).unwrap();
        released.await.unwrap();
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\nConnection: close\r\n\r\nbefore")
            .await
            .unwrap();
    });
    let request = tokio::spawn(async move {
        before
            .get("http://service.invalid/")
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap()
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), began)
        .await
        .unwrap()
        .unwrap();
    let next = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("after"))
        .expect(1)
        .mount(&next)
        .await;
    config.global = ProxyRule::custom(&next.uri());
    network::save(&conn, &config).unwrap();
    let after = clients.clone().client(NetworkScope::Api).await.unwrap();
    assert_eq!(
        after
            .get("http://service.invalid/")
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap(),
        "after"
    );
    release.send(()).unwrap();
    assert_eq!(request.await.unwrap(), "before");
    server.await.unwrap();
}

#[tokio::test]
async fn invalid_new_configuration_never_falls_back_to_a_cached_client() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let conn = db::open_at(&db_path).unwrap();
    let clients = NetworkClients::new(&db_path);
    clients.client(NetworkScope::Api).await.unwrap();
    db::set_setting(&conn, network::SETTING_KEY, "broken-private-secret").unwrap();
    let error = clients.client(NetworkScope::Api).await.unwrap_err();
    assert!(!format!("{error:?} {error}").contains("private-secret"));
    network::save(&conn, &ProxyConfig::default()).unwrap();
    clients.client(NetworkScope::Api).await.unwrap();
}

#[tokio::test]
async fn adding_a_feed_and_its_summary_use_content_and_api_routes_separately() {
    let content = MockServer::start().await;
    let api = MockServer::start().await;
    let unused = MockServer::start().await;
    Mock::given(method("GET")).and(path("/rss"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "<rss version='2.0'><channel><title>Mock</title><link>http://feed.invalid</link><description>mock</description><item><guid>a</guid><title>Article</title><description>A local test article.</description></item></channel></rss>"))
        .expect(1).mount(&content).await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{"message": {"content": "{\"summary\":\"Local summary\"}"}}],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1}
        })))
        .expect(1)
        .mount(&api)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"data": [{"id": "local-model"}]})),
        )
        .expect(1)
        .mount(&api)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let conn = db::open_at(&db_path).unwrap();
    db::set_setting(&conn, rrss_core::feed_icon::SETTING_KEY, "0").unwrap();
    db::set_setting(&conn, rrss_core::FOCUS_SUMMARY_NEUTRAL_LIMIT_KEY, "10").unwrap();
    db::set_setting(&conn, rrss_core::ai_store::FOCUS_AUTO_SUMMARY.id, "1").unwrap();
    rrss_core::pi_store::set_pi_auto_download(&conn, false).unwrap();
    rrss_core::ai_store::set_daily_budget_tokens(&conn, 100).unwrap();
    let provider = rrss_core::ai_store::insert_provider(
        &conn,
        &rrss_core::ai_store::NewProvider {
            name: "local".into(),
            base_url: "http://model.invalid/v1".into(),
            api_key: None,
            protocol: "chat".into(),
        },
    )
    .unwrap();
    rrss_core::ai_store::set_task_binding(
        &conn,
        rrss_core::TASK_FOCUS_SUMMARY,
        Some(provider),
        Some("local-model"),
    )
    .unwrap();
    network::save(
        &conn,
        &ProxyConfig {
            global: ProxyRule::custom(&unused.uri()),
            content: ProxyRule::custom(&content.uri()),
            api: ProxyRule::custom(&api.uri()),
            pi: ProxyRule::custom(&unused.uri()),
            ..Default::default()
        },
    )
    .unwrap();
    let clients = NetworkClients::new(&db_path);
    let scheduler = crate::scheduler::Scheduler::start(db_path.clone(), clients.clone(), |_| {});
    let result = scheduler.add_feed("http://feed.invalid/rss").await.unwrap();
    scheduler.submit(result.id).unwrap().wait().await.unwrap();
    assert!(
        api.received_requests().await.unwrap().is_empty(),
        "adding does not trigger AI"
    );
    let version = scheduler.request_background().unwrap();
    scheduler.wait_background(version).await.unwrap();
    let summary: String = conn
        .query_row(
            "SELECT summary FROM articles WHERE feed_id=?1",
            [result.id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(summary, "Local summary");
    let models = crate::capabilities::list_models(
        &db_path,
        &clients.client(NetworkScope::Api).await.unwrap(),
        provider,
        "http://model.invalid/v1",
        None,
    )
    .await
    .unwrap();
    assert_eq!(models, ["local-model"]);
    assert_eq!(content.received_requests().await.unwrap().len(), 1);
    let api_requests = api.received_requests().await.unwrap();
    assert_eq!(api_requests.len(), 2);
    assert!(api_requests.iter().all(|request| {
        request
            .headers
            .get("user-agent")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("Mozilla/5.0"))
    }));
    assert!(unused.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn invalid_api_environment_does_not_disable_content_or_local_jobs() {
    // 系统模式需要有效的环境；这里在独立子进程设环境，避免并行测试相互影响。
    if std::env::var_os("RRSS_NETWORK_ENV_CHILD").is_none() {
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "network::tests::invalid_api_environment_does_not_disable_content_or_local_jobs",
                "--nocapture",
            ])
            .env("RRSS_NETWORK_ENV_CHILD", "1");
        for key in rrss_core::network::PROXY_ENV_KEYS {
            command.env_remove(key);
        }
        command.env("HTTPS_PROXY", "unsupported://private-secret@host");
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let conn = db::open_at(&db_path).unwrap();
    network::save(
        &conn,
        &ProxyConfig {
            api: ProxyRule {
                mode: ProxyMode::System,
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .unwrap();
    db::set_setting(&conn, rrss_core::feed_icon::SETTING_KEY, "0").unwrap();
    conn.execute(
        "INSERT INTO ai_cache(cache_key, value, created_at) VALUES('old', '{}', 0)",
        [],
    )
    .unwrap();
    let clients = NetworkClients::new(&db_path);
    clients.client(NetworkScope::Content).await.unwrap();
    assert!(clients.client(NetworkScope::Api).await.is_err());
    let scheduler = crate::scheduler::Scheduler::start(db_path.clone(), clients, |_| {});
    let version = scheduler.request_background().unwrap();
    scheduler.wait_background(version).await.unwrap();
    let remaining: i64 = conn
        .query_row("SELECT COUNT(*) FROM ai_cache", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        remaining, 0,
        "network failure must not skip local housekeeping"
    );
}

#[tokio::test]
async fn feed_redirects_preserve_validators_and_use_the_specialized_content_client() {
    let origin = MockServer::start().await;
    let target = MockServer::start().await;
    for (server, response) in [
        (
            &origin,
            ResponseTemplate::new(302).insert_header("Location", target.uri()),
        ),
        (
            &target,
            ResponseTemplate::new(304).insert_header("ETag", "version"),
        ),
    ] {
        Mock::given(method("GET"))
            .respond_with(response)
            .expect(1)
            .mount(server)
            .await;
    }
    let dir = tempfile::tempdir().unwrap();
    let clients = NetworkClients::new(dir.path().join("test.db"));
    let visits = std::sync::Mutex::new(Vec::new());
    let response = rrss_core::ingest::feed_http::fetch(
        &clients.feed_client().await.unwrap(),
        rrss_core::ingest::FetchRequest {
            url: origin.uri(),
            etag: Some("version".into()),
            last_modified: Some("Wed, 21 Oct 2015 07:28:00 GMT".into()),
        },
        Default::default(),
        |url| {
            visits.lock().unwrap().push(url);
            async { Ok(()) }
        },
        |_| {},
    )
    .await
    .unwrap();
    assert_eq!(response.status, 304);
    for server in [&origin, &target] {
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].headers.get("if-none-match").unwrap(), "version");
        assert_eq!(
            requests[0].headers.get("if-modified-since").unwrap(),
            "Wed, 21 Oct 2015 07:28:00 GMT"
        );
    }
    assert_eq!(
        visits.lock().unwrap().len(),
        2,
        "each redirect hop must request admission"
    );
}
