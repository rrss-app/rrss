use std::path::Path;

use crate::scheduler::model_capacity::{Priority, RequestAdmission};
use anyhow::Result;
use rrss_ai::{CompleteRequest, complete::complete_admitted_until};
use rrss_core::Error as CoreError;
use rrss_core::ai_store::{self, ResolvedTask};
use serde_json::Value;
use tracing::{Instrument, instrument::WithSubscriber};

use super::spawn_db;

pub(crate) struct Prepared {
    pub resolved: ResolvedTask,
    pub provider: ai_store::AiProvider,
    pub cache_key: String,
    pub cached: Option<String>,
    pub write_detected_protocol: bool,
    pub auto: bool,
}

pub(crate) fn same_task(left: &ResolvedTask, right: &ResolvedTask) -> bool {
    left.task == right.task
        && left.model == right.model
        && left.template.id == right.template.id
        && left.template.version == right.template.version
        && left.template.body == right.template.body
        && left.provider.id == right.provider.id
        && left.provider.base_url == right.provider.base_url
        && left.provider.api_key == right.provider.api_key
        && (left.provider.protocol == right.provider.protocol
            || (left.provider.protocol == "auto"
                && matches!(right.provider.protocol.as_str(), "chat" | "responses")))
}

pub(crate) fn task_is_current(
    conn: &rusqlite::Connection,
    snapshot: &ResolvedTask,
) -> rrss_core::Result<bool> {
    match ai_store::resolve_task(conn, &snapshot.task) {
        Ok(current) => Ok(same_task(snapshot, &current)),
        Err(
            CoreError::ProviderNotFound
            | CoreError::TaskModelUnset(_)
            | CoreError::TemplateNotFound(_),
        ) => Ok(false),
        Err(error) => Err(error),
    }
}

pub(crate) fn prepare_resolved(
    conn: &rusqlite::Connection,
    resolved: ResolvedTask,
    user: &str,
    auto: bool,
) -> rrss_core::Result<Prepared> {
    if auto {
        ai_store::ensure_auto_job_budget(conn, ai_store::BudgetKind::Generation)?;
    }
    if resolved.provider.base_url.trim().is_empty() {
        return Err(CoreError::ProviderNotFound);
    }
    // Different providers/templates using the same model name must not share cache.
    let namespace = ai_store::hash_input(
        &serde_json::json!([
            resolved.task,
            resolved.provider.id,
            resolved.provider.base_url,
            resolved.template.id,
            resolved.template.body
        ])
        .to_string(),
    );
    let key = ai_store::cache_key(
        &namespace,
        &resolved.model,
        resolved.template.version,
        &ai_store::hash_input(user),
    );
    Ok(Prepared {
        provider: resolved.provider.clone(),
        resolved,
        cached: ai_store::cache_get(conn, &key)?,
        cache_key: key,
        write_detected_protocol: true,
        auto,
    })
}

pub(crate) async fn complete_task(
    db_path: &Path,
    client: &reqwest::Client,
    task: &str,
    user: &str,
    schema: Value,
    auto: bool,
) -> Result<Value> {
    complete_task_checked(db_path, client, task, user, schema, auto, |value| {
        Ok(value.clone())
    })
    .await
}

/// 只有通过 `check` 的输出才写入补全缓存；本地校验失败的缓存命中会被清掉，
/// 让显式重试真正重新请求模型。用量无论校验结果都照记。
pub(crate) async fn complete_task_checked<T>(
    db_path: &Path,
    client: &reqwest::Client,
    task: &str,
    user: &str,
    schema: Value,
    auto: bool,
    check: impl FnOnce(&Value) -> Result<T>,
) -> Result<T> {
    let path = db_path.to_path_buf();
    let task_owned = task.to_string();
    let user_owned = user.to_string();
    let prepared = spawn_db(path.clone(), move |conn| {
        if auto {
            ai_store::ensure_auto_job_budget(conn, ai_store::BudgetKind::Generation)?;
        }
        let resolved = ai_store::resolve_task(conn, &task_owned)?;
        prepare_resolved(conn, resolved, &user_owned, auto)
    })
    .await?;

    execute_prepared(db_path, client, prepared, user, schema, check).await
}

/// 用户正在等结果的请求插队：翻译，手动的本篇摘要（含分段）、相近报道综述和手动简报（含选题）。
/// 插队只排到后台请求前面，不打断在途请求；自动批次和对话后的记忆整理仍走后台。
fn request_priority(task: &str, auto: bool) -> Priority {
    let manual = !auto
        && matches!(
            task,
            ai_store::TASK_ARTICLE_SUMMARY
                | ai_store::TASK_ARTICLE_SEGMENT
                | ai_store::TASK_CLUSTER_SUMMARY
                | ai_store::TASK_DIGEST_DAILY
                | ai_store::TASK_DIGEST_WEEKLY
                | ai_store::TASK_DIGEST_PICK_DAILY
                | ai_store::TASK_DIGEST_PICK_WEEKLY
        );
    if manual || task == ai_store::TASK_ARTICLE_TRANSLATE {
        Priority::Interactive
    } else {
        Priority::Background
    }
}

/// 执行已固定的配置。翻译按整篇固定配置并自行隔离缓存命名空间；
/// 共用请求、输出校验与用量记录，不在每个窗重新解析绑定。
pub(crate) async fn execute_prepared<T>(
    db_path: &Path,
    client: &reqwest::Client,
    prepared: Prepared,
    user: &str,
    schema: Value,
    check: impl FnOnce(&Value) -> Result<T>,
) -> Result<T> {
    execute_prepared_with_policy(db_path, client, prepared, user, schema, |value| {
        check(value).map(|out| (out, true))
    })
    .await
}

/// 部分有效的翻译仍可上屏，但不缓存缺项响应；用量与本地校验成功与否无关。
pub(crate) async fn execute_prepared_with_policy<T>(
    db_path: &Path,
    client: &reqwest::Client,
    prepared: Prepared,
    user: &str,
    schema: Value,
    check: impl FnOnce(&Value) -> Result<(T, bool)>,
) -> Result<T> {
    let path = db_path.to_path_buf();

    let req = CompleteRequest {
        base_url: prepared.provider.base_url.clone(),
        api_key: prepared.provider.api_key.clone(),
        model: prepared.resolved.model.clone(),
        system: prepared.resolved.template.body.clone(),
        user: user.to_string(),
        json_schema: Some(schema),
        protocol: rrss_ai::Protocol::parse(&prepared.provider.protocol),
    };
    tracing::debug!(task = %prepared.resolved.task, model = %prepared.resolved.model, "executing ai complete task");
    let (usage_send, mut usage_receive) = tokio::sync::mpsc::unbounded_channel::<(u32, u32)>();
    let usage_path = path.clone();
    // 独立持有已收到的计数：网络总期限取消调用者时仍完成有界数据库写入。
    let settling = crate::scheduler::model_capacity::Capacity::for_database(db_path).work();
    let usage_task = tokio::spawn(
        async move {
            let _settling = settling;
            while let Some((prompt, completion)) = usage_receive.recv().await {
                let day = ai_store::local_day();
                spawn_db(usage_path.clone(), move |conn| {
                    ai_store::add_usage(conn, &day, i64::from(prompt), i64::from(completion))
                })
                .await?;
            }
            Ok::<_, anyhow::Error>(())
        }
        .in_current_span()
        .with_current_subscriber(),
    );
    let admission = RequestAdmission::new(
        db_path,
        &prepared.provider.base_url,
        request_priority(&prepared.resolved.task, prepared.auto),
        prepared.auto,
    )?;
    let response = complete_admitted_until(
        client,
        &req,
        prepared.cached.as_deref(),
        &|prompt, completion| {
            let _ = usage_send.send((prompt, completion));
        },
        tokio::time::Instant::now() + rrss_ai::deadline::ITEM_TIMEOUT,
        Some(&admission),
    )
    .await;
    drop(usage_send);
    usage_task.await??;
    let out = response.map_err(|err| {
        let failure = crate::ui_error::jobs::from_ai(&err);
        if matches!(err, rrss_ai::Error::BudgetExhausted | rrss_ai::Error::ServiceCooldown | rrss_ai::Error::Capacity | rrss_ai::Error::Admission(_)) {
            tracing::debug!(task = %prepared.resolved.task, code = failure.code(), detail = %failure.log_detail(), "ai complete task paused");
        } else {
            tracing::warn!(task = %prepared.resolved.task, code = failure.code(), detail = %failure.log_detail(), "ai complete task failed");
        }
        anyhow::anyhow!(err)
    })?;
    let checked = check(&out.parsed);
    let valid = checked.as_ref().is_ok_and(|(_, cacheable)| *cacheable);
    let current = spawn_db(path, move |conn| {
        crate::configuration::write(conn, |conn| {
            if prepared.write_detected_protocol && !task_is_current(conn, &prepared.resolved)? {
                return Ok(false);
            }
            if out.from_cache && !valid {
                ai_store::cache_delete(conn, &prepared.cache_key)?;
            } else if !out.from_cache && valid {
                ai_store::cache_put(conn, &prepared.cache_key, &out.content)?;
            }
            if let Some(detected) = out.detected_protocol
                && prepared.write_detected_protocol
            {
                ai_store::set_provider_protocol(conn, prepared.provider.id, &detected)?;
            }
            Ok(true)
        })
    })
    .await?;
    anyhow::ensure!(
        current,
        rrss_core::Error::Jobs(rrss_core::error::JobFailure::ConfigChanged)
    );
    checked.map(|(out, _)| out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rrss_core::db;
    use serde_json::json;
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn manual_summary_and_digest_jump_the_background_queue() {
        use ai_store::*;
        for (task, auto, interactive) in [
            (TASK_ARTICLE_TRANSLATE, false, true),
            (TASK_ARTICLE_SUMMARY, false, true),
            (TASK_ARTICLE_SEGMENT, false, true),
            (TASK_DIGEST_DAILY, false, true),
            (TASK_DIGEST_PICK_WEEKLY, false, true),
            (TASK_DIGEST_DAILY, true, false),
            (TASK_FOCUS_SUMMARY, true, false),
            (TASK_MEMORY_CANDIDATE, false, false),
            (TASK_EPISODE_COMPRESS, false, false),
        ] {
            let expected = if interactive {
                Priority::Interactive
            } else {
                Priority::Background
            };
            assert!(
                request_priority(task, auto) == expected,
                "{task} auto={auto}"
            );
        }
    }

    fn reply(content: &str) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{ "message": { "content": content } }],
            "usage": { "prompt_tokens": 1, "completion_tokens": 1 }
        }))
    }

    fn reject_bad(value: &Value) -> Result<String> {
        match value.get("v").and_then(Value::as_str) {
            Some("good") => Ok("good".into()),
            _ => anyhow::bail!("local validation failed"),
        }
    }

    #[tokio::test]
    async fn cancelling_a_repair_keeps_the_usage_already_received() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(reply("broken JSON"))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .respond_with(reply(r#"{"v":"good"}"#).set_delay(std::time::Duration::from_secs(60)))
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        let conn = db::open_at(&path).unwrap();
        let provider = crate::test_net::fake_provider(&conn, "mock", server.uri());
        ai_store::set_task_binding(
            &conn,
            ai_store::TASK_FOCUS_SUMMARY,
            Some(provider),
            Some("m"),
        )
        .unwrap();
        let operation = tokio::spawn({
            let path = path.clone();
            async move {
                complete_task(
                    &path,
                    &reqwest::Client::new(),
                    ai_store::TASK_FOCUS_SUMMARY,
                    "input",
                    json!({}),
                    true,
                )
                .await
            }
        });
        // 只设宽松上限、短间隔查询：共享 CI runner 很忙时，空转查询会和 mock 记录请求抢锁，
        // 两次请求加准入核对也可能超过几秒。断言的是行为，不是速度。
        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            while server.received_requests().await.unwrap().len() < 2 {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the repair request should reach the mock server");
        // A parent deadline drops the same future as this cancellation. Its usage
        // writer owns the received counters independently of the HTTP operation.
        operation.abort();
        assert!(operation.await.unwrap_err().is_cancelled());
        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            loop {
                let usage = ai_store::usage_on(&conn, &ai_store::local_day()).unwrap();
                if usage.prompt_tokens == 1 && usage.completion_tokens == 1 {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("usage received before cancellation should be recorded");
    }

    #[tokio::test]
    async fn preparation_skip_errors_keep_the_core_type_without_http() {
        for reason in ["budget", "provider", "model"] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("t.db");
            let conn = db::open_at(&path).unwrap();
            let task = ai_store::TASK_FOCUS_SUMMARY;
            if reason == "budget" {
                ai_store::set_daily_budget_tokens(&conn, 1).unwrap();
                ai_store::add_usage(&conn, &ai_store::local_day(), 1, 0).unwrap();
            } else if reason == "provider" {
                let pid = crate::test_net::fake_provider(&conn, "empty endpoint", String::new());
                ai_store::set_task_binding(&conn, task, Some(pid), Some("m")).unwrap();
            }
            let err = complete_task(
                &path,
                &reqwest::Client::new(),
                task,
                "input",
                json!({}),
                true,
            )
            .await
            .unwrap_err();
            let core = err.downcast_ref::<CoreError>().unwrap();
            assert!(matches!(
                (reason, core),
                ("budget", CoreError::DailyBudgetExhausted)
                    | ("provider", CoreError::ProviderNotFound)
                    | ("model", CoreError::TaskModelUnset(_))
            ));
            assert!(super::super::is_skip(&err.context("summary or memory job")));
        }
    }

    #[tokio::test]
    async fn rejected_output_is_not_cached_and_bad_hits_are_evicted() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(reply(r#"{"v":"bad"}"#))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .respond_with(reply(r#"{"v":"good"}"#))
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        let task = ai_store::TASK_DIGEST_DAILY;
        let key = {
            let conn = db::open_at(&path).unwrap();
            let pid = crate::test_net::fake_provider(&conn, "mock", server.uri());
            ai_store::set_task_binding(&conn, task, Some(pid), Some("m")).unwrap();
            let resolved = ai_store::resolve_task(&conn, task).unwrap();
            prepare_resolved(&conn, resolved, "input", false)
                .unwrap()
                .cache_key
        };
        let client = reqwest::Client::new();
        let run =
            || complete_task_checked(&path, &client, task, "input", json!({}), false, reject_bad);

        assert!(run().await.is_err());
        let conn = db::open_at(&path).unwrap();
        assert_eq!(ai_store::cache_get(&conn, &key).unwrap(), None);
        assert_eq!(run().await.unwrap(), "good");
        assert_eq!(server.received_requests().await.unwrap().len(), 2);

        ai_store::cache_put(&conn, &key, r#"{"v":"bad"}"#).unwrap();
        assert!(run().await.is_err());
        assert_eq!(ai_store::cache_get(&conn, &key).unwrap(), None);
        assert_eq!(run().await.unwrap(), "good");
        assert_eq!(server.received_requests().await.unwrap().len(), 3);
    }
}
