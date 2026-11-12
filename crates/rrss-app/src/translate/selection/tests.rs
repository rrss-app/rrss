use super::*;
use rrss_core::{ai_store, db};
use rusqlite::OptionalExtension;
use serde_json::json;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_string_contains, method, path},
};

fn setup(url: &str, complete: bool) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("selection.db");
    let conn = db::open_at(&path).unwrap();
    if complete {
        let id = ai_store::insert_provider(
            &conn,
            &ai_store::NewProvider {
                name: "Mock model".into(),
                base_url: url.into(),
                api_key: Some("private-key".into()),
                protocol: "auto".into(),
            },
        )
        .unwrap();
        ai_store::set_task_binding(
            &conn,
            ai_store::TASK_ARTICLE_TRANSLATE,
            Some(id),
            Some("model"),
        )
        .unwrap();
    } else {
        translations::set_backend(&conn, "http").unwrap();
        translations::set_http_config(&conn, Some(url), Some("private-key")).unwrap();
    }
    (dir, path)
}

fn identity(path: &Path) -> String {
    configuration(&db::open_at(path).unwrap())
        .unwrap()
        .unwrap()
        .identity
}

fn table(path: &Path, sql: &str) -> Vec<Vec<String>> {
    let conn = db::open_at(path).unwrap();
    let mut stmt = conn.prepare(sql).unwrap();
    let width = stmt.column_count();
    let mut rows = stmt.query([]).unwrap();
    let mut out = Vec::new();
    while let Some(row) = rows.next().unwrap() {
        out.push(
            (0..width)
                .map(|i| match row.get_ref(i).unwrap() {
                    rusqlite::types::ValueRef::Null => "null".into(),
                    rusqlite::types::ValueRef::Integer(v) => v.to_string(),
                    rusqlite::types::ValueRef::Real(v) => v.to_string(),
                    rusqlite::types::ValueRef::Text(v) => String::from_utf8_lossy(v).into_owned(),
                    rusqlite::types::ValueRef::Blob(v) => format!("blob{}", v.len()),
                })
                .collect(),
        );
    }
    out
}

fn stable_rows(path: &Path) -> Vec<Vec<Vec<String>>> {
    [
        "SELECT * FROM article_translations ORDER BY article_id, target_lang, engine",
        "SELECT * FROM ai_cache ORDER BY cache_key",
    ]
    .into_iter()
    .map(|sql| table(path, sql))
    .collect()
}

fn usage(path: &Path) -> (i64, i64) {
    let used = ai_store::usage_on(&db::open_at(path).unwrap(), &ai_store::local_day()).unwrap();
    (used.prompt_tokens, used.completion_tokens)
}

fn protocol(path: &Path) -> Option<String> {
    db::open_at(path)
        .unwrap()
        .query_row("SELECT protocol FROM ai_providers", [], |row| row.get(0))
        .optional()
        .unwrap()
}

fn translated_units(text: &str) -> serde_json::Value {
    json!({"choices": [{"message": {"content": format!("{{\"units\":[{{\"i\":0,\"t\":\"{text}\"}}]}}")}}], "usage": {"prompt_tokens": 5, "completion_tokens": 2}})
}

#[tokio::test]
async fn missing_configuration_has_the_full_translation_error_kind() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("missing.db");
    let conn = db::open_at(&path).unwrap();
    for backend in ["complete", "deepl", "http"] {
        translations::set_backend(&conn, backend).unwrap();
        if backend == "http" {
            translations::set_http_config(&conn, Some("not-a-url"), None).unwrap();
        }
        let err = translate_selection(&path, &reqwest::Client::new(), "Hello".into(), "".into())
            .await
            .unwrap_err();
        assert_eq!(err.kind, TranslationFailure::Configuration, "{backend}");
        assert_eq!(err.message, TranslationFailure::Configuration.message());
    }
}

#[tokio::test]
async fn overlong_and_changed_configuration_never_reach_the_service() {
    let server = MockServer::start().await;
    let (_dir, path) = setup(&server.uri(), false);
    let client = reqwest::Client::new();
    let old = identity(&path);
    let err = translate_selection(&path, &client, "字".repeat(MAX_CHARS + 1), old.clone())
        .await
        .unwrap_err();
    assert!(err.message.contains("全文翻译"));
    translations::set_target_lang(&db::open_at(&path).unwrap(), "en").unwrap();
    let err = translate_selection(&path, &client, "Hello".into(), old)
        .await
        .unwrap_err();
    assert_eq!(err.kind, TranslationFailure::Invalidated);
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn http_and_model_results_leave_translations_and_cache_unchanged() {
    for complete in [false, true] {
        let server = MockServer::start().await;
        let body = if complete {
            json!({"choices": [{"message": {"content": "{\"units\":[{\"i\":0,\"t\":\"你好\"}]}"}}], "usage": {"prompt_tokens": 5, "completion_tokens": 2}})
        } else {
            json!({"translatedText": "你好"})
        };
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&server)
            .await;
        let (_dir, path) = setup(&server.uri(), complete);
        let config = identity(&path);
        let before = stable_rows(&path);
        let out = translate_selection(
            &path,
            &reqwest::Client::new(),
            "Hello".into(),
            config.clone(),
        )
        .await
        .unwrap();
        assert_eq!(out.text, "你好");
        assert_eq!(out.config.identity, config);
        assert_eq!(
            out.config.service,
            if complete {
                "Mock model"
            } else {
                "HTTP 服务"
            }
        );
        assert_eq!(stable_rows(&path), before, "translations and AI cache");
        assert_eq!(
            usage(&path),
            if complete { (5, 2) } else { (0, 0) },
            "model usage is recorded; HTTP is not"
        );
        if complete {
            assert_eq!(protocol(&path).as_deref(), Some("auto"));
        }
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }
}

#[tokio::test]
async fn failures_are_classified_without_secrets_or_automatic_retries() {
    for complete in [false, true] {
        let cases: &[(u16, TranslationFailure)] = if complete {
            &[
                (401, TranslationFailure::Authentication),
                (429, TranslationFailure::RateLimited),
                (503, TranslationFailure::Network),
            ]
        } else {
            &[
                (401, TranslationFailure::Authentication),
                (429, TranslationFailure::RateLimited),
                (400, TranslationFailure::Configuration),
            ]
        };
        for &(status, kind) in cases {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .respond_with(
                    ResponseTemplate::new(status)
                        .set_body_string("private-key request rejected Hello secret translation"),
                )
                .mount(&server)
                .await;
            let (_dir, path) = setup(&server.uri(), complete);
            let before = stable_rows(&path);
            let err = translate_selection(
                &path,
                &reqwest::Client::new(),
                "Hello".into(),
                identity(&path),
            )
            .await
            .unwrap_err();
            assert_eq!(err.kind, kind);
            let serialized = serde_json::to_string(&err).unwrap();
            for secret in ["private-key", "Hello", "secret translation"] {
                assert!(!serialized.contains(secret));
            }
            assert_eq!(server.received_requests().await.unwrap().len(), 1);
            assert_eq!(stable_rows(&path), before);
            assert_eq!(usage(&path), (0, 0));
        }
    }
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"choices": [{"message": {"content": "not JSON"}}]})),
        )
        .mount(&server)
        .await;
    let (_dir, path) = setup(&server.uri(), true);
    let err = translate_selection(
        &path,
        &reqwest::Client::new(),
        "Hello".into(),
        identity(&path),
    )
    .await
    .unwrap_err();
    assert_eq!(err.kind, TranslationFailure::InvalidOutput);
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

#[tokio::test]
async fn manual_translation_bypasses_only_inferred_limits_and_still_pauses_background() {
    use crate::scheduler::model_capacity::{Priority, RequestAdmission};
    use rrss_ai::admission::Admission;
    for status in [429, 503] {
        for retry_after in [None, Some("600")] {
            let server = MockServer::start().await;
            let mut response = ResponseTemplate::new(status);
            if let Some(value) = retry_after {
                response = response.insert_header("Retry-After", value);
            }
            Mock::given(method("POST"))
                .respond_with(response)
                .up_to_n_times(1)
                .expect(1)
                .mount(&server)
                .await;
            Mock::given(method("POST"))
                .respond_with(ResponseTemplate::new(200).set_body_json(translated_units("你好")))
                .expect(if retry_after.is_some() { 0 } else { 1 })
                .mount(&server)
                .await;
            let (_dir, path) = setup(&server.uri(), true);
            let first = translate_selection(
                &path,
                &reqwest::Client::new(),
                "Hello".into(),
                identity(&path),
            )
            .await
            .unwrap_err();
            assert_eq!(
                first.kind,
                if status == 429 {
                    TranslationFailure::RateLimited
                } else {
                    TranslationFailure::Network
                }
            );
            let background =
                RequestAdmission::new(&path, &server.uri(), Priority::Background, false).unwrap();
            assert!(matches!(
                background.acquire().await,
                Err(rrss_ai::Error::ServiceCooldown)
            ));
            let retry = translate_selection(
                &path,
                &reqwest::Client::new(),
                "Hello".into(),
                identity(&path),
            )
            .await;
            if retry_after.is_some() {
                assert_eq!(retry.unwrap_err().kind, TranslationFailure::RateLimited);
            } else {
                assert_eq!(retry.unwrap().text, "你好");
            }
            assert!(matches!(
                background.acquire().await,
                Err(rrss_ai::Error::ServiceCooldown)
            ));
        }
    }
}

#[tokio::test]
async fn json_object_is_accepted_after_json_schema_is_rejected() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(body_string_contains("json_schema"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "error": { "message": "unsupported parameter response_format" }
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(body_string_contains("json_object"))
        .respond_with(ResponseTemplate::new(200).set_body_json(translated_units("你好")))
        .mount(&server)
        .await;
    let (_dir, path) = setup(&server.uri(), true);
    let before = stable_rows(&path);
    let out = translate_selection(
        &path,
        &reqwest::Client::new(),
        "Hello".into(),
        identity(&path),
    )
    .await
    .unwrap();
    assert_eq!(out.text, "你好");
    assert_eq!(stable_rows(&path), before);
    assert_eq!(protocol(&path).as_deref(), Some("auto"));
    assert_eq!(usage(&path), (5, 2));
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

#[tokio::test]
async fn auto_protocol_falls_back_from_chat_404_to_responses() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(404).set_body_string("not found"))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "output": [{
                "type": "message",
                "content": [{ "type": "output_text", "text": "{\"units\":[{\"i\":0,\"t\":\"你好\"}]}" }]
            }],
            "usage": { "input_tokens": 5, "output_tokens": 2 }
        })))
        .mount(&server)
        .await;
    let (_dir, path) = setup(&server.uri(), true);
    let before = stable_rows(&path);
    let out = translate_selection(
        &path,
        &reqwest::Client::new(),
        "Hello".into(),
        identity(&path),
    )
    .await
    .unwrap();
    assert_eq!(out.text, "你好");
    assert_eq!(stable_rows(&path), before);
    assert_eq!(protocol(&path).as_deref(), Some("auto"));
    assert_eq!(usage(&path), (5, 2));
    let paths: Vec<_> = server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .map(|req| req.url.path().to_string())
        .collect();
    assert!(paths.contains(&"/v1/chat/completions".to_string()));
    assert!(paths.contains(&"/v1/responses".to_string()));
}
