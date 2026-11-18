use super::*;
use rrss_core::{ai_store, threads::MessageRole};
use serde_json::json;
use wiremock::{Mock, MockServer, ResponseTemplate, matchers};

fn title_of(conn: &Connection, id: i64) -> Option<String> {
    threads::get_thread(conn, id).unwrap().title
}

fn question(conn: &Connection, id: i64, text: &str) {
    threads::insert_message(conn, id, MessageRole::User, text, None).unwrap();
}

fn open() -> (
    tempfile::TempDir,
    std::path::PathBuf,
    db::pool::PooledConnection,
) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("titles.db");
    let conn = db::open_at(&path).unwrap();
    (dir, path, conn)
}

fn enable(conn: &Connection, value: &str) {
    crate::settings_schema::set(conn, AUTO_TITLE_KEY, value).unwrap();
}

fn bind(conn: &Connection, url: String) -> ModelChoice {
    let provider_id = crate::test_net::fake_provider(conn, "Fixture", url);
    ai_store::set_task_binding(
        conn,
        TASK_CHAT_TITLE,
        Some(provider_id),
        Some("fixture-model"),
    )
    .unwrap();
    ModelChoice {
        provider_id,
        model: "fixture-model".into(),
    }
}

fn reply(conn: &Connection, path: &Path, id: i64) -> Option<TitleRequest> {
    save_successful_reply(conn, path, id, "Assistant's answer")
        .unwrap()
        .1
}

#[test]
fn second_committed_success_is_the_only_automatic_opportunity() {
    let (_dir, path, conn) = open();
    let id = threads::create_thread(&conn, Some("Local title"))
        .unwrap()
        .id;
    question(&conn, id, "First question");
    assert!(reply(&conn, &path, id).is_none());
    assert!(reply(&conn, &path, id).is_none());
    enable(&conn, "1");
    assert!(reply(&conn, &path, id).is_none());
    assert_eq!(title_of(&conn, id).as_deref(), Some("Local title"));

    let id = threads::create_thread(&conn, None).unwrap().id;
    question(&conn, id, "Second conversation");
    assert!(reply(&conn, &path, id).is_none());
    threads::insert_message(
        &conn,
        id,
        MessageRole::Assistant,
        "Partial failed reply",
        None,
    )
    .unwrap();
    conn.execute_batch(
        "CREATE TRIGGER reject_second BEFORE UPDATE OF title_successful_turns ON threads
        WHEN new.title_successful_turns = 2 BEGIN SELECT RAISE(ABORT, 'fixture rollback'); END;",
    )
    .unwrap();
    assert!(save_successful_reply(&conn, &path, id, "Rolled back").is_err());
    assert_eq!(threads::list_messages(&conn, id).unwrap().len(), 3);
    conn.execute_batch("DROP TRIGGER reject_second").unwrap();
    assert!(reply(&conn, &path, id).is_some());
    assert!(reply(&conn, &path, id).is_none());
}

#[test]
fn digest_legacy_and_manually_named_threads_skip_automatic_generation() {
    let (_dir, path, conn) = open();
    enable(&conn, "1");
    let digest = threads::get_or_create_anchored_thread(&conn, "Digest title", "digest", 7)
        .unwrap()
        .id;
    conn.execute(
        "INSERT INTO threads(id,title,created_at,updated_at) VALUES(123,'Legacy title',0,0)",
        [],
    )
    .unwrap();
    let renamed = threads::create_thread(&conn, None).unwrap().id;
    crate::actions::rename_thread(&conn, renamed, "").unwrap();
    for id in [digest, 123, renamed] {
        question(&conn, id, "A question");
        assert!(reply(&conn, &path, id).is_none());
        assert!(reply(&conn, &path, id).is_none());
    }
    assert_eq!(
        claim_manual(&conn, &path, digest).unwrap().err(),
        Some(ThreadTitleOutcome::NoQuestions)
    );
    assert!(claim_manual(&conn, &path, 123).unwrap().is_ok());
}

#[test]
fn requests_are_scoped_to_a_thread_and_rename_invalidates_writeback() {
    let (_dir, path, conn) = open();
    bind(&conn, "http://titles.example/v1".into());
    let config = crate::ai_language::resolve_task(&conn, TASK_CHAT_TITLE).unwrap();
    let other_conn = db::open_at(&path).unwrap();
    let id = threads::create_thread(&conn, Some("First title"))
        .unwrap()
        .id;
    let other = threads::create_thread(&conn, None).unwrap().id;
    question(&conn, id, "First question");
    question(&conn, other, "Other question");
    for title in ["First title", ""] {
        let request = claim_manual(&conn, &path, id).unwrap().unwrap();
        assert_eq!(
            claim_manual(&other_conn, &path, id).unwrap().err(),
            Some(ThreadTitleOutcome::Busy)
        );
        assert!(claim_manual(&other_conn, &path, other).unwrap().is_ok());
        crate::actions::rename_thread(&other_conn, id, title).unwrap();
        assert_eq!(
            request
                .authorization
                .apply(&conn, &config, "Late model title")
                .unwrap(),
            ThreadTitleOutcome::Stale
        );
        assert_eq!(
            title_of(&conn, id).as_deref(),
            (!title.is_empty()).then_some(title)
        );
    }
    let request = claim_manual(&conn, &path, other).unwrap().unwrap();
    conn.execute("DELETE FROM threads WHERE id = ?1", [other])
        .unwrap();
    let replacement = threads::create_thread(&conn, None).unwrap().id;
    assert_eq!(replacement, other);
    assert_eq!(
        request
            .authorization
            .apply(&conn, &config, "Late model title")
            .unwrap(),
        ThreadTitleOutcome::Stale
    );
    assert_eq!(title_of(&conn, replacement), None);
}

#[tokio::test]
async fn mock_http_only_sends_bounded_questions_and_validates_results() {
    let (_dir, path, conn) = open();
    let server = MockServer::start().await;
    let chosen = bind(&conn, server.uri());
    let network = NetworkClients::new(&path);
    let id = threads::create_thread(&conn, Some("First title"))
        .unwrap()
        .id;
    threads::insert_message(
        &conn,
        id,
        MessageRole::System,
        "Private system context",
        None,
    )
    .unwrap();
    question(&conn, id, &"界".repeat(300));
    threads::insert_message(
        &conn,
        id,
        MessageRole::Assistant,
        "Private assistant text",
        None,
    )
    .unwrap();
    question(&conn, id, "Another question");
    question(&conn, id, "Question outside the limit");

    let response = |content| {
        ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{"message": {"content": content}}],
            "usage": {"prompt_tokens": 14, "completion_tokens": 3}
        }))
    };
    Mock::given(matchers::method("POST"))
        .respond_with(response("broken JSON"))
        .expect(1)
        .mount(&server)
        .await;
    let request = claim_manual(&conn, &path, id).unwrap().unwrap();
    assert!(
        generate(&path, &network, &request, Some(chosen.clone()))
            .await
            .is_err()
    );
    drop(request);
    assert_eq!(title_of(&conn, id).as_deref(), Some("First title"));
    server.reset().await;

    Mock::given(matchers::method("POST"))
        .respond_with(response("{\"title\":\"  Garden planning  \"}"))
        .expect(1)
        .mount(&server)
        .await;
    let request = claim_manual(&conn, &path, id).unwrap().unwrap();
    assert_eq!(
        generate(&path, &network, &request, Some(chosen))
            .await
            .unwrap(),
        ThreadTitleOutcome::Applied
    );
    let requests = server.received_requests().await.unwrap();
    let payload: Value = requests[0].body_json().unwrap();
    let input = payload["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["role"] == "user")
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(input["content"].as_str().unwrap()).unwrap(),
        json!(["界".repeat(240), "Another question"])
    );
    assert_eq!(title_of(&conn, id).as_deref(), Some("Garden planning"));
}

#[test]
fn output_must_be_nonempty_one_line_and_at_most_eighty_characters() {
    for response in [
        json!({}),
        json!({"title": 3}),
        json!({"title": " "}),
        json!({"title": "first\nsecond"}),
        json!({"title": "first\u{2028}second"}),
        json!({"title": "题".repeat(81)}),
    ] {
        assert_eq!(validated_title(&response), None);
    }
    assert_eq!(
        validated_title(&json!({"title": "题".repeat(80)}))
            .unwrap()
            .chars()
            .count(),
        80
    );
}

#[tokio::test]
async fn missing_model_exhausted_budget_and_revoked_authorization_send_no_requests() {
    let (_dir, path, conn) = open();
    let server = MockServer::start().await;
    Mock::given(matchers::method("POST"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;
    let network = NetworkClients::new(&path);
    let id = threads::create_thread(&conn, Some("Local title"))
        .unwrap()
        .id;
    question(&conn, id, "First question");
    let manual = claim_manual(&conn, &path, id).unwrap().unwrap();
    assert_eq!(
        generate(&path, &network, &manual, None).await.unwrap(),
        ThreadTitleOutcome::NoModel
    );
    drop(manual);

    let model = bind(&conn, server.uri());
    let manual = claim_manual(&conn, &path, id).unwrap().unwrap();
    let expected = ModelChoice {
        provider_id: model.provider_id,
        model: "old-model".into(),
    };
    assert_eq!(
        generate(&path, &network, &manual, Some(expected))
            .await
            .unwrap(),
        ThreadTitleOutcome::ModelChanged
    );
    ai_store::set_task_binding(&conn, TASK_CHAT_TITLE, None, None).unwrap();
    ai_store::set_task_binding(
        &conn,
        TASK_CHAT_TITLE,
        Some(model.provider_id),
        Some(&model.model),
    )
    .unwrap();
    assert_eq!(
        generate(&path, &network, &manual, Some(model))
            .await
            .unwrap(),
        ThreadTitleOutcome::Stale
    );
    drop(manual);

    enable(&conn, "1");
    assert!(reply(&conn, &path, id).is_none());
    let automatic = reply(&conn, &path, id).unwrap();
    db::set_setting(&conn, ai_store::AI_DAILY_BUDGET_TOKENS_KEY, "1").unwrap();
    ai_store::add_usage(&conn, &ai_store::local_day(), 1, 0).unwrap();
    assert!(super::super::is_skip(
        &generate(&path, &network, &automatic, None)
            .await
            .unwrap_err()
    ));
    enable(&conn, "0");
    enable(&conn, "1");
    assert_eq!(
        generate(&path, &network, &automatic, None).await.unwrap(),
        ThreadTitleOutcome::Stale
    );
    assert_eq!(title_of(&conn, id).as_deref(), Some("Local title"));
}
