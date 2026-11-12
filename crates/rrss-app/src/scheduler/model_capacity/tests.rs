use super::*;
use rrss_ai::admission::Admission;
use std::{future::Future, pin::Pin, task::Poll, time::Duration};

#[tokio::test]
async fn cancelled_response_still_persists_limit_across_tasks_and_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cooldown.db");
    let request = RequestAdmission::new(
        &path,
        "https://service.test/v1",
        Priority::Background,
        false,
    )
    .unwrap();
    let permit = request.acquire().await.unwrap();
    // Drop the response waiter before its first poll, as an expired item or
    // disconnected IPC waiter may do immediately after receiving headers.
    drop(request.response(429, Some("600".into())));
    drop(permit);
    let other = RequestAdmission::new(
        &path,
        "https://SERVICE.test:443/embeddings",
        Priority::Interactive,
        false,
    )
    .unwrap();
    assert!(matches!(
        other.acquire().await,
        Err(rrss_ai::Error::ServiceCooldown)
    ));
    drop(other);
    drop(request);
    let restarted = RequestAdmission::new(
        &path,
        "https://service.test/new-path",
        Priority::Background,
        false,
    )
    .unwrap();
    assert!(matches!(
        restarted.acquire().await,
        Err(rrss_ai::Error::ServiceCooldown)
    ));
    let independent =
        RequestAdmission::new(&path, "https://other.test/v1", Priority::Background, false).unwrap();
    assert!(independent.acquire().await.is_ok());
}

#[tokio::test]
async fn model_limit_save_failure_stops_dispatch_until_snapshot_is_saved() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("store.db");
    let conn = rrss_core::db::open_at(&path).unwrap();
    conn.execute_batch(
        "CREATE TRIGGER reject_limit BEFORE INSERT ON settings
        WHEN NEW.key='scheduler_model_cooldowns' BEGIN SELECT RAISE(FAIL,'injected'); END;",
    )
    .unwrap();
    let request =
        RequestAdmission::new(&path, "https://service.test", Priority::Background, false).unwrap();
    assert!(request.response(503, Some("600".into())).await.is_err());
    assert!(matches!(
        request.acquire().await,
        Err(rrss_ai::Error::Admission(_))
    ));
    conn.execute_batch("DROP TRIGGER reject_limit").unwrap();
    assert!(matches!(
        request.acquire().await,
        Err(rrss_ai::Error::ServiceCooldown)
    ));
    assert!(
        rrss_core::db::get_setting(&conn, "scheduler_model_cooldowns")
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn production_rate_limit_prevents_a_following_embedding_request() {
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path},
    };
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "600"))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/embeddings"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("model.db");
    let admission = RequestAdmission::new(&db, &server.uri(), Priority::Background, false).unwrap();
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let request = rrss_ai::CompleteRequest {
        base_url: server.uri(),
        api_key: None,
        model: "fake".into(),
        system: "s".into(),
        user: "u".into(),
        json_schema: None,
        protocol: rrss_ai::Protocol::Chat,
    };
    let result = rrss_ai::complete::complete_admitted_until(
        &client,
        &request,
        None,
        &|_, _| {},
        tokio::time::Instant::now() + Duration::from_secs(30),
        Some(&admission),
    )
    .await;
    assert!(matches!(
        result,
        Err(rrss_ai::Error::Http { status: 429, .. })
    ));
    let result = rrss_ai::embed::embed_batch_admitted_until(
        &client,
        &rrss_ai::EmbedBatchRequest {
            base_url: server.uri(),
            api_key: None,
            model: "fake".into(),
            inputs: vec!["text".into()],
        },
        tokio::time::Instant::now() + Duration::from_secs(30),
        Some(&admission),
    )
    .await;
    assert!(matches!(result, Err(rrss_ai::Error::ServiceCooldown)));
}

async fn enqueue<T>(mut future: Pin<&mut impl Future<Output = T>>) {
    std::future::poll_fn(|cx| {
        assert!(future.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
}

#[tokio::test]
async fn interactive_priority_cancellation_and_database_isolation() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("first.db");
    let background = RequestAdmission::new(
        &path,
        "https://service.test/v1/embeddings",
        Priority::Background,
        false,
    )
    .unwrap();
    let interactive = RequestAdmission::new(
        &path,
        "https://SERVICE.test:443/v1/chat/completions",
        Priority::Interactive,
        false,
    )
    .unwrap();
    assert!(Arc::ptr_eq(&background.endpoint, &interactive.endpoint));
    let first = background.acquire().await.unwrap();
    let mut queued = Box::pin(background.acquire());
    enqueue(queued.as_mut()).await;
    let mut cancelled = Box::pin(interactive.acquire());
    enqueue(cancelled.as_mut()).await;
    let mut translate = Box::pin(interactive.acquire());
    enqueue(translate.as_mut()).await;
    let snapshot = background._capacity.diagnostics();
    assert_eq!(
        (
            snapshot.active,
            snapshot.waiting_background,
            snapshot.waiting_interactive
        ),
        (1, 1, 2)
    );
    drop(cancelled);
    assert_eq!(background._capacity.diagnostics().waiting_interactive, 1);
    let other = RequestAdmission::new(
        &dir.path().join("second.db"),
        "https://service.test",
        Priority::Background,
        false,
    )
    .unwrap();
    let _other = other.acquire().await.unwrap();
    drop(first);
    let priority = translate.await.unwrap();
    enqueue(queued.as_mut()).await;
    drop(priority);
    let next = queued.await.unwrap();
    drop(next);
    assert!(background.endpoint.queue.lock().unwrap().waiting.is_empty());
    assert!(background.endpoint.queue.lock().unwrap().active.is_empty());
    let snapshot = background._capacity.diagnostics();
    assert_eq!(
        (
            snapshot.active,
            snapshot.waiting_background,
            snapshot.waiting_interactive
        ),
        (0, 0, 0)
    );
    assert_eq!(snapshot.peak_active, 1);
    assert_eq!(snapshot.held.count, 3);
}

#[tokio::test]
async fn bounded_waiters_are_removed_on_cancel_and_budget_rechecked_after_waiting() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.db");
    let conn = rrss_core::db::open_at(&path).unwrap();
    rrss_core::ai_store::set_daily_budget_tokens(&conn, 10).unwrap();
    let admission =
        RequestAdmission::new(&path, "http://model.test", Priority::Background, true).unwrap();
    let first = admission.acquire().await.unwrap();
    let mut waiting = Vec::new();
    for _ in 0..MAX_WAITERS {
        let mut future = Box::pin(admission.acquire());
        enqueue(future.as_mut()).await;
        waiting.push(future);
    }
    assert!(admission.acquire().await.is_err());
    let last = waiting.pop().unwrap();
    drop(waiting);
    rrss_core::ai_store::add_usage(&conn, &rrss_core::ai_store::local_day(), 10, 0).unwrap();
    drop(first);
    assert!(matches!(last.await, Err(rrss_ai::Error::BudgetExhausted)));
    assert_eq!(admission._capacity.diagnostics().budget_rejections, 1);
    assert!(admission.endpoint.queue.lock().unwrap().active.is_empty());
}

#[tokio::test]
async fn production_completion_and_embedding_share_capacity_and_cache_bypasses_it() {
    use rrss_ai::{CompleteRequest, EmbedBatchRequest, Protocol};
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path},
    };
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "choices":[{"message":{"content":"{}"}}]
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/embeddings"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data":[{"index":0,"embedding":[1.0]}]
        })))
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("capacity.db");
    let background =
        RequestAdmission::new(&db, &server.uri(), Priority::Background, false).unwrap();
    let interactive =
        RequestAdmission::new(&db, &server.uri(), Priority::Interactive, false).unwrap();
    let client = reqwest::Client::new();
    let request = CompleteRequest {
        base_url: server.uri(),
        api_key: None,
        model: "test".into(),
        system: "s".into(),
        user: "u".into(),
        json_schema: None,
        protocol: Protocol::Chat,
    };
    let embed = EmbedBatchRequest {
        base_url: server.uri(),
        api_key: None,
        model: "test".into(),
        inputs: vec!["input".into()],
    };
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    let held = background.acquire().await.unwrap();
    let mut embedding = Box::pin(rrss_ai::embed::embed_batch_admitted_until(
        &client,
        &embed,
        deadline,
        Some(&background),
    ));
    enqueue(embedding.as_mut()).await;
    let mut translation = Box::pin(rrss_ai::complete::complete_admitted_until(
        &client,
        &request,
        None,
        &|_, _| {},
        deadline,
        Some(&interactive),
    ));
    enqueue(translation.as_mut()).await;
    let cached = rrss_ai::complete::complete_admitted_until(
        &client,
        &request,
        Some("{}"),
        &|_, _| {},
        deadline,
        Some(&interactive),
    )
    .await
    .unwrap();
    assert!(cached.from_cache);
    assert!(server.received_requests().await.unwrap().is_empty());
    drop(held);
    // Polling embedding first must not bypass the queued interactive request.
    enqueue(embedding.as_mut()).await;
    translation.await.unwrap();
    embedding.await.unwrap();
    let calls = server.received_requests().await.unwrap();
    assert_eq!(
        calls.iter().map(|r| r.url.path()).collect::<Vec<_>>(),
        vec!["/v1/chat/completions", "/v1/embeddings"]
    );
}

#[tokio::test]
async fn repair_reacquires_capacity_and_usage_survives_wait_cancellation() {
    use rrss_ai::{CompleteRequest, Protocol};
    use wiremock::{Mock, MockServer, ResponseTemplate, matchers::method};
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "choices":[{"message":{"content":"broken JSON"}}],
            "usage":{"prompt_tokens":3,"completion_tokens":2}
        })))
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let admission = RequestAdmission::new(
        &dir.path().join("repair.db"),
        &server.uri(),
        Priority::Background,
        false,
    )
    .unwrap();
    let client = reqwest::Client::new();
    let req = CompleteRequest {
        base_url: server.uri(),
        api_key: None,
        model: "test".into(),
        system: "s".into(),
        user: "u".into(),
        json_schema: None,
        protocol: Protocol::Chat,
    };
    let endpoint = admission.endpoint.clone();
    let usage = std::sync::Mutex::new(Vec::new());
    let observed = |p, c| {
        usage.lock().unwrap().push((p, c));
        let mut queue = endpoint.queue.lock().unwrap();
        assert!(
            queue.active.is_empty(),
            "HTTP attempt releases capacity before parsing/repair"
        );
        queue.active.insert(u64::MAX); // a competing request wins the released slot
    };
    let mut work = Box::pin(rrss_ai::complete::complete_admitted_until(
        &client,
        &req,
        None,
        &observed,
        tokio::time::Instant::now() + std::time::Duration::from_secs(10),
        Some(&admission),
    ));
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        std::future::poll_fn(|cx| {
            assert!(work.as_mut().poll(cx).is_pending());
            if !endpoint.queue.lock().unwrap().waiting.is_empty() {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
        .await;
    })
    .await
    .unwrap();
    drop(work);
    assert!(endpoint.queue.lock().unwrap().waiting.is_empty());
    assert_eq!(*usage.lock().unwrap(), vec![(3, 2)]);
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn content_total_bounds_media_and_prioritizes_reader_waiters() {
    let dir = tempfile::tempdir().unwrap();
    let capacity = Capacity::for_database(&dir.path().join("content.db"));
    let mut running = Vec::new();
    for _ in 0..8 {
        running.push(
            capacity
                .content
                .clone()
                .acquire(Priority::Background)
                .await
                .unwrap(),
        );
    }
    let mut background = Box::pin(capacity.content.clone().acquire(Priority::Background));
    enqueue(background.as_mut()).await;
    let mut reader = Box::pin(capacity.content.clone().acquire(Priority::Interactive));
    enqueue(reader.as_mut()).await;
    running.pop();
    enqueue(background.as_mut()).await;
    let reader = reader.await.unwrap();
    assert_eq!(capacity.content.queue.lock().unwrap().active.len(), 8);
    drop(reader);
    drop(background.await.unwrap());
}
