use std::path::Path;

use anyhow::Result;
use rrss_ai::{digest_json_schema, digest_pick_json_schema};
use rrss_core::Error as CoreError;
use rrss_core::ai_store::{
    self, TASK_DIGEST_DAILY, TASK_DIGEST_PICK_DAILY, TASK_DIGEST_PICK_WEEKLY, TASK_DIGEST_WEEKLY,
};
use rrss_core::rag::{self, DigestCandidate};

use super::complete::{execute_prepared, prepare_resolved, task_is_current};
use super::{
    DIGEST_ITEMS_DAILY, DIGEST_ITEMS_WEEKLY, DIGEST_PICK_MAX_DAILY, DIGEST_PICK_MAX_WEEKLY,
    DIGEST_PICK_MIN, spawn_db,
};

/// 已有本周日报时，周报补充候选取多少篇、最多补几篇。
const WEEKLY_SUPPLEMENT_CANDIDATES: i64 = 30;
const WEEKLY_SUPPLEMENT_MAX: usize = 5;

struct DigestPrep {
    /// 选题候选；周报已有日报时是日报没收的补充候选，可能为空。
    mapped: Vec<ai_store::DigestItem>,
    shallow: String,
    pick_max: usize,
    pick_min: usize,
    /// 周报用到的本周日报条目；日报与没有日报的周报为空。
    daily: Vec<ai_store::DailyDigestLine>,
    inputs: Vec<(i64, String)>,
    daily_snapshot: Option<(i64, i64, String)>,
    previous: Option<(i64, String, String)>,
    tier: ai_store::InputTier,
    pick_task: Option<ai_store::ResolvedTask>,
    final_task: ai_store::ResolvedTask,
}

fn article_version(conn: &rusqlite::Connection, id: i64) -> rrss_core::Result<String> {
    let row = rrss_core::articles::get_article(conn, id)?;
    Ok(ai_store::hash_input(
        &serde_json::json!([
            row.title,
            row.summary,
            row.summary_json,
            row.blocks_json,
            row.content_html
        ])
        .to_string(),
    ))
}

fn previous_digest(
    conn: &rusqlite::Connection,
    period: &str,
    start: i64,
) -> rrss_core::Result<Option<(i64, String, String)>> {
    Ok(ai_store::latest_digest(conn, period)?
        .filter(|digest| digest.period_start == start)
        .map(|digest| (digest.id, digest.body, digest.items_json)))
}

fn daily_version(conn: &rusqlite::Connection, since: i64, until: i64) -> rrss_core::Result<String> {
    let rows = conn.prepare("SELECT id,body,COALESCE(items_json,'[]') FROM digests WHERE period='daily' AND period_start>=?1 AND period_start<=?2 ORDER BY id")?
        .query_map(rusqlite::params![since,until], |r| Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(ai_store::hash_input(&serde_json::to_string(&rows).unwrap()))
}

pub async fn run_digest(
    db_path: &Path,
    client: &reqwest::Client,
    period: &'static str,
    force: bool,
) -> Result<()> {
    crate::scheduler::digest_jobs::run(db_path, client, period, force).await
}

pub(crate) async fn generate(
    db_path: &Path,
    client: &reqwest::Client,
    period: &'static str,
    start: i64,
    force: bool,
) -> Result<()> {
    tokio::time::timeout(
        rrss_ai::deadline::ITEM_TIMEOUT,
        run_digest_inner(db_path, client, period, start, force),
    )
    .await
    .map_err(|_| rrss_ai::deadline::timeout_error())?
}

async fn run_digest_inner(
    db_path: &Path,
    client: &reqwest::Client,
    period: &'static str,
    start: i64,
    force: bool,
) -> Result<()> {
    let path = db_path.to_path_buf();
    let weekly = period == "weekly";

    let prep = spawn_db(path.clone(), move |conn| {
        let tx = conn.unchecked_transaction()?;
        let conn = &tx;
        let now = rrss_core::feeds::now_ms();
        if !force {
            let minutes = ai_store::digest_generate_minutes(conn)?;
            if !ai_store::digest_generate_due(start, now, minutes) {
                tracing::debug!(period, minutes, "digest waiting for the configured local time");
                return Ok(None);
            }
            if let Some(existing) = ai_store::latest_digest(conn, period)?
                && existing.period_start == start
            {
                return Ok(None);
            }
        }
        // 周报优先用本周日报；没有日报时退回按候选选题。
        let daily = if weekly {
            ai_store::week_daily_lines(conn, now - ai_store::WEEKLY_WINDOW_MS, now)?
        } else {
            Vec::new()
        };
        let (candidate_limit, pick_max, pick_min) = match (weekly, daily.is_empty()) {
            (false, _) => (DIGEST_ITEMS_DAILY, DIGEST_PICK_MAX_DAILY, DIGEST_PICK_MIN),
            (true, true) => (DIGEST_ITEMS_WEEKLY, DIGEST_PICK_MAX_WEEKLY, DIGEST_PICK_MIN),
            (true, false) => (WEEKLY_SUPPLEMENT_CANDIDATES, WEEKLY_SUPPLEMENT_MAX, 0),
        };
        let mut window = ai_store::DigestWindow::for_period(period, now, start, candidate_limit);
        if !daily.is_empty() {
            // 补充候选排除本周日报用过的全部来源。
            window.exclude_daily_before = Some(i64::MAX);
        }
        let items = ai_store::digest_candidates(conn, candidate_limit, &window)?;
        if items.is_empty() && daily.is_empty() {
            if force {
                return Err(CoreError::Jobs(rrss_core::error::JobFailure::DigestNoArticles));
            }
            return Ok(None);
        }
        let cands: Vec<DigestCandidate> = items
            .iter()
            .map(|i| DigestCandidate {
                article_id: i.article_id,
                title: i.title.clone(),
                summary: i.summary.clone(),
                cluster_id: i.cluster_id,
            })
            .collect();
        let kept = rag::digest_representatives(conn, cands, candidate_limit as usize)?;
        let mapped: Vec<_> = kept
            .into_iter()
            .map(|i| {
                let src = items.iter().find(|s| s.article_id == i.article_id);
                ai_store::DigestItem {
                    article_id: i.article_id,
                    title: i.title,
                    summary: i.summary,
                    summary_json: src.and_then(|s| s.summary_json.clone()),
                    cluster_id: src.and_then(|s| s.cluster_id),
                    feed_title: src.map(|s| s.feed_title.clone()).unwrap_or_default(),
                }
            })
            .collect();
        let noisy = rrss_core::feeds::noisy_feed_titles(conn).unwrap_or_default();
        let shallow = ai_store::format_digest_input(&mapped, &noisy, pick_max);
        if shallow.is_empty() && daily.is_empty() {
            if force {
                return Err(CoreError::Jobs(rrss_core::error::JobFailure::DigestEmpty));
            }
            return Ok(None);
        }
        Ok(Some(DigestPrep {
            inputs: mapped.iter().map(|item| item.article_id).chain(daily.iter().flat_map(|line| line.article_ids.iter().copied())).collect::<std::collections::BTreeSet<_>>().into_iter().map(|id| Ok((id, article_version(conn, id)?))).collect::<rrss_core::Result<Vec<_>>>()?,
            daily_snapshot: if weekly { Some((now - ai_store::WEEKLY_WINDOW_MS, now, daily_version(conn, now - ai_store::WEEKLY_WINDOW_MS, now)?)) } else { None },
            previous: previous_digest(conn, period, start)?,
            tier: ai_store::input_tier(conn)?,
            pick_task: if shallow.is_empty() { None } else { Some(ai_store::resolve_task(conn, if weekly { TASK_DIGEST_PICK_WEEKLY } else { TASK_DIGEST_PICK_DAILY })?) },
            final_task: ai_store::resolve_task(conn, if weekly { TASK_DIGEST_WEEKLY } else { TASK_DIGEST_DAILY })?,
            mapped,
            shallow,
            pick_max,
            pick_min,
            daily,
        }))
    })
    .await?;

    let Some(prep) = prep else {
        return Ok(());
    };

    let pick_ids = if prep.shallow.is_empty() {
        Vec::new()
    } else {
        let task = prep.pick_task.clone().unwrap();
        let input = prep.shallow.clone();
        let prepared = spawn_db(path.clone(), move |conn| {
            prepare_resolved(conn, task, &input, !force)
        })
        .await?;
        let pick_parsed = execute_prepared(
            db_path,
            client,
            prepared,
            &prep.shallow,
            digest_pick_json_schema(),
            |value| Ok(value.clone()),
        )
        .await?;
        ai_store::parse_digest_picks(&pick_parsed)
    };
    tracing::info!(
        period,
        picks = pick_ids.len(),
        candidates = prep.mapped.len(),
        daily_lines = prep.daily.len(),
        "digest pick round complete"
    );

    let DigestPrep {
        mapped,
        pick_max,
        pick_min,
        daily,
        inputs,
        daily_snapshot,
        previous,
        tier,
        pick_task,
        final_task,
        ..
    } = prep;
    let deep_input = spawn_db(path.clone(), move |conn| {
        let selected = ai_store::apply_digest_picks(&mapped, &pick_ids, pick_max, pick_min);
        if weekly {
            ai_store::format_weekly_input(conn, &daily, &selected, tier)
        } else {
            ai_store::format_digest_deep_input(conn, &selected, tier)
        }
    })
    .await?;
    if deep_input.prompt.trim().is_empty() {
        if force {
            anyhow::bail!(crate::views::UiError::new(
                crate::views::UiErrorKind::Common(crate::ui_error::common::CommonError::Internal),
                "digest deep pack empty",
            ));
        }
        return Ok(());
    }

    let task = final_task.clone();
    let input = deep_input.prompt.clone();
    let prepared = spawn_db(path.clone(), move |conn| {
        prepare_resolved(conn, task, &input, !force)
    })
    .await?;
    let numbered = deep_input.numbered_items;
    let draft = execute_prepared(
        db_path,
        client,
        prepared,
        &deep_input.prompt,
        digest_json_schema(),
        |parsed| Ok(ai_store::prepare_digest_draft(parsed, &numbered)?),
    )
    .await?;
    let stored = spawn_db(path, move |conn| {
        rrss_core::db::atomic_write(conn, |conn| {
            if !task_is_current(conn, &final_task)?
                || ai_store::input_tier(conn)? != tier
                || previous_digest(conn, period, start)? != previous
            {
                return Ok(false);
            }
            if let Some(task) = pick_task
                && !task_is_current(conn, &task)?
            {
                return Ok(false);
            }
            if let Some((since, until, version)) = daily_snapshot
                && daily_version(conn, since, until)? != version
            {
                return Ok(false);
            }
            for (id, version) in inputs {
                match article_version(conn, id) {
                    Ok(current) if current == version => {}
                    Ok(_) | Err(CoreError::ArticleNotFound) => return Ok(false),
                    Err(error) => return Err(error),
                }
            }
            ai_store::insert_digest(conn, period, start, &draft.body, &draft.items_json)?;
            Ok(true)
        })
    })
    .await?;
    anyhow::ensure!(stored, rrss_core::Error::Jobs(rrss_core::error::JobFailure::DigestChanged));
    tracing::info!(period, "digest stored");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rrss_core::db;
    use serde_json::{Value, json};
    use wiremock::matchers::{body_string_contains, method};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn reply(content: Value) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{ "message": { "content": content.to_string() } }],
            "usage": { "prompt_tokens": 1, "completion_tokens": 1 }
        }))
    }

    /// 已有本周日报时：补充候选排除日报来源，成稿输入带日报条目、来源与补充文章，来源编号能对上。
    #[tokio::test]
    async fn weekly_digest_builds_on_daily_digests_and_supplements() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(body_string_contains("请选出"))
            .respond_with(reply(
                json!({"picks": [{"article_id": 2, "reason": "补充"}]}),
            ))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .respond_with(reply(json!({"sections": [{"title": "硬件", "items": [
                {"text": "**Acme** 本周发布 X2 并公布跑分", "refs": [1]},
                {"text": "**Beta** 开源新工具", "refs": [2]}
            ]}]})))
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        {
            let conn = db::open_at(&path).unwrap();
            let pid = crate::test_net::fake_provider(&conn, "mock", server.uri());
            for task in [TASK_DIGEST_PICK_WEEKLY, TASK_DIGEST_WEEKLY] {
                ai_store::set_task_binding(&conn, task, Some(pid), Some("m")).unwrap();
            }
            let now = rrss_core::feeds::now_ms();
            conn.execute("INSERT INTO feeds (id, url, title, created_at) VALUES (1, 'https://f.test/rss', 'F', 1)", [])
                .unwrap();
            for (id, title) in [(1, "Acme X2 发布"), (2, "Beta 开源工具"), (3, "别的新闻")]
            {
                conn.execute(
                    "INSERT INTO articles (id, feed_id, title, hash, created_at, published_at, score, summary, content_html)
                     VALUES (?1, 1, ?2, ?3, ?4, ?4, 5, '摘要。', '<p>正文。</p>')",
                    rusqlite::params![id, title, format!("h{id}"), now - 2 * 3_600_000],
                )
                .unwrap();
            }
            let numbered = ai_store::digest_items_by_ids(&conn, &[1]).unwrap();
            let draft = ai_store::prepare_digest_draft(
                &json!({"sections": [{"title": "硬件", "items": [{"text": "**Acme** 发布 X2", "refs": [1]}]}]}),
                &numbered,
            )
            .unwrap();
            ai_store::insert_digest(
                &conn,
                "daily",
                now - 86_400_000,
                &draft.body,
                &draft.items_json,
            )
            .unwrap();
        }
        let client = reqwest::Client::new();
        run_digest(&path, &client, "weekly", true).await.unwrap();

        let bodies: Vec<String> = server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .map(|r| String::from_utf8_lossy(&r.body).into_owned())
            .collect();
        assert_eq!(bodies.len(), 2);
        assert!(
            bodies[0].contains("[id:2]") && !bodies[0].contains("[id:1]"),
            "{}",
            bodies[0]
        );
        assert!(bodies[0].contains("请选出 2 篇"), "{}", bodies[0]);
        assert!(
            bodies[1].contains("## 日报条目") && bodies[1].contains("**Acme** 发布 X2 〈1〉"),
            "{}",
            bodies[1]
        );
        assert!(
            bodies[1].contains("〈2〉Beta 开源工具") && !bodies[1].contains("别的新闻"),
            "{}",
            bodies[1]
        );

        let conn = db::open_at(&path).unwrap();
        let weekly = ai_store::latest_digest(&conn, "weekly").unwrap().unwrap();
        assert!(weekly.body.contains("Acme"));
        let ids: Vec<i64> = serde_json::from_str::<Value>(&weekly.items_json).unwrap()["items"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|item| {
                item["refs"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|r| r["article_id"].as_i64().unwrap())
            })
            .collect();
        assert_eq!(ids, vec![1, 2]);

        // A daily revision received during final generation invalidates the
        // weekly result even when the candidate article rows did not change.
        conn.execute("DELETE FROM ai_cache", []).unwrap();
        server.reset().await;
        let changed = path.clone();
        Mock::given(method("POST")).respond_with(move |request: &wiremock::Request| {
            if String::from_utf8_lossy(&request.body).contains("请选出") {
                reply(json!({"picks":[{"article_id":2,"reason":"补充"}]}))
            } else {
                db::open_at(&changed).unwrap().execute("UPDATE digests SET body=body||' revised' WHERE period='daily'", []).unwrap();
                reply(json!({"sections":[{"title":"late","items":[{"text":"late weekly","refs":[1]}]}]}))
            }
        }).mount(&server).await;
        assert!(run_digest(&path, &client, "weekly", true).await.is_err());
        assert_eq!(
            ai_store::latest_digest(&conn, "weekly")
                .unwrap()
                .unwrap()
                .body,
            weekly.body
        );
    }
}
