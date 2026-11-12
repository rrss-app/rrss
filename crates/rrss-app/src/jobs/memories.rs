use std::path::Path;

use anyhow::{Context, Result};
use rrss_ai::facts_json_schema;
use rrss_core::ai_store::{self, TASK_EPISODE_COMPRESS, TASK_MEMORY_CANDIDATE};
use rrss_core::memories::{self, MemoryKind};
use serde_json::Value;

use super::complete::complete_task;
use super::spawn_db;

pub async fn extract_memory_candidates(
    db_path: &Path,
    client: &reqwest::Client,
    transcript: String,
) -> Result<Vec<String>> {
    let parsed = complete_task(
        db_path,
        client,
        TASK_MEMORY_CANDIDATE,
        &transcript,
        facts_json_schema(),
        false,
    )
    .await?;
    let facts = parsed
        .get("facts")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut out = Vec::new();
    for fact in facts.into_iter().take(3) {
        let Some(text) = fact.as_str().map(str::trim).filter(|s| !s.is_empty()) else {
            continue;
        };
        out.push(text.to_string());
    }
    if !out.is_empty() {
        let path = db_path.to_path_buf();
        let stored = out.clone();
        spawn_db(path, move |conn| {
            for fact in &stored {
                memories::insert_pending_fact(&conn, fact)?;
            }
            Ok(())
        })
        .await?;
    }
    Ok(out)
}

pub async fn compress_episode_if_needed(
    db_path: &Path,
    client: &reqwest::Client,
    thread_id: i64,
) -> Result<()> {
    let path = db_path.to_path_buf();
    let packed = spawn_db(path, move |conn| {
        let msgs = rrss_core::threads::list_messages(&conn, thread_id)?;
        if msgs.len() <= 20 {
            return Ok(None);
        }
        let keep = 10;
        let older = &msgs[..msgs.len() - keep];
        let mut body = String::new();
        for m in older {
            body.push_str(m.role.as_str());
            body.push_str(": ");
            body.push_str(&ai_store::truncate_chars(&m.content, 200));
            body.push('\n');
        }
        Ok(Some(body))
    })
    .await?;
    let Some(input) = packed else {
        return Ok(());
    };
    let parsed = complete_task(
        db_path,
        client,
        TASK_EPISODE_COMPRESS,
        &input,
        facts_json_schema(),
        false,
    )
    .await?;
    let body = parsed
        .get("body")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
        .context("episode JSON missing body")?;
    let path = db_path.to_path_buf();
    spawn_db(path, move |conn| {
        memories::insert_memory(&conn, MemoryKind::Episode, &body, "compress", 1.0)?;
        Ok(())
    })
    .await?;
    Ok(())
}
