use super::*;
use crate::translate::cache::configuration_fingerprint;
use rrss_core::network::{self, NetworkScope, ProxyConfig, ProxyRule};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn routes(api: &str, unused: &str) -> ProxyConfig {
    ProxyConfig {
        global: ProxyRule::custom(unused),
        content: ProxyRule::custom(unused),
        api: ProxyRule::custom(api),
        pi: ProxyRule::custom(unused),
        ..Default::default()
    }
}

fn response(engine: &str, request: &Request) -> ResponseTemplate {
    let value = match engine {
        "complete" => complete_response(request),
        "http" => json!({"translatedText":"已翻译"}),
        "deepl" => json!({"translations":[{"text":"已翻译"}]}),
        "deeplx" => json!({"code":200,"data":"已翻译"}),
        _ => unreachable!(),
    };
    ResponseTemplate::new(200).set_body_json(value)
}

#[tokio::test]
async fn saved_proxy_switches_later_windows_and_tasks_without_invalidating_translation() {
    for engine in ["complete", "http", "deepl", "deeplx"] {
        let target = crate::test_net::proxied_target().await;
        let unused = MockServer::start().await;
        let first = MockServer::start().await;
        let next = MockServer::start().await;
        let mut h = Harness::http(
            &target.uri(),
            engine,
            Limits {
                chunk_chars: 1,
                ..Limits::default()
            },
        );
        network::save(&h.conn(), &routes(&first.uri(), &unused.uri())).unwrap();
        let fingerprint = configuration_fingerprint(&h.conn()).unwrap();
        let db_path = h.runtime.path.clone();
        let next_config = routes(&next.uri(), &unused.uri());
        Mock::given(method("POST"))
            .respond_with(move |request: &Request| {
                // The old proxy has accepted the request, but has not sent its response yet.
                // Saving here deterministically exercises an in-flight client snapshot.
                network::save(&db::open_at(&db_path).unwrap(), &next_config).unwrap();
                response(engine, request)
            })
            .expect(1)
            .mount(&first)
            .await;
        Mock::given(method("POST"))
            .respond_with(move |request: &Request| response(engine, request))
            .expect(2)
            .mount(&next)
            .await;
        let start = h.start(StartMode::Default).await;
        let end = h.end().await;
        assert_eq!(end.phase, Phase::Completed, "{engine}");
        assert_eq!(end.identity, start.identity, "{engine}");
        assert_eq!(end.done, 3, "the in-flight result must still be accepted");
        assert_eq!(first.received_requests().await.unwrap().len(), 1);
        assert_eq!(next.received_requests().await.unwrap().len(), 2);
        assert!(target.received_requests().await.unwrap().is_empty());
        assert!(unused.received_requests().await.unwrap().is_empty());
        assert_eq!(configuration_fingerprint(&h.conn()).unwrap(), fingerprint);
        assert!(Arc::ptr_eq(
            &h.runtime,
            &Runtime::new(&h.runtime.path, Arc::new(|_: Event| {})).unwrap()
        ));

        // Offline article caches are independent of even a broken proxy configuration.
        db::set_setting(&h.conn(), network::SETTING_KEY, "broken-proxy-secret").unwrap();
        assert!(matches!(
            h.runtime.start(7, StartMode::Default, None).await.unwrap(),
            StartResult::Cached { .. }
        ));
        network::save(&h.conn(), &ProxyConfig::default()).unwrap();
        Mock::given(method("POST"))
            .respond_with(move |request: &Request| response(engine, request))
            .expect(3)
            .mount(&target)
            .await;
        let regenerated = h.start(StartMode::Regenerate).await;
        assert_ne!(regenerated.identity.generation, start.identity.generation);
        assert_eq!(regenerated.identity.config_hash, start.identity.config_hash);
        assert_eq!(h.end().await.phase, Phase::Completed);
        assert_eq!(target.received_requests().await.unwrap().len(), 3);
        assert_eq!(next.received_requests().await.unwrap().len(), 2);
        assert!(unused.received_requests().await.unwrap().is_empty());
    }
}

#[tokio::test]
async fn complete_format_retry_retains_the_current_window_proxy_snapshot() {
    let target = crate::test_net::proxied_target().await;
    let first = MockServer::start().await;
    let next = MockServer::start().await;
    let mut h = Harness::http(&target.uri(), "complete", Limits::default());
    network::save(&h.conn(), &routes(&first.uri(), &target.uri())).unwrap();
    let path = h.runtime.path.clone();
    let next_config = routes(&next.uri(), &target.uri());
    Mock::given(method("POST"))
        .respond_with(move |_: &Request| {
            network::save(&db::open_at(&path).unwrap(), &next_config).unwrap();
            ResponseTemplate::new(200).set_body_json(json!({
                "choices":[{"message":{"content":"not JSON"}}],
                "usage":{"prompt_tokens":10,"completion_tokens":5}
            }))
        })
        .up_to_n_times(1)
        .expect(1)
        .mount(&first)
        .await;
    Mock::given(method("POST"))
        .respond_with(|request: &Request| response("complete", request))
        .expect(1)
        .mount(&first)
        .await;
    Mock::given(method("POST"))
        .respond_with(|request: &Request| response("complete", request))
        .expect(1)
        .mount(&next)
        .await;
    h.start(StartMode::Default).await;
    assert_eq!(h.end().await.phase, Phase::Completed);
    assert_eq!(first.received_requests().await.unwrap().len(), 2);
    assert!(next.received_requests().await.unwrap().is_empty());
    h.start(StartMode::Regenerate).await;
    assert_eq!(h.end().await.phase, Phase::Completed);
    assert_eq!(next.received_requests().await.unwrap().len(), 1);
    assert!(target.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn broken_proxy_stops_new_work_instead_of_using_direct_or_cached_clients() {
    for change_inflight in [false, true] {
        let target = crate::test_net::proxied_target().await;
        let proxy = MockServer::start().await;
        let mut h = Harness::http(
            &target.uri(),
            "http",
            Limits {
                chunk_chars: 1,
                ..Limits::default()
            },
        );
        network::save(&h.conn(), &routes(&proxy.uri(), &target.uri())).unwrap();
        let path = h.runtime.path.clone();
        Mock::given(method("POST"))
            .respond_with(move |request: &Request| {
                db::set_setting(
                    &db::open_at(&path).unwrap(),
                    network::SETTING_KEY,
                    "broken-proxy-secret",
                )
                .unwrap();
                response("http", request)
            })
            .expect(if change_inflight { 1 } else { 0 })
            .mount(&proxy)
            .await;
        if !change_inflight {
            db::set_setting(&h.conn(), network::SETTING_KEY, "broken-proxy-secret").unwrap();
        }
        h.start(StartMode::Default).await;
        let end = h.end().await;
        assert_eq!(end.error, Some(Failure::Configuration));
        assert_eq!(end.done, usize::from(change_inflight));
        assert!(!format!("{end:?}").contains("proxy-secret"));
        assert!(target.received_requests().await.unwrap().is_empty());
    }
}

#[tokio::test]
async fn service_transport_errors_drop_reqwest_sources_and_skip_407_bodies() {
    for engine in ["http", "deepl", "deeplx"] {
        for mode in ["send_timeout", "body_timeout", "body_error", "407"] {
            let target = crate::test_net::proxied_target().await;
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let proxy_url = format!(
                "http://proxy-user:proxy-password@{}",
                listener.local_addr().unwrap()
            );
            let endpoint = format!("{}/private?token=endpoint-secret", target.uri());
            let h = Harness::http(&endpoint, engine, Limits::default());
            network::save(&h.conn(), &routes(&proxy_url, &target.uri())).unwrap();
            let client = network::load(&h.conn())
                .unwrap()
                .build_client(
                    NetworkScope::Api,
                    reqwest::Client::builder().timeout(Duration::from_millis(300)),
                )
                .unwrap();
            let config = Config::read(&h.conn()).unwrap();
            let (release, released) = oneshot::channel();
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let headers = crate::test_net::read_request_head(&mut socket).await;
                // Drain the request to avoid a reset masking the response status.
                let length: usize = headers
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length:"))
                    .unwrap()
                    .trim()
                    .parse()
                    .unwrap();
                let mut body = vec![0; length];
                socket.read_exact(&mut body).await.unwrap();
                if mode != "send_timeout" {
                    let status = if mode == "407" {
                        "407 Proxy Authentication Required"
                    } else {
                        "200 OK"
                    };
                    socket
                        .write_all(
                            format!(
                                "HTTP/1.1 {status}\r\nContent-Length: 4096\r\nConnection: close\r\n\r\n"
                            )
                            .as_bytes(),
                        )
                        .await
                        .unwrap();
                }
                if mode == "body_error" {
                    // A truncated body must also lose its raw Reqwest source.
                    socket
                        .write_all(b"proxy-user proxy-password")
                        .await
                        .unwrap();
                    return;
                }
                // In the 407 case no response body is sent. Reading it would time out.
                let _ = released.await;
            });
            let units = [TranslateUnit {
                i: 0,
                t: "English title".into(),
            }];
            let error = tokio::time::timeout(
                Duration::from_secs(5),
                crate::translate::non_complete_window(&h.runtime.path, &client, &config, &units),
            )
            .await
            .unwrap()
            .unwrap_err();
            let _ = release.send(());
            server.await.unwrap();
            assert_eq!(
                window::classify(&error),
                match mode {
                    "407" => Failure::Configuration,
                    "body_error" => Failure::Network,
                    _ => Failure::Timeout,
                },
                "{engine}: {mode}"
            );
            assert!(
                error.chain().all(|cause| !cause.is::<reqwest::Error>()),
                "{engine}: {mode}: raw Reqwest source must not escape"
            );
            let rendered = format!("{error:?} {error:#}");
            for secret in ["proxy-user", "proxy-password", "endpoint-secret"] {
                assert!(!rendered.contains(secret), "{engine}: {mode}");
            }
            assert!(target.received_requests().await.unwrap().is_empty());
        }
    }
}

#[tokio::test]
async fn runtime_proxy_authentication_and_timeout_are_safe_classified_failures() {
    for engine in ["complete", "http", "deepl", "deeplx"] {
        for timeout in [false, true] {
            let target = crate::test_net::proxied_target().await;
            let proxy = MockServer::start().await;
            let mut h = Harness::http(
                &target.uri(),
                engine,
                Limits {
                    // One title makes every backend execute exactly one window.
                    timeout: Duration::from_millis(500),
                    ..Limits::default()
                },
            );
            h.conn()
                .execute("UPDATE articles SET content_html='' WHERE id=7", [])
                .unwrap();
            let url = proxy
                .uri()
                .replacen("http://", "http://proxy-user:proxy-password@", 1);
            network::save(&h.conn(), &routes(&url, &target.uri())).unwrap();
            Mock::given(method("POST"))
                .respond_with(if timeout {
                    ResponseTemplate::new(200)
                        .set_delay(Duration::from_secs(2))
                        .set_body_string("proxy-user proxy-password")
                } else {
                    ResponseTemplate::new(407)
                        .set_body_string("proxy-user proxy-password sensitive proxy diagnostic")
                })
                .mount(&proxy)
                .await;
            h.start(StartMode::Default).await;
            let end = h.end().await;
            let expected = if timeout {
                Failure::Timeout
            } else {
                Failure::Configuration
            };
            assert_eq!(end.error, Some(expected), "{engine}");
            let error = format!("{end:?}");
            assert!(!error.contains("proxy-user"));
            assert!(!error.contains("proxy-password"));
            assert!(!error.contains("sensitive proxy diagnostic"));
            assert!(target.received_requests().await.unwrap().is_empty());
        }
    }
}
