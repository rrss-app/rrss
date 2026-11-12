use std::path::Path;

use anyhow::{Context, Result};
use rrss_ai::{cite_json_schema, segment_json_schema, summary_json_schema};
use rrss_core::ai_store::{
    self, TASK_ARTICLE_SEGMENT, TASK_ARTICLE_SUMMARY, TASK_CITE_SUMMARIZE, TASK_FOCUS_SUMMARY,
};
use rrss_core::articles::{self, DueSummary};
use rrss_core::summary_extract;
use serde_json::Value;

use super::complete::{complete_task, execute_prepared, prepare_resolved, task_is_current};
use super::{FOCUS_BATCH, is_skip, spawn_db};

/// 单篇摘要最多保留的要点与关键事实条数，与模板里的要求一致。
const MAX_KEY_POINTS: usize = 3;
const MAX_FACTS: usize = 5;

pub async fn run_focus_summaries(db_path: &Path, client: &reqwest::Client) -> Result<()> {
    let path = db_path.to_path_buf();
    let capacity = crate::scheduler::model_capacity::Capacity::for_database(db_path);
    let batch = capacity.object("focus-summaries".into());
    let _batch_owner = batch.lock().await;
    let retries = capacity.clone();
    let jobs = spawn_db(path.clone(), move |conn| {
        let slots = ai_store::focus_summary_slots(conn)?.min(FOCUS_BATCH);
        if slots == 0 {
            return Ok(Vec::new());
        }
        let task = ai_store::resolve_task(conn, TASK_FOCUS_SUMMARY)?;
        let context = ai_store::hash_input(
            &serde_json::json!([
                task.model,
                task.provider.id,
                task.provider.base_url,
                task.provider.api_key,
                task.template.body,
                task.template.version,
                ai_store::input_tier(conn)?.as_str()
            ])
            .to_string(),
        );
        let excluded = {
            let mut retries = retries.retries.lock().unwrap();
            retries.context(super::retry::Kind::Summary, &context);
            retries.excluded(super::retry::Kind::Summary)
        };
        articles::due_focus_summaries_excluding(conn, slots, &excluded)
    })
    .await?;
    capacity
        .summaries
        .send_replace(Some(jobs.iter().map(|job| job.id).collect()));
    if jobs.is_empty() {
        return Ok(());
    }
    let mut failed = None;
    for job in jobs {
        let result = summarize_job(db_path, client, TASK_FOCUS_SUMMARY, &job, true, false).await;
        capacity.summaries.send_modify(|pending| {
            if let Some(pending) = pending {
                pending.remove(&job.id);
            }
        });
        match result {
            Ok(_) => capacity
                .retries
                .lock()
                .unwrap()
                .success(super::retry::Kind::Summary, job.id),
            Err(err) if is_skip(&err) => {
                tracing::debug!(error = %crate::log::error(err.as_ref()), "skip remaining auto summaries");
                return Err(err);
            }
            Err(err) => {
                if !super::retry::input_failure(&err) {
                    return Err(err);
                }
                capacity.retries.lock().unwrap().failed(
                    super::retry::Kind::Summary,
                    job.id,
                    job.hash.clone(),
                    true,
                );
                tracing::warn!(article_id = job.id, error = %crate::log::error(err.as_ref()), "focus summary failed");
                failed.get_or_insert(err);
            }
        }
    }
    failed.map_or(Ok(()), Err)
}

pub async fn summarize_article(
    db_path: &Path,
    client: &reqwest::Client,
    id: i64,
) -> Result<String> {
    let path = db_path.to_path_buf();
    let job = spawn_db(path, move |conn| {
        let row = articles::get_article(conn, id)?;
        Ok(DueSummary {
            hash: row.hash.clone().unwrap_or_default(),
            id: row.id,
            title: row.title,
            content_html: row.content_html,
            blocks_json: row.blocks_json,
            score: row.score,
            starred: row.starred,
        })
    })
    .await?;
    summarize_job(db_path, client, TASK_ARTICLE_SUMMARY, &job, false, true).await
}

pub async fn summarize_citations(
    db_path: &Path,
    client: &reqwest::Client,
    payload: String,
) -> Result<String> {
    let parsed = complete_task(
        db_path,
        client,
        TASK_CITE_SUMMARIZE,
        &payload,
        cite_json_schema(),
        false,
    )
    .await?;
    parsed
        .get("summary")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
        .context("cite summary JSON missing summary")
}

pub(crate) async fn summarize_job(
    db_path: &Path,
    client: &reqwest::Client,
    task: &str,
    job: &DueSummary,
    auto: bool,
    segmented: bool,
) -> Result<String> {
    // Different templates never share results, but writes to one article are
    // serialized. Re-read under ownership so an auto candidate cannot replace
    // a manual summary completed while it waited.
    let capacity = crate::scheduler::model_capacity::Capacity::for_database(db_path);
    let object = capacity.object(format!("summary:{}", job.id));
    let _owner = object.lock().await;
    if auto {
        let id = job.id;
        let missing = spawn_db(db_path.to_path_buf(), move |conn| {
            if ai_store::focus_summary_slots(conn)? == 0 {
                return Ok(false);
            }
            Ok(conn.query_row("SELECT summary IS NULL OR summary='' OR summary_hash IS NULL OR summary_hash!=hash FROM articles WHERE id=?1", [id], |r| r.get::<_,bool>(0))?)
        }).await?;
        if !missing {
            return Ok(String::new());
        }
    }
    tokio::time::timeout(
        rrss_ai::deadline::ITEM_TIMEOUT,
        summarize_job_inner(db_path, client, task, job, auto, segmented),
    )
    .await
    .map_err(|_| rrss_ai::deadline::timeout_error())?
}

async fn summarize_job_inner(
    db_path: &Path,
    client: &reqwest::Client,
    task: &str,
    job: &DueSummary,
    auto: bool,
    segmented: bool,
) -> Result<String> {
    let high_priority = job.score >= 10 || job.starred;
    let task_name = task.to_owned();
    let id = job.id;
    let (before, tier, resolved) = spawn_db(db_path.to_path_buf(), move |conn| {
        Ok((
            articles::get_article(conn, id)?,
            ai_store::input_tier(conn)?,
            ai_store::resolve_task(conn, &task_name)?,
        ))
    })
    .await?;
    anyhow::ensure!(
        before.title == job.title
            && before.blocks_json == job.blocks_json
            && before.content_html == job.content_html,
        "正文已改变，请重新生成摘要"
    );
    // 只有手动「本篇摘要」、且档位不是「低」时，放不下的长文才先分段压缩。
    let segments = (segmented && tier != ai_store::InputTier::Low)
        .then(|| {
            summary_extract::split_segments(
                job.blocks_json.as_deref(),
                job.content_html.as_deref(),
                tier.summary_budget(high_priority),
            )
        })
        .flatten();
    let segment_task = if segments.is_some() {
        Some(
            spawn_db(db_path.to_path_buf(), |conn| {
                ai_store::resolve_task(conn, TASK_ARTICLE_SEGMENT)
            })
            .await?,
        )
    } else {
        None
    };
    let payload = match segments {
        Some(segments) => {
            segment_notes_payload(
                db_path,
                client,
                &job.title,
                segments,
                segment_task.as_ref().unwrap(),
            )
            .await?
        }
        None => ai_store::summary_payload(
            job.title.as_str(),
            job.blocks_json.as_deref(),
            job.content_html.as_deref(),
            high_priority,
            tier,
        ),
    };
    let snapshot = resolved.clone();
    let input = payload.clone();
    let prepared = spawn_db(db_path.to_path_buf(), move |conn| {
        prepare_resolved(conn, snapshot, &input, auto)
    })
    .await?;
    let mut parsed = execute_prepared(
        db_path,
        client,
        prepared,
        &payload,
        summary_json_schema(),
        |value| Ok(value.clone()),
    )
    .await?;
    if let Some(obj) = parsed.as_object_mut() {
        // 条数上限只写在提示词里，模型多写的在这里截掉，免得原样进库和简报。
        for (key, max) in [("key_points", MAX_KEY_POINTS), ("facts", MAX_FACTS)] {
            if let Some(Value::Array(list)) = obj.get_mut(key) {
                list.truncate(max);
            }
        }
        // 记下这次摘要依据的正文，抓回全文后阅读器据此提示摘要来自旧正文。
        obj.insert(
            summary_extract::BODY_SIG_KEY.into(),
            Value::String(summary_extract::body_signature(
                job.blocks_json.as_deref(),
                job.content_html.as_deref(),
            )),
        );
    }
    let summary = parsed
        .get("summary")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
        .ok_or_else(|| rrss_ai::Error::InvalidJson("summary JSON missing summary".into()))?;
    let id = job.id;
    let text = summary.clone();
    let raw_json = serde_json::to_string(&parsed).unwrap_or_default();
    let path = db_path.to_path_buf();
    let saved = spawn_db(path, move |conn| {
        rrss_core::db::atomic_write(conn, |conn| {
            let current = match articles::get_article(conn, id) {
                Ok(row) => row,
                Err(rrss_core::Error::ArticleNotFound) => return Ok(false),
                Err(error) => return Err(error),
            };
            if current.title != before.title
                || current.blocks_json != before.blocks_json
                || current.content_html != before.content_html
                || current.summary != before.summary
                || current.summary_json != before.summary_json
                || ai_store::input_tier(conn)? != tier
                || !task_is_current(conn, &resolved)?
            {
                return Ok(false);
            }
            if let Some(segment) = segment_task
                && !task_is_current(conn, &segment)?
            {
                return Ok(false);
            }
            if auto && ai_store::focus_summary_slots(conn)? == 0 {
                return Ok(false);
            }
            articles::set_summary_and_json(conn, id, &text, Some(&raw_json))?;
            if auto {
                conn.execute(
                    "INSERT INTO ai_usage(day,auto_summaries) VALUES(?1,1)
                    ON CONFLICT(day) DO UPDATE SET auto_summaries=auto_summaries+1",
                    [ai_store::local_day()],
                )?;
            }
            Ok(true)
        })
    })
    .await?;
    anyhow::ensure!(saved, "正文、摘要或模型配置已改变，已丢弃旧摘要");
    Ok(summary)
}

/// 长文先逐段压缩（并发请求），再把按原文顺序排好的笔记交给本篇摘要汇总。任何一段失败则整次失败，旧摘要保留。
async fn segment_notes_payload(
    db_path: &Path,
    client: &reqwest::Client,
    title: &str,
    segments: Vec<String>,
    resolved: &ai_store::ResolvedTask,
) -> Result<String> {
    let total = segments.len();
    let mut set = tokio::task::JoinSet::new();
    for (i, segment) in segments.into_iter().enumerate() {
        let path = db_path.to_path_buf();
        let client = client.clone();
        let resolved = resolved.clone();
        let user = format!("{title}\n\n（第 {} / {total} 部分）\n\n{segment}", i + 1);
        set.spawn(async move {
            let input = user.clone();
            let prepared = spawn_db(path.clone(), move |conn| {
                prepare_resolved(conn, resolved, &input, false)
            })
            .await?;
            let parsed = execute_prepared(
                &path,
                &client,
                prepared,
                &user,
                segment_json_schema(),
                |value| Ok(value.clone()),
            )
            .await?;
            anyhow::Ok((i, parsed))
        });
    }
    let mut notes = vec![Value::Null; total];
    while let Some(joined) = set.join_next().await {
        let (i, parsed) = joined.context("segment task panicked")??;
        notes[i] = parsed;
    }
    Ok(format_segment_notes(title, &notes))
}

fn format_segment_notes(title: &str, notes: &[Value]) -> String {
    let mut out = format!(
        "{title}\n\n以下是全文按原文顺序分 {} 部分整理的笔记：",
        notes.len()
    );
    for (i, note) in notes.iter().enumerate() {
        let text = note
            .get("notes")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("");
        out.push_str(&format!("\n\n【第 {} 部分】{text}", i + 1));
        let facts = note
            .get("facts")
            .and_then(Value::as_array)
            .into_iter()
            .flatten();
        for fact in facts
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter(|f| !f.is_empty())
        {
            out.push_str(&format!("\n- {fact}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use rrss_core::db;
    use serde_json::json;
    use wiremock::matchers::{body_string_contains, method};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn reply(content: Value) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{ "message": { "content": content.to_string() } }],
            "usage": { "prompt_tokens": 1, "completion_tokens": 1 }
        }))
    }

    /// 一次性库：模拟模型服务、摘要相关任务都绑到它，文章 7 是一篇远超中档预算的长文，已有旧摘要。
    fn long_article_db(base_url: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        let conn = db::open_at(&path).unwrap();
        let pid = ai_store::insert_provider(
            &conn,
            &ai_store::NewProvider {
                name: "mock".into(),
                base_url: base_url.into(),
                api_key: None,
                protocol: "openai".into(),
            },
        )
        .unwrap();
        for task in [
            TASK_ARTICLE_SUMMARY,
            TASK_ARTICLE_SEGMENT,
            TASK_FOCUS_SUMMARY,
        ] {
            ai_store::set_task_binding(&conn, task, Some(pid), Some("m")).unwrap();
        }
        conn.execute(
            "INSERT INTO feeds (id, url, created_at) VALUES (1, 'https://f.test/rss', 1)",
            [],
        )
        .unwrap();
        let body: String = (0..40)
            .map(|i| format!("<p>第 {i} 段。{}</p>", "论述内容".repeat(60)))
            .collect();
        conn.execute(
            "INSERT INTO articles (id, feed_id, title, hash, created_at, content_html, score, summary) VALUES (7, 1, '长文', 'h', 1, ?1, 5, '旧摘要')",
            [&body],
        )
        .unwrap();
        conn.execute(
            "UPDATE articles SET created_at=?1",
            [rrss_core::feeds::now_ms()],
        )
        .unwrap();
        (dir, path)
    }

    fn stored_summary(path: &std::path::Path) -> (Option<String>, Option<String>) {
        db::open_at(path)
            .unwrap()
            .query_row(
                "SELECT summary, summary_json FROM articles WHERE id = 7",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap()
    }

    #[tokio::test]
    async fn disabled_auto_summary_never_requests_even_with_a_model() {
        let server = MockServer::start().await;
        let (_dir, path) = long_article_db(&server.uri());
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        run_focus_summaries(&path, &client).await.unwrap();
        assert!(server.received_requests().await.unwrap().is_empty());
        assert_eq!(stored_summary(&path).0.as_deref(), Some("旧摘要"));
    }

    #[tokio::test]
    async fn auto_summary_daily_cap_counts_only_saved_results_including_neutrals() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(|request: &wiremock::Request| {
                if String::from_utf8_lossy(&request.body).contains("bad-summary") {
                    reply(json!({}))
                } else {
                    reply(json!({"summary": "saved"}))
                }
            })
            .mount(&server)
            .await;
        let (_dir, path) = long_article_db(&server.uri());
        let _capacity = crate::scheduler::model_capacity::Capacity::for_database(&path);
        let conn = db::open_at(&path).unwrap();
        db::set_setting(&conn, ai_store::FOCUS_AUTO_SUMMARY.id, "1").unwrap();
        articles::set_focus_summary_neutral_limit(&conn, 10).unwrap();
        conn.execute(
            "INSERT INTO ai_usage(day,auto_summaries) VALUES(?1,98)",
            [ai_store::local_day()],
        )
        .unwrap();
        for (id, title, score) in [(8, "bad-summary", 20), (9, "neutral", 0)] {
            conn.execute(
                "INSERT INTO articles(id,feed_id,title,hash,score,created_at)
                VALUES(?1,1,?2,?2,?3,?4)",
                rusqlite::params![id, title, score, rrss_core::feeds::now_ms()],
            )
            .unwrap();
        }
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        assert!(run_focus_summaries(&path, &client).await.is_err());
        assert_eq!(
            ai_store::usage_on(&conn, &ai_store::local_day())
                .unwrap()
                .auto_summaries,
            99
        );
        run_focus_summaries(&path, &client).await.unwrap();
        assert_eq!(
            ai_store::usage_on(&conn, &ai_store::local_day())
                .unwrap()
                .auto_summaries,
            100
        );
        assert_eq!(
            articles::get_article(&conn, 9).unwrap().summary.as_deref(),
            Some("saved")
        );
        let requests = server.received_requests().await.unwrap().len();
        run_focus_summaries(&path, &client).await.unwrap();
        assert_eq!(server.received_requests().await.unwrap().len(), requests);
        // 过去本地日的篇数不占今天名额；不需要真实等待午夜。
        conn.execute("UPDATE ai_usage SET day='2000-01-01'", [])
            .unwrap();
        conn.execute(
            "UPDATE articles SET hash='next-day',summary=NULL WHERE id=9",
            [],
        )
        .unwrap();
        run_focus_summaries(&path, &client).await.unwrap();
        assert_eq!(
            ai_store::usage_on(&conn, &ai_store::local_day())
                .unwrap()
                .auto_summaries,
            1
        );
    }

    #[tokio::test]
    async fn cooling_bad_summaries_leave_batch_slots_for_healthy_articles() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(|request: &wiremock::Request| {
                if String::from_utf8_lossy(&request.body).contains("bad-summary") {
                    reply(json!({}))
                } else {
                    reply(json!({"summary": "healthy summary"}))
                }
            })
            .mount(&server)
            .await;
        let (_dir, path) = long_article_db(&server.uri());
        let capacity = crate::scheduler::model_capacity::Capacity::for_database(&path);
        let conn = db::open_at(&path).unwrap();
        db::set_setting(&conn, ai_store::FOCUS_AUTO_SUMMARY.id, "1").unwrap();
        conn.execute("UPDATE articles SET score=1,summary=NULL WHERE id=7", [])
            .unwrap();
        for id in 100..100 + FOCUS_BATCH {
            conn.execute(
                "INSERT INTO articles(id,feed_id,title,hash,score,created_at)
                VALUES(?1,1,'bad-summary',?2,10,?3)",
                rusqlite::params![id, format!("hash-{id}"), rrss_core::feeds::now_ms()],
            )
            .unwrap();
        }
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        assert!(run_focus_summaries(&path, &client).await.is_err());
        assert!(stored_summary(&path).0.is_none());
        run_focus_summaries(&path, &client).await.unwrap();
        assert_eq!(stored_summary(&path).0.as_deref(), Some("healthy summary"));
        assert_eq!(
            capacity
                .retries
                .lock()
                .unwrap()
                .excluded(super::super::retry::Kind::Summary)
                .len(),
            FOCUS_BATCH as usize
        );
    }

    #[tokio::test]
    async fn stale_body_summary_or_binding_never_overwrites_and_known_usage_is_retained() {
        for change in ["body", "summary", "binding", "delete"] {
            let server = MockServer::start().await;
            let (_dir, path) = long_article_db(&server.uri());
            ai_store::set_input_tier(&db::open_at(&path).unwrap(), ai_store::InputTier::Low)
                .unwrap();
            let changed = path.clone();
            Mock::given(method("POST"))
                .respond_with(move |_: &wiremock::Request| {
                    let conn = db::open_at(&changed).unwrap();
                    match change {
                        "body" => {
                            conn.execute(
                                "UPDATE articles SET content_html='new body' WHERE id=7",
                                [],
                            )
                            .unwrap();
                        }
                        "summary" => {
                            articles::set_summary(&conn, 7, "new summary").unwrap();
                        }
                        "binding" => {
                            conn.execute(
                                "UPDATE task_bindings SET model='changed' WHERE task=?1",
                                [TASK_ARTICLE_SUMMARY],
                            )
                            .unwrap();
                        }
                        "delete" => {
                            conn.execute("DELETE FROM articles WHERE id=7", []).unwrap();
                        }
                        _ => unreachable!(),
                    }
                    reply(json!({"summary":"late"}))
                })
                .mount(&server)
                .await;
            assert!(
                summarize_article(&path, &reqwest::Client::new(), 7)
                    .await
                    .is_err()
            );
            let conn = db::open_at(&path).unwrap();
            let late: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM articles WHERE summary='late'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(late, 0);
            if change == "binding" {
                let cached: i64 = conn
                    .query_row("SELECT COUNT(*) FROM ai_cache", [], |r| r.get(0))
                    .unwrap();
                assert_eq!(cached, 0, "obsolete configuration must not populate cache");
            }
            let tokens: i64 = conn
                .query_row("SELECT SUM(tokens) FROM ai_usage", [], |r| r.get(0))
                .unwrap();
            assert_eq!(tokens, 2);
        }
    }

    #[tokio::test]
    async fn failed_segment_fails_the_whole_summary_and_keeps_the_old_one() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(body_string_contains("其中一部分"))
            .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .respond_with(reply(
                json!({"summary": "不该写入。", "one_liner": "x", "key_points": [], "facts": []}),
            ))
            .mount(&server)
            .await;
        let (_dir, path) = long_article_db(&server.uri());
        assert!(
            summarize_article(&path, &reqwest::Client::new(), 7)
                .await
                .is_err()
        );
        assert_eq!(stored_summary(&path), (Some("旧摘要".into()), None));
        let bodies = server.received_requests().await.unwrap();
        assert!(
            bodies
                .iter()
                .all(|r| String::from_utf8_lossy(&r.body).contains("其中一部分"))
        );
    }

    #[tokio::test]
    async fn claimed_auto_candidate_does_not_replace_a_completed_manual_summary() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(reply(json!({"summary":"manual"})))
            .mount(&server)
            .await;
        let (_dir, path) = long_article_db(&server.uri());
        let conn = db::open_at(&path).unwrap();
        ai_store::set_input_tier(&conn, ai_store::InputTier::Low).unwrap();
        db::set_setting(&conn, ai_store::FOCUS_AUTO_SUMMARY.id, "1").unwrap();
        let candidate = articles::due_focus_summaries(&conn, 1).unwrap().remove(0);
        summarize_article(&path, &reqwest::Client::new(), 7)
            .await
            .unwrap();
        summarize_job(
            &path,
            &reqwest::Client::new(),
            TASK_FOCUS_SUMMARY,
            &candidate,
            true,
            false,
        )
        .await
        .unwrap();
        assert_eq!(stored_summary(&path).0.as_deref(), Some("manual"));
        assert_eq!(
            ai_store::usage_on(&conn, &ai_store::local_day())
                .unwrap()
                .auto_summaries,
            0
        );
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn extra_key_points_and_facts_are_dropped() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(reply(json!({
                "summary": "摘要。", "one_liner": "一句",
                "key_points": ["1", "2", "3", "4", "5"],
                "facts": ["a", "b", "c", "d", "e", "f", "g"]
            })))
            .mount(&server)
            .await;
        let (_dir, path) = long_article_db(&server.uri());
        {
            let conn = db::open_at(&path).unwrap();
            ai_store::set_input_tier(&conn, ai_store::InputTier::Low).unwrap();
        }
        summarize_article(&path, &reqwest::Client::new(), 7)
            .await
            .unwrap();
        let raw = stored_summary(&path).1.expect("summary_json");
        let v: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["key_points"], json!(["1", "2", "3"]));
        assert_eq!(v["facts"], json!(["a", "b", "c", "d", "e"]));
        assert!(v[summary_extract::BODY_SIG_KEY].is_string());
    }

    /// 一篇远超中档预算的长文：手动摘要先分段压缩再汇总；低档或自动摘要只请求一次。
    #[tokio::test]
    async fn manual_summary_of_long_article_compresses_segments_first() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(body_string_contains("其中一部分"))
            .respond_with(reply(
                json!({"notes": "这一部分讲了 X2 芯片。", "facts": ["售价 499 美元"]}),
            ))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .respond_with(reply(json!({"summary": "整篇摘要。", "one_liner": "一句", "key_points": [], "facts": []})))
            .mount(&server)
            .await;
        let (_dir, path) = long_article_db(&server.uri());
        let client = reqwest::Client::new();
        assert_eq!(
            summarize_article(&path, &client, 7).await.unwrap(),
            "整篇摘要。"
        );
        let requests = server.received_requests().await.unwrap();
        let bodies: Vec<String> = requests
            .iter()
            .map(|r| String::from_utf8_lossy(&r.body).into_owned())
            .collect();
        let segments = bodies.iter().filter(|b| b.contains("其中一部分")).count();
        assert!(
            (2..=summary_extract::MAX_SEGMENTS).contains(&segments),
            "{segments}"
        );
        assert_eq!(bodies.len(), segments + 1);
        assert!(
            bodies
                .iter()
                .any(|b| b.contains("【第 1 部分】") && b.contains("售价 499 美元"))
        );

        // 低档不分段：只请求一次（输入变了，不会命中缓存）。
        {
            let conn = db::open_at(&path).unwrap();
            ai_store::set_input_tier(&conn, ai_store::InputTier::Low).unwrap();
        }
        summarize_article(&path, &client, 7).await.unwrap();
        assert_eq!(
            server.received_requests().await.unwrap().len(),
            bodies.len() + 1
        );
    }

    #[test]
    fn segment_notes_keep_order_and_facts() {
        let notes = [
            json!({"notes": "开篇介绍 X2 芯片。", "facts": ["售价 499 美元", " "]}),
            json!({"notes": "结尾讨论供货。", "facts": []}),
        ];
        let out = format_segment_notes("长文", &notes);
        assert!(out.starts_with("长文\n\n以下是全文按原文顺序分 2 部分整理的笔记："));
        let first = out
            .find("【第 1 部分】开篇介绍 X2 芯片。\n- 售价 499 美元")
            .unwrap();
        assert!(out.find("【第 2 部分】结尾讨论供货。").unwrap() > first);
        assert_eq!(out.matches("\n- ").count(), 1);
    }
}
