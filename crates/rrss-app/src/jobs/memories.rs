use std::collections::HashSet;
use std::path::Path;

use anyhow::{Context, Result};
use rrss_ai::{episode_json_schema, facts_json_schema};
use rrss_core::ai_store::{TASK_EPISODE_COMPRESS, TASK_MEMORY_CANDIDATE};
use rrss_core::memories::{
    self, EXTRACTION_EXISTING_LIMIT, ExistingMemorySummary, ExtractionInput, MemoryCandidate,
    MemoryKind,
};
use serde_json::Value;

use super::complete::complete_task_checked;
use super::spawn_db;

const MAX_CANDIDATES_PER_TURN: usize = 2;

/// Validate and normalize structured candidates returned by the memory
/// extraction model against the user's cleaned message and existing confirmed
/// memories. Shared by production extraction and `prompt_eval`.
pub fn parse_memory_candidates(
    parsed: &Value,
    clean_user_message: &str,
    existing: &[ExistingMemorySummary],
) -> Result<Vec<MemoryCandidate>> {
    let items = parsed
        .get("candidates")
        .and_then(Value::as_array)
        .context("memory JSON missing candidates")?;
    let confirmed_ids: HashSet<i64> = existing
        .iter()
        .filter(|m| !m.pending)
        .map(|m| m.id)
        .collect();
    let mut out = Vec::new();
    for item in items {
        let Some(action) = item.get("action").and_then(Value::as_str) else {
            continue;
        };
        let Some(kind_raw) = item.get("kind").and_then(Value::as_str) else {
            continue;
        };
        let kind = match kind_raw {
            "procedural" => MemoryKind::Procedural,
            "semantic" => MemoryKind::Semantic,
            _ => continue,
        };
        let Some(content) = item
            .get("text")
            .or_else(|| item.get("content"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            continue;
        };
        if rrss_core::logsafe::scrub_secrets(content) != content {
            continue;
        }
        let Some(evidence) = item.get("evidence").and_then(Value::as_str) else {
            continue;
        };
        if !memories::evidence_matches(clean_user_message, evidence) {
            continue;
        }
        let replaces_id = match action {
            "add" => None,
            "update" => {
                let id = item
                    .get("replaces_id")
                    .and_then(Value::as_i64)
                    .unwrap_or(0);
                if id <= 0 || !confirmed_ids.contains(&id) {
                    continue;
                }
                Some(id)
            }
            _ => continue,
        };
        out.push(MemoryCandidate {
            kind,
            content: content.to_owned(),
            replaces_id,
        });
        if out.len() >= MAX_CANDIDATES_PER_TURN {
            break;
        }
    }
    Ok(out)
}

pub async fn extract_memory_candidates(
    db_path: &Path,
    client: &reqwest::Client,
    thread_id: Option<i64>,
    settled_id: Option<i64>,
    user_text: &str,
) -> Result<Vec<String>> {
    let clean = memories::clean_user_message(user_text);
    if clean.is_empty() {
        return Ok(Vec::new());
    }
    let clean_check = clean.clone();
    let prep = spawn_db(db_path.to_path_buf(), move |conn| {
        let pending = memories::pending_count(conn)?;
        if !memories::should_extract_memory(&clean_check, pending) {
            return Ok(None);
        }
        let existing = memories::existing_for_extraction(conn, EXTRACTION_EXISTING_LIMIT)?;
        let previous = match (thread_id, settled_id) {
            (Some(tid), Some(sid)) => memories::previous_turn_context(conn, tid, sid)?,
            _ => None,
        };
        Ok(Some((existing, previous)))
    })
    .await?;
    let Some((existing, previous)) = prep else {
        return Ok(Vec::new());
    };
    let input = serde_json::to_string(&ExtractionInput {
        user_message: &clean,
        previous_turn: previous.as_ref(),
        existing_memories: &existing,
    })?;
    let candidates = complete_task_checked(
        db_path,
        client,
        TASK_MEMORY_CANDIDATE,
        &input,
        facts_json_schema(),
        false,
        |parsed| parse_memory_candidates(parsed, &clean, &existing),
    )
    .await?;
    if candidates.is_empty() {
        return Ok(Vec::new());
    }
    spawn_db(db_path.to_path_buf(), move |conn| {
        memories::insert_new_candidates(conn, candidates)
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
    async fn skips_ordinary_qa_without_calling_model_and_grounds_rewritten_rules() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("candidates.db");
        let conn = db::open_at(&path).unwrap();
        let server = MockServer::start().await;
        bind(&conn, &server, TASK_MEMORY_CANDIDATE);
        let client = reqwest::Client::builder().no_proxy().build().unwrap();

        let qa = "What does Wren 12 do when a tab exceeds its memory limit?";
        let quoted_only = "> I always prefer offline reading.";
        let mixed = "这段解释太学术了，你以后讲芯片架构都少堆术语、多打比方。\n\n> I always prefer dark mode.\n\n```text\nQuoted passage\n```";
        let generalized = "解释芯片或底层硬件概念时，少堆砌学术术语，优先用通俗类比。";

        Mock::given(method("POST"))
            .respond_with(move |request: &Request| {
                let payload: Value = serde_json::from_str(&supplied_input(request)).unwrap();
                assert!(
                    !payload["user_message"]
                        .as_str()
                        .unwrap()
                        .contains("dark mode")
                );
                reply(json!({
                    "candidates": [
                        {
                            "action": "add",
                            "evidence": "I always prefer dark mode",
                            "kind": "semantic",
                            "replaces_id": 0,
                            "text": "Prefers dark mode."
                        },
                        {
                            "action": "add",
                            "evidence": "你以后讲芯片架构都少堆术语、多打比方",
                            "kind": "procedural",
                            "replaces_id": 0,
                            "text": generalized
                        }
                    ]
                }))
            })
            .mount(&server)
            .await;

        assert!(
            extract_memory_candidates(&path, &client, None, None, qa)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            extract_memory_candidates(&path, &client, None, None, quoted_only)
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(server.received_requests().await.unwrap().len(), 0);

        assert_eq!(
            extract_memory_candidates(&path, &client, None, None, mixed)
                .await
                .unwrap(),
            [generalized]
        );
        // Same generalized content is deduplicated via persistent receipt.
        assert!(
            extract_memory_candidates(&path, &client, None, None, mixed)
                .await
                .unwrap()
                .is_empty()
        );
        let pending = &memories::list_memories(&conn, "").unwrap()[0];
        assert_eq!(pending.kind, MemoryKind::Procedural);
        assert_eq!(pending.content, generalized);
        assert!(pending.pending());

        memories::delete_memory(&conn, pending.id).unwrap();
        assert!(
            extract_memory_candidates(&path, &client, None, None, mixed)
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
    async fn sends_previous_turn_and_existing_memories_and_records_valid_updates() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("updates.db");
        let conn = db::open_at(&path).unwrap();
        let server = MockServer::start().await;
        bind(&conn, &server, TASK_MEMORY_CANDIDATE);
        let client = reqwest::Client::builder().no_proxy().build().unwrap();

        let existing_id = memories::insert_memory(
            &conn,
            MemoryKind::Semantic,
            "主要使用 Linux 桌面环境。",
            "chat",
            1.0,
        )
        .unwrap();
        let thread = threads::create_thread(&conn, None).unwrap().id;
        threads::insert_message(&conn, thread, threads::MessageRole::User, "怎么清理缓存？", None)
            .unwrap();
        threads::insert_message(
            &conn,
            thread,
            threads::MessageRole::Assistant,
            "可以在 bash 里运行 rm -rf ~/.cache/app",
            None,
        )
        .unwrap();
        let current_text = "我主力机换回 Windows 11 了，以后给命令别默认写 bash。";
        threads::insert_message(
            &conn,
            thread,
            threads::MessageRole::User,
            current_text,
            None,
        )
        .unwrap();
        let settled = threads::insert_message(
            &conn,
            thread,
            threads::MessageRole::Assistant,
            "明白，Windows 11 下可以用 PowerShell。",
            None,
        )
        .unwrap()
        .id;

        Mock::given(method("POST"))
            .respond_with(move |request: &Request| {
                let payload: Value = serde_json::from_str(&supplied_input(request)).unwrap();
                assert!(
                    payload["previous_turn"]["assistant"]
                        .as_str()
                        .unwrap()
                        .contains("bash")
                );
                assert_eq!(payload["existing_memories"][0]["id"], existing_id);
                reply(json!({
                    "candidates": [
                        {
                            "action": "update",
                            "evidence": "以后给命令别默认写 bash",
                            "kind": "procedural",
                            "replaces_id": 9999,
                            "text": "Should be dropped due to bogus target."
                        },
                        {
                            "action": "update",
                            "evidence": "主力机换回 Windows 11 了，以后给命令别默认写 bash",
                            "kind": "procedural",
                            "replaces_id": existing_id,
                            "text": "主力机使用 Windows 11，提供命令示例时优先考虑 Windows 环境而非默认 bash。"
                        }
                    ]
                }))
            })
            .mount(&server)
            .await;

        let added = extract_memory_candidates(
            &path,
            &client,
            Some(thread),
            Some(settled),
            current_text,
        )
        .await
        .unwrap();
        assert_eq!(added.len(), 1);

        let all = memories::list_memories(&conn, "").unwrap();
        assert_eq!(all.len(), 2);
        let pending = all.iter().find(|m| m.pending()).unwrap();
        assert_eq!(pending.replaces_id, Some(existing_id));
        assert_eq!(
            pending.replaces_content.as_deref(),
            Some("主要使用 Linux 桌面环境。")
        );
    }

    #[test]
    fn parser_drops_candidates_containing_credentials_even_when_grounded() {
        let user = "记住我的测试 API key 是 sk-demo-9876543210abcdef，以后帮我测接口方便点。";
        let parsed = json!({
            "candidates": [{
                "action": "add",
                "evidence": "记住我的测试 API key 是 sk-demo-9876543210abcdef",
                "kind": "semantic",
                "replaces_id": 0,
                "text": "测试 API key 是 sk-demo-9876543210abcdef。"
            }]
        });
        let kept = parse_memory_candidates(&parsed, user, &[]).unwrap();
        assert!(kept.is_empty());
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
