use super::*;
use std::sync::{Arc, Mutex};
use wiremock::{Mock, MockServer, ResponseTemplate, matchers::method};

// 期限测试只验证“同一条绝对期限贯穿重试”，不能依赖几十毫秒的调度时序：
// 共享 CI runner 会把测试线程搁置数百毫秒，期限必须明显长于首次响应的往返时间。
const DEADLINE: Duration = Duration::from_secs(5);
const HANG: Duration = Duration::from_secs(10);

async fn wait_for_requests(server: &MockServer, expected: usize) {
    // 超时取消客户端时，最后一次请求可能仍在路上；有界等待 mock 记录它，而不是断言瞬间的状态。
    // 短间隔查询而不是空转：空转会和 mock 记录请求抢锁，忙碌的 runner 上反而拖慢请求被记下。
    tokio::time::timeout(Duration::from_secs(30), async {
        while server.received_requests().await.unwrap().len() < expected {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("request should reach the mock server");
}

#[tokio::test]
async fn repair_shares_deadline_and_preserves_received_usage() {
    let server = MockServer::start().await;
    let calls = Arc::new(Mutex::new(0));
    let count = calls.clone();
    Mock::given(method("POST"))
        .respond_with(move |_: &wiremock::Request| {
            let mut count = count.lock().unwrap();
            *count += 1;
            // 首次立刻返回坏 JSON；修复请求不再获得新期限，被同一条绝对期限中止。
            let (content, delay) = if *count == 1 {
                ("broken", Duration::ZERO)
            } else {
                (r#"{"ok":true}"#, HANG)
            };
            ResponseTemplate::new(200)
                .set_delay(delay)
                .set_body_json(json!({
                    "choices": [{"message": {"content": content}}],
                    "usage": {"prompt_tokens": 3, "completion_tokens": 2}
                }))
        })
        .mount(&server)
        .await;
    let req = CompleteRequest {
        base_url: server.uri(),
        api_key: None,
        model: "m".into(),
        system: "json".into(),
        user: "input".into(),
        json_schema: None,
        protocol: Protocol::Chat,
    };
    let usage = Mutex::new(Vec::new());
    let out = complete_cached_until(
        &reqwest::Client::new(),
        &req,
        None,
        &|p, c| usage.lock().unwrap().push((p, c)),
        tokio::time::Instant::now() + DEADLINE,
    )
    .await;
    assert!(matches!(
        out,
        Err(Error::Request {
            kind: crate::complete::RequestKind::Timeout,
            ..
        })
    ));
    assert_eq!(*usage.lock().unwrap(), vec![(3, 2)]);
    wait_for_requests(&server, 2).await;
    assert_eq!(*calls.lock().unwrap(), 2);
    // Expiration never turns a valid cache hit into a network request.
    let out = complete_cached_until(
        &reqwest::Client::new(),
        &req,
        Some(r#"{"ok":true}"#),
        &|_, _| panic!("cached usage"),
        tokio::time::Instant::now(),
    )
    .await
    .unwrap();
    assert!(out.from_cache);
    assert_eq!(*calls.lock().unwrap(), 2);
}

#[tokio::test]
async fn protocol_fallback_does_not_reset_deadline() {
    let server = MockServer::start().await;
    Mock::given(wiremock::matchers::path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    Mock::given(wiremock::matchers::path("/v1/responses"))
        .respond_with(ResponseTemplate::new(200).set_delay(HANG))
        .mount(&server)
        .await;
    let req = CompleteRequest {
        base_url: server.uri(),
        api_key: None,
        model: "m".into(),
        system: "json".into(),
        user: "input".into(),
        json_schema: None,
        protocol: Protocol::Auto,
    };
    assert!(matches!(
        complete_cached_until(
            &reqwest::Client::new(),
            &req,
            None,
            &|_, _| {},
            tokio::time::Instant::now() + DEADLINE
        )
        .await,
        Err(Error::Request {
            kind: crate::complete::RequestKind::Timeout,
            ..
        })
    ));
    wait_for_requests(&server, 2).await;
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}
