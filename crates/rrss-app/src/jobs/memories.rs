use std::path::Path;

use anyhow::{Context, Result};
use rrss_ai::{episode_json_schema, facts_json_schema};
use rrss_core::ai_store::{TASK_EPISODE_COMPRESS, TASK_MEMORY_CANDIDATE};
use rrss_core::memories;
use serde_json::Value;

use super::complete::complete_task_checked;
use super::spawn_db;

pub async fn extract_memory_candidates(
    db_path: &Path,
    client: &reqwest::Client,
    user_text: &str,
) -> Result<Vec<String>> {
    let statements = memories::user_statements(user_text);
    if statements.is_empty() {
        return Ok(Vec::new());
    }
    let input = serde_json::to_string(&statements)?;
    let facts = complete_task_checked(
        db_path,
        client,
        TASK_MEMORY_CANDIDATE,
        &input,
        facts_json_schema(),
        false,
        |parsed| {
            let facts = parsed
                .get("facts")
                .and_then(Value::as_array)
                .context("memory JSON missing facts")?;
            Ok(facts
                .iter()
                .filter_map(Value::as_str)
                .filter(|fact| statements.contains(fact))
                .take(3)
                .map(str::to_owned)
                .collect::<Vec<_>>())
        },
    )
    .await?;
    if facts.is_empty() {
        return Ok(facts);
    }
    spawn_db(db_path.to_path_buf(), move |conn| {
        memories::insert_new_candidates(conn, facts)
    })
    .await
}

pub async fn compress_episode_if_needed(
    db_path: &Path,
    client: &reqwest::Client,
    thread_id: i64,
    settled_id: i64,
) -> Result<()> {
    let Some(batch) = spawn_db(db_path.to_path_buf(), move |conn| {
        memories::episode_batch(conn, thread_id, settled_id)
    })
    .await?
    else {
        return Ok(());
    };
    let input = serde_json::to_string(&batch.lines)?;
    let body = complete_task_checked(
        db_path,
        client,
        TASK_EPISODE_COMPRESS,
        &input,
        episode_json_schema(),
        false,
        |parsed| {
            parsed
                .get("body")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|body| !body.is_empty())
                .map(str::to_owned)
                .context("episode JSON missing body")
        },
    )
    .await?;
    spawn_db(db_path.to_path_buf(), move |conn| {
        memories::store_episode(conn, &batch, &body)
    })
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rrss_core::{ai_store, db, threads};
    use serde_json::json;
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, Request, ResponseTemplate};

    fn reply(value: Value) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{"message": {"content": value.to_string()}}],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1}
        }))
    }

    fn bind(conn: &rusqlite::Connection, server: &MockServer, task: &str) {
        let provider = crate::test_net::fake_provider(conn, "Memory fixture", server.uri());
        ai_store::set_task_binding(conn, task, Some(provider), Some("fixture")).unwrap();
    }

    fn supplied_input(request: &Request) -> String {
        let body: Value = request.body_json().unwrap();
        body["messages"][1]["content"].as_str().unwrap().to_owned()
    }

    #[tokio::test]
    async fn picks_only_verbatim_user_statements_and_allows_no_facts() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("candidates.db");
        let conn = db::open_at(&path).unwrap();
        let server = MockServer::start().await;
        bind(&conn, &server, TASK_MEMORY_CANDIDATE);
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let fact = "I always prefer offline reading.";
        let current = "Make this summary short today.";
        let quoted = "I need updates about this quoted topic every day.";
        let invented = "I prefer coding in the morning.";
        let mixed = format!("{current} {fact}\n\n> {quoted}\n\n```text\nQuoted passage\n```");
        Mock::given(method("POST"))
            .respond_with(move |request: &Request| {
                let input: Vec<String> = serde_json::from_str(&supplied_input(request)).unwrap();
                match input.as_slice() {
                    [only] if only == current => reply(json!({"facts": []})),
                    [first, second] if first == current && second == fact => {
                        reply(json!({"facts": [invented, quoted, fact]}))
                    }
                    _ => ResponseTemplate::new(400),
                }
            })
            .mount(&server)
            .await;
        assert!(
            extract_memory_candidates(&path, &client, current)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            extract_memory_candidates(&path, &client, &format!("> {quoted}"))
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            extract_memory_candidates(&path, &client, &mixed)
                .await
                .unwrap(),
            [fact]
        );
        assert!(
            extract_memory_candidates(&path, &client, &mixed)
                .await
                .unwrap()
                .is_empty()
        );
        let pending = &memories::list_memories(&conn, "").unwrap()[0];
        assert_eq!(pending.content, fact);
        assert!(pending.pending());
        memories::delete_memory(&conn, pending.id).unwrap();
        assert!(
            extract_memory_candidates(&path, &client, &mixed)
                .await
                .unwrap()
                .is_empty()
        );
        let cached: i64 = conn
            .query_row("SELECT count(*) FROM ai_cache", [], |row| row.get(0))
            .unwrap();
        assert_eq!(cached, 0);
    }

    #[tokio::test]
    async fn compression_retries_failed_batch_and_never_replays_deleted_episode() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("episodes.db");
        let conn = db::open_at(&path).unwrap();
        let server = MockServer::start().await;
        bind(&conn, &server, TASK_EPISODE_COMPRESS);
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let thread = threads::create_thread(&conn, None).unwrap().id;
        let fill = |range: std::ops::Range<i64>| {
            let mut latest = 0;
            for number in range {
                threads::insert_message(
                    &conn,
                    thread,
                    threads::MessageRole::User,
                    &format!("question {number}"),
                    None,
                )
                .unwrap();
                latest = threads::insert_message(
                    &conn,
                    thread,
                    threads::MessageRole::Assistant,
                    &format!("answer {number}"),
                    None,
                )
                .unwrap()
                .id;
            }
            latest
        };
        let first = fill(0..10);
        Mock::given(method("POST"))
            .respond_with(reply(json!({"body": " "})))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .respond_with(reply(json!({"body": "Conversation context"})))
            .mount(&server)
            .await;
        assert!(
            compress_episode_if_needed(&path, &client, thread, first)
                .await
                .is_err()
        );
        assert!(memories::list_memories(&conn, "").unwrap().is_empty());
        let later = fill(10..11);
        compress_episode_if_needed(&path, &client, thread, later)
            .await
            .unwrap();
        let episode = &memories::list_memories(&conn, "").unwrap()[0];
        assert_eq!(episode.scope, "thread");
        memories::delete_memory(&conn, episode.id).unwrap();
        compress_episode_if_needed(&path, &client, thread, later)
            .await
            .unwrap();
        assert!(memories::list_memories(&conn, "").unwrap().is_empty());
        let latest = fill(11..15);
        compress_episode_if_needed(&path, &client, thread, latest)
            .await
            .unwrap();
        let inputs: Vec<_> = server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .map(supplied_input)
            .collect();
        assert_eq!(inputs.len(), 3);
        assert_eq!(inputs[0], inputs[1]);
        let second: Vec<(String, String)> = serde_json::from_str(&inputs[2]).unwrap();
        assert_eq!(second.first().unwrap().1, "question 5");
        assert_eq!(second.last().unwrap().1, "answer 9");
    }
}
