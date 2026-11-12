use std::time::Duration;

use rrss_ai::{CompleteRequest, EmbedRequest, Error, Protocol};
use wiremock::matchers::any;
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn completion_and_embedding_errors_clean_the_full_message_before_clipping() {
    let key = "private-\"credential\\canary";
    for body in [
        format!("{} {key} rejected", "x".repeat(390)),
        serde_json::json!({"error": {"message": format!(
            "rejected {key}\n{}\nauthorization: bearer other-canary\nretry later",
            serde_json::json!({"detail": key})
        )}})
        .to_string(),
    ] {
        let server = MockServer::start().await;
        Mock::given(any())
            .and(wiremock::matchers::header(
                "authorization",
                format!("Bearer {key}"),
            ))
            .respond_with(ResponseTemplate::new(401).set_body_string(body.clone()))
            .mount(&server)
            .await;
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let mut errors = Vec::new();
        for protocol in [Protocol::Chat, Protocol::Responses] {
            errors.push(
                rrss_ai::complete(
                    &client,
                    &CompleteRequest {
                        base_url: server.uri(),
                        api_key: Some(format!(" {key} ")),
                        model: "local".into(),
                        system: "test".into(),
                        user: "test".into(),
                        json_schema: None,
                        protocol,
                    },
                )
                .await
                .unwrap_err(),
            );
        }
        errors.push(
            rrss_ai::embed(
                &client,
                &EmbedRequest {
                    base_url: server.uri(),
                    api_key: Some(format!(" {key} ")),
                    model: "local".into(),
                    input: "test".into(),
                },
            )
            .await
            .unwrap_err(),
        );
        for error in errors {
            assert!(matches!(error, Error::Http { status: 401, .. }));
            assert_eq!(error.log_message(), "http status 401");
            let text = format!("{error:?} {error}");
            assert!(!text.contains("private-") && !text.contains("canary"));
            assert!(text.contains("***"));
            if body.starts_with('{') {
                assert!(error.to_string().ends_with("\nretry later"));
            }
        }
    }
}

#[tokio::test]
async fn request_errors_keep_timeout_classification_without_url_or_source() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_millis(30))
        .build()
        .unwrap();
    let raw = client
        .get(format!(
            "http://private-user:private-pass@{}/",
            listener.local_addr().unwrap()
        ))
        .send()
        .await
        .unwrap_err();
    assert!(raw.is_timeout());
    let error = Error::from(raw);
    assert!(matches!(error, Error::Request { timeout: true, .. }));
    assert!(std::error::Error::source(&error).is_none());
    assert!(!format!("{error:?} {error}").contains("private-"));
}

#[tokio::test]
async fn proxy_auth_response_is_not_exposed_by_models_completions_or_embeddings() {
    let proxy = MockServer::start().await;
    Mock::given(any())
        .respond_with(
            ResponseTemplate::new(407).set_body_string(
                "proxy-user:proxy-password Proxy-Authorization: secret-encoded-auth",
            ),
        )
        .expect(3)
        .mount(&proxy)
        .await;
    let client = reqwest::Client::builder()
        .no_proxy()
        .proxy(reqwest::Proxy::all(proxy.uri()).unwrap())
        .build()
        .unwrap();
    let models = rrss_ai::list_models(&client, "http://target.invalid/v1", None)
        .await
        .unwrap_err();
    let complete = rrss_ai::complete(
        &client,
        &CompleteRequest {
            base_url: "http://target.invalid/v1".into(),
            api_key: None,
            model: "local".into(),
            system: "test".into(),
            user: "test".into(),
            json_schema: None,
            protocol: Protocol::Chat,
        },
    )
    .await
    .unwrap_err();
    let embed = rrss_ai::embed(
        &client,
        &EmbedRequest {
            base_url: "http://target.invalid/v1".into(),
            api_key: None,
            model: "local".into(),
            input: "test".into(),
        },
    )
    .await
    .unwrap_err();
    for error in [models, complete, embed] {
        assert!(matches!(error, Error::Http { status: 407, .. }));
        let text = format!("{error:?} {error}");
        assert!(!text.contains("proxy-user"));
        assert!(!text.contains("proxy-password"));
        assert!(!text.contains("secret-encoded-auth"));
    }
}
