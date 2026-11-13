use super::*;
use rrss_core::ai_store;
use serde_json::{Value, json};
use wiremock::{Mock, MockServer, Request, ResponseTemplate, matchers::method};

mod proxy;

impl Harness {
    fn http(url: &str, engine: &str, limits: Limits) -> Self {
        let mut h = Self::with_limits(limits);
        let conn = h.conn();
        translations::set_backend(&conn, engine).unwrap();
        match engine {
            "http" => translations::set_http_config(&conn, Some(url), None).unwrap(),
            "deeplx" => {
                translations::set_deeplx_config(&conn, Some(url), Some("local-token")).unwrap()
            }
            "deepl" => translations::set_deepl_config(&conn, Some(url), Some("local-key")).unwrap(),
            "complete" => {
                let pid = crate::test_net::fake_provider(&conn, "local mock", url.into());
                ai_store::set_task_binding(
                    &conn,
                    ai_store::TASK_ARTICLE_TRANSLATE,
                    Some(pid),
                    Some("mock-model"),
                )
                .unwrap();
            }
            _ => unreachable!(),
        }
        let (send, events) = mpsc::unbounded_channel();
        h.events = events;
        h.runtime = Runtime::build(
            h.runtime.path.clone(),
            Arc::new(HttpBackend(crate::network::NetworkClients::new(
                &h.runtime.path,
            ))),
            Arc::new(move |event| {
                let _ = send.send(event);
            }),
            limits,
        );
        RUNTIMES
            .get_or_init(Mutex::default)
            .lock()
            .unwrap()
            .push(Arc::downgrade(&h.runtime));
        h
    }
}
fn complete_response(request: &Request) -> Value {
    let body: Value = request.body_json().unwrap();
    let user = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["role"] == "user")
        .unwrap()["content"]
        .as_str()
        .unwrap();
    let (_, units) = user.split_once('\n').unwrap();
    let units: Vec<TranslateUnit> = serde_json::from_str(units).unwrap();
    json!({"choices":[{"message":{"content":json!({"units":units.into_iter().map(|u| json!({"i":u.i,"t":"已翻译"})).collect::<Vec<_>>()}).to_string()}}],"usage":{"prompt_tokens":10,"completion_tokens":5}})
}

#[tokio::test]
async fn all_backends_execute_one_window_and_complete_records_usage_once() {
    for engine in ["complete", "http", "deepl", "deeplx"] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(move |request: &Request| {
                let value = match engine {
                    "complete" => complete_response(request),
                    "http" => json!({"translatedText":"已翻译"}),
                    "deepl" => json!({"translations":[{"text":"已翻译"}]}),
                    "deeplx" => json!({"code":200,"data":"已翻译"}),
                    _ => unreachable!(),
                };
                ResponseTemplate::new(200).set_body_json(value)
            })
            .mount(&server)
            .await;
        let mut h = Harness::http(
            &server.uri(),
            engine,
            Limits {
                chunk_chars: 1,
                ..Limits::default()
            },
        );
        h.start(StartMode::Default).await;
        assert_eq!(h.end().await.phase, Phase::Completed, "{engine}");
        let requests = server.received_requests().await.unwrap();
        assert_eq!(
            requests.len(),
            3,
            "{engine} must leave all window selection to runtime"
        );
        if engine == "complete" {
            let conn = h.conn();
            let (p, c): (i64, i64) = conn
                .query_row(
                    "SELECT prompt_tokens,completion_tokens FROM ai_usage",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .unwrap();
            assert_eq!((p, c), (30, 15));
            assert!(matches!(
                h.runtime.start(7, StartMode::Default, None).await.unwrap(),
                StartResult::Cached { .. }
            ));
            assert_eq!(server.received_requests().await.unwrap().len(), 3);
        }
    }
}

#[tokio::test]
async fn complete_partial_output_is_not_cached_and_default_retry_only_requests_missing_id() {
    let server = MockServer::start().await;
    Mock::given(method("POST")).respond_with(ResponseTemplate::new(200).set_body_json(json!({
        "choices":[{"message":{"content":r#"{"units":[{"i":0,"t":"标题"},{"i":2,"t":"末段"}]}"#}}],
        "usage":{"prompt_tokens":10,"completion_tokens":5}
    }))).up_to_n_times(1).mount(&server).await;
    Mock::given(method("POST"))
        .respond_with(|r: &Request| ResponseTemplate::new(200).set_body_json(complete_response(r)))
        .mount(&server)
        .await;
    let mut h = Harness::http(&server.uri(), "complete", Limits::default());
    h.start(StartMode::Default).await;
    assert_eq!(h.end().await.phase, Phase::Partial);
    assert_eq!(
        h.conn()
            .query_row::<i64, _, _>("SELECT count(*) FROM ai_cache", [], |r| r.get(0))
            .unwrap(),
        0
    );
    let retry = h.start(StartMode::Default).await;
    assert_eq!(
        retry.units,
        [UnitState::Done, UnitState::Pending, UnitState::Done]
    );
    assert_eq!(h.end().await.phase, Phase::Completed);
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 2);
    let body: Value = requests[1].body_json().unwrap();
    let content = body["messages"][1]["content"].as_str().unwrap();
    let (_, units) = content.split_once('\n').unwrap();
    let units: Vec<TranslateUnit> = serde_json::from_str(units).unwrap();
    assert_eq!(units.len(), 1);
    assert_eq!(units[0].i, 1);
}

#[tokio::test]
async fn unknown_usage_creates_no_zero_usage_row_and_invalidated_response_still_counts() {
    for mode in ["unknown", "incomplete", "invalidated"] {
        let server = MockServer::start().await;
        let mut h = Harness::http(&server.uri(), "complete", Limits::default());
        let db_path = h.runtime.path.clone();
        Mock::given(method("POST"))
            .respond_with(move |r: &Request| {
                let mut response = complete_response(r);
                match mode {
                    "unknown" => {
                        response.as_object_mut().unwrap().remove("usage");
                    }
                    "incomplete" => {
                        response["usage"] = json!({"prompt_tokens":10});
                    }
                    "invalidated" => {
                        let conn = db::open_at(&db_path).unwrap();
                        crate::settings_schema::set(
                            &conn,
                            rrss_core::translations::TRANSLATE_TARGET_LANG_KEY,
                            "fr",
                        )
                        .unwrap();
                        crate::settings_schema::set(
                            &conn,
                            rrss_core::translations::TRANSLATE_TARGET_LANG_KEY,
                            "zh",
                        )
                        .unwrap();
                    }
                    _ => unreachable!(),
                }
                ResponseTemplate::new(200).set_body_json(response)
            })
            .mount(&server)
            .await;
        h.start(StartMode::Default).await;
        let end = h.end().await;
        if mode == "invalidated" {
            assert_eq!(end.phase, Phase::Invalidated);
            // Invalidation event intentionally precedes the response; wait for this worker's accounting.
            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    if !h.runtime.state.lock().unwrap().worker {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .unwrap();
        } else {
            assert_eq!(end.phase, Phase::Completed);
        }
        assert_eq!(
            h.conn()
                .query_row::<i64, _, _>("SELECT count(*) FROM ai_usage", [], |r| r.get(0))
                .unwrap(),
            if mode == "invalidated" { 1 } else { 0 }
        );
    }
}

#[tokio::test]
async fn total_deadline_includes_fallback_and_keeps_usage_from_response_before_timeout() {
    let server = MockServer::start().await;
    // Invalid JSON has real usage. The prompt-only retry must share the original deadline.
    // First usage arrives immediately; the repair hangs past the original deadline.
    Mock::given(method("POST")).respond_with(ResponseTemplate::new(200).set_body_json(json!({
        "choices":[{"message":{"content":"not JSON"}}], "usage":{"prompt_tokens":10,"completion_tokens":5}
    }))).up_to_n_times(1).mount(&server).await;
    Mock::given(method("POST")).respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(10)).set_body_json(json!({
        "choices":[{"message":{"content":r#"{"units":[{"i":0,"t":"标题"},{"i":1,"t":"一"},{"i":2,"t":"二"}]}"#}}], "usage":{"prompt_tokens":99,"completion_tokens":99}
    }))).mount(&server).await;
    let mut h = Harness::http(
        &server.uri(),
        "complete",
        Limits {
            timeout: Duration::from_secs(5),
            ..Limits::default()
        },
    );
    h.start(StartMode::Default).await;
    let end = h.end().await;
    assert!(matches!(end.kind, EventKind::Error(Failure::Timeout)));
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
    // Cancellation releases the window before the independent usage writer settles.
    let capacity = crate::scheduler::model_capacity::Capacity::for_database(&h.runtime.path);
    let mut settling = capacity.active_work.subscribe();
    tokio::time::timeout(
        Duration::from_secs(10),
        settling.wait_for(|count| *count == 0),
    )
    .await
    .unwrap()
    .unwrap();
    let (p, c): (i64, i64) = h
        .conn()
        .query_row(
            "SELECT prompt_tokens,completion_tokens FROM ai_usage",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!((p, c), (10, 5), "only received real usage is persisted");
}
