//! 相近报道综述：手动与自动任务共用输入、校验和按簇存储；自动任务受名额与生成预算限制。

use std::path::Path;

use anyhow::{Context, Result};
use rrss_core::ai_store::{self, TASK_CLUSTER_SUMMARY};
use rrss_core::cluster_summary;
use rrss_core::feed_scope::{self, FeedScope};

use super::complete::complete_task_checked;
use super::spawn_db;

pub async fn run_cluster_summaries(db_path: &Path, client: &reqwest::Client) -> Result<()> {
    let capacity = crate::scheduler::model_capacity::Capacity::for_database(db_path);
    let batch = capacity.object("cluster-summaries".into());
    let _batch_owner = batch.lock().await;
    let jobs = spawn_db(db_path.to_path_buf(), |conn| {
        let limit = ai_store::cluster_summary_slots(conn)?.min(ai_store::CLUSTER_BATCH);
        if limit == 0 {
            return Ok(Vec::new());
        }
        let task = ai_store::resolve_task(conn, TASK_CLUSTER_SUMMARY)?;
        if task.provider.base_url.trim().is_empty() {
            return Err(rrss_core::Error::ProviderNotFound);
        }
        ai_store::ensure_auto_job_budget(conn, ai_store::BudgetKind::Generation)?;
        rrss_core::db::read_snapshot(conn, |conn| {
            cluster_summary::due_auto_summaries(conn, rrss_core::feeds::now_ms(), limit)
        })
    })
    .await?;
    let mut failed = None;
    for id in jobs {
        if let Err(error) = summarize_cluster_inner(db_path, client, id, FeedScope::All, true).await
        {
            if super::is_skip(&error) {
                return Err(error);
            }
            failed.get_or_insert(error);
        }
    }
    failed.map_or(Ok(()), Err)
}

/// 生成并存下这组的综述，返回综述正文。输出校验不过（综述为空）时报错，旧综述保留。
pub async fn summarize_cluster(
    db_path: &Path,
    client: &reqwest::Client,
    cluster_id: i64,
    scope: FeedScope,
) -> Result<String> {
    summarize_cluster_inner(db_path, client, cluster_id, scope, false).await
}

async fn summarize_cluster_inner(
    db_path: &Path,
    client: &reqwest::Client,
    cluster_id: i64,
    scope: FeedScope,
    auto: bool,
) -> Result<String> {
    let capacity = crate::scheduler::model_capacity::Capacity::for_database(db_path);
    let object = capacity.object(format!("cluster-summary:{cluster_id}"));
    let _owner = object.lock().await;
    let input = spawn_db(db_path.to_path_buf(), move |conn| {
        if auto
            && (ai_store::cluster_summary_slots(conn)? == 0
                || !cluster_summary::auto_summary_due(
                    conn,
                    cluster_id,
                    rrss_core::feeds::now_ms(),
                )?)
        {
            return Ok(None);
        }
        let scope = feed_scope::resolve(conn, scope)?;
        let members = cluster_summary::visible_members(conn, cluster_id, scope)?;
        cluster_summary::build_input(conn, &members, ai_store::input_tier(conn)?).map(Some)
    })
    .await?;
    let Some(input) = input else {
        return Ok(String::new());
    };
    anyhow::ensure!(input.members.len() >= 2, "这组相近报道已不足两篇，无法汇总");
    let members = input.members.clone();
    let digest = complete_task_checked(
        db_path,
        client,
        TASK_CLUSTER_SUMMARY,
        &input.prompt,
        cluster_summary::json_schema(),
        auto,
        |value| {
            cluster_summary::parse_output(value, &members)
                .context("模型没有给出综述，已保留上一次的结果")
        },
    )
    .await?;
    let overview = digest.overview.clone();
    let marks = input.marks;
    let saved = spawn_db(db_path.to_path_buf(), move |conn| {
        rrss_core::db::atomic_write(conn, |conn| {
            let now = rrss_core::feeds::now_ms();
            if auto {
                if ai_store::cluster_summary_slots(conn)? == 0
                    || !cluster_summary::auto_summary_due(conn, cluster_id, now)?
                {
                    return Ok(false);
                }
                let current = cluster_summary::visible_members(conn, cluster_id, FeedScope::All)?;
                if cluster_summary::member_marks(conn, &current)? != marks {
                    return Ok(false);
                }
            }
            cluster_summary::save(conn, cluster_id, &digest, &marks, now)?;
            if auto {
                conn.execute(
                    "INSERT INTO ai_usage(day,auto_cluster_summaries) VALUES(?1,1)
                    ON CONFLICT(day) DO UPDATE SET auto_cluster_summaries=auto_cluster_summaries+1",
                    [ai_store::local_day()],
                )?;
            }
            Ok(true)
        })
    })
    .await?;
    if !saved {
        return Ok(String::new());
    }
    tracing::info!(cluster_id, members = members.len(), "cluster summary saved");
    Ok(overview)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rrss_core::db;
    use serde_json::{Value, json};
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn reply(content: Value) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{ "message": { "content": content.to_string() } }],
            "usage": { "prompt_tokens": 1, "completion_tokens": 1 }
        }))
    }

    /// 一次性库：簇 7 有两个来源的三篇报道，「相近报道综述」绑到模拟模型（为空时不绑）。
    fn cluster_db(base_url: Option<&str>) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        let conn = db::open_at(&path).unwrap();
        if let Some(url) = base_url {
            let pid = crate::test_net::fake_provider(&conn, "mock", url.to_string());
            ai_store::set_task_binding(&conn, TASK_CLUSTER_SUMMARY, Some(pid), Some("m")).unwrap();
        }
        conn.execute_batch(
            r#"INSERT INTO feeds(id,url,title,created_at) VALUES
                 (1,'https://a.test/rss','甲报',0),(2,'https://b.test/rss','乙报',0);
               INSERT INTO clusters(id,label,member_count,created_at) VALUES (7,'新规',3,0);
               INSERT INTO articles(id,feed_id,title,score,published_at,cluster_id,summary,summary_json,created_at) VALUES
                 (1,1,'新规公布',5,1000,7,'新规今日公布。','{"one_liner":"新规今日公布"}',1),
                 (2,2,'厂商回应',5,2000,7,'厂商称订单照常。','{"one_liner":"厂商称订单照常"}',1);"#,
        )
        .unwrap();
        (dir, path)
    }

    fn client() -> reqwest::Client {
        reqwest::Client::builder().no_proxy().build().unwrap()
    }

    fn view(path: &std::path::Path) -> crate::views::ClusterSummaryView {
        crate::queries::cluster_summary(&db::open_at(path).unwrap(), 7, None).unwrap()
    }

    fn auto_db(base_url: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let (dir, path) = cluster_db(Some(base_url));
        let conn = db::open_at(&path).unwrap();
        db::set_setting(&conn, ai_store::CLUSTER_AUTO_SUMMARY.id, "1").unwrap();
        conn.execute(
            "UPDATE articles SET created_at=?1",
            [rrss_core::feeds::now_ms()],
        )
        .unwrap();
        (dir, path)
    }

    #[tokio::test]
    async fn disabled_or_unconfigured_auto_clusters_never_request() {
        let server = MockServer::start().await;
        let (_dir, path) = auto_db(&server.uri());
        let conn = db::open_at(&path).unwrap();
        db::set_setting(&conn, ai_store::CLUSTER_AUTO_SUMMARY.id, "0").unwrap();
        run_cluster_summaries(&path, &client()).await.unwrap();
        db::set_setting(&conn, ai_store::CLUSTER_AUTO_SUMMARY.id, "1").unwrap();
        ai_store::set_task_binding(&conn, TASK_CLUSTER_SUMMARY, None, None).unwrap();
        let error = run_cluster_summaries(&path, &client()).await.unwrap_err();
        assert!(super::super::is_skip(&error));
        assert!(server.received_requests().await.unwrap().is_empty());
        assert!(cluster_summary::load(&conn, 7).unwrap().is_none());
    }

    #[tokio::test]
    async fn generation_budget_stops_auto_clusters_but_not_manual_generation() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(reply(json!({"overview":"手动仍可生成"})))
            .mount(&server)
            .await;
        let (_dir, path) = auto_db(&server.uri());
        let conn = db::open_at(&path).unwrap();
        ai_store::set_daily_budget_tokens(&conn, 1).unwrap();
        ai_store::add_usage(&conn, &ai_store::local_day(), 1, 0).unwrap();
        let error = run_cluster_summaries(&path, &client()).await.unwrap_err();
        assert!(super::super::is_skip(&error));
        assert!(server.received_requests().await.unwrap().is_empty());
        summarize_cluster(&path, &client(), 7, FeedScope::All)
            .await
            .unwrap();
        let usage = ai_store::usage_on(&conn, &ai_store::local_day()).unwrap();
        assert_eq!(usage.auto_cluster_summaries, 0);
        assert_eq!(usage.tokens, 3);
    }

    #[tokio::test]
    async fn daily_cap_and_batch_limit_count_only_committed_automatic_groups() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(reply(json!({"overview":"自动综述"})))
            .mount(&server)
            .await;
        let (_dir, path) = auto_db(&server.uri());
        let conn = db::open_at(&path).unwrap();
        assert!(!ai_store::FOCUS_AUTO_SUMMARY.enabled(&conn).unwrap());
        conn.execute("UPDATE articles SET summary=NULL,summary_json=NULL,content_html='<p>来自正文的细节</p>'", []).unwrap();
        for id in 8..14 {
            conn.execute(
                "INSERT INTO clusters(id,label,member_count,created_at) VALUES(?1,'新组',2,0)",
                [id],
            )
            .unwrap();
            for feed in 1..=2 {
                conn.execute(
                    "INSERT INTO articles(feed_id,title,score,cluster_id,content_html,created_at)
                    VALUES(?1,?2,1,?3,'<p>来自正文的细节</p>',?4)",
                    rusqlite::params![
                        feed,
                        format!("组{id}来源{feed}"),
                        id,
                        rrss_core::feeds::now_ms()
                    ],
                )
                .unwrap();
            }
        }
        conn.execute(
            "INSERT INTO ai_usage(day,auto_cluster_summaries) VALUES(?1,19)",
            [ai_store::local_day()],
        )
        .unwrap();
        conn.execute_batch(
            "CREATE TRIGGER reject_count BEFORE UPDATE OF auto_cluster_summaries ON ai_usage
            BEGIN SELECT RAISE(ABORT,'injected'); END;",
        )
        .unwrap();
        assert!(run_cluster_summaries(&path, &client()).await.is_err());
        let saved = || {
            conn.query_row(
                "SELECT count(*) FROM clusters WHERE summary_json IS NOT NULL",
                [],
                |r| r.get::<_, i64>(0),
            )
            .unwrap()
        };
        assert_eq!(saved(), 0, "计数失败也必须回滚综述写入");
        assert_eq!(
            ai_store::usage_on(&conn, &ai_store::local_day())
                .unwrap()
                .auto_cluster_summaries,
            19
        );
        conn.execute_batch("DROP TRIGGER reject_count;").unwrap();
        run_cluster_summaries(&path, &client()).await.unwrap();
        assert_eq!(saved(), 1);
        assert_eq!(
            ai_store::usage_on(&conn, &ai_store::local_day())
                .unwrap()
                .auto_cluster_summaries,
            20
        );
        let requests = server.received_requests().await.unwrap();
        assert!(String::from_utf8_lossy(&requests[0].body).contains("来自正文的细节"));
        run_cluster_summaries(&path, &client()).await.unwrap();
        assert_eq!(
            server.received_requests().await.unwrap().len(),
            requests.len()
        );
        summarize_cluster(&path, &client(), 7, FeedScope::All)
            .await
            .unwrap();
        assert_eq!(saved(), 2);
        assert_eq!(
            ai_store::usage_on(&conn, &ai_store::local_day())
                .unwrap()
                .auto_cluster_summaries,
            20
        );
        conn.execute("UPDATE ai_usage SET day='2000-01-01'", [])
            .unwrap();
        run_cluster_summaries(&path, &client()).await.unwrap();
        assert_eq!(saved(), 6);
        assert_eq!(
            ai_store::usage_on(&conn, &ai_store::local_day())
                .unwrap()
                .auto_cluster_summaries,
            4
        );
        run_cluster_summaries(&path, &client()).await.unwrap();
        assert_eq!(saved(), 7);
    }

    #[tokio::test]
    async fn members_hidden_during_generation_are_not_saved_or_counted() {
        let server = MockServer::start().await;
        let (_dir, path) = auto_db(&server.uri());
        let changed = path.clone();
        Mock::given(method("POST"))
            .respond_with(move |_: &wiremock::Request| {
                db::open_at(&changed)
                    .unwrap()
                    .execute("UPDATE articles SET score=-10 WHERE id=2", [])
                    .unwrap();
                reply(json!({"overview":"迟到的综述"}))
            })
            .mount(&server)
            .await;
        run_cluster_summaries(&path, &client()).await.unwrap();
        let conn = db::open_at(&path).unwrap();
        assert!(cluster_summary::load(&conn, 7).unwrap().is_none());
        let usage = ai_store::usage_on(&conn, &ai_store::local_day()).unwrap();
        assert_eq!((usage.auto_cluster_summaries, usage.tokens), (0, 2));
    }

    #[tokio::test]
    async fn generates_stores_and_reports_new_reports_afterwards() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(reply(json!({
                "overview": "**新规**公布，厂商回应。",
                "additions": [{"ref": 2, "text": "订单照常交付"}, {"ref": 7, "text": "序号对不上"}],
                "conflicts": [{"refs": [1, 2], "text": "生效时间说法不一"}]
            })))
            .mount(&server)
            .await;
        let (_dir, path) = cluster_db(Some(&server.uri()));
        let before = view(&path);
        assert_eq!((before.state.as_str(), before.model_ready), ("none", true));
        assert_eq!(
            before
                .members
                .iter()
                .map(|m| m.one_liner.as_str())
                .collect::<Vec<_>>(),
            ["新规今日公布", "厂商称订单照常"]
        );
        assert!(before.estimate_tokens >= 500 && before.estimate_tokens % 500 == 0);

        let overview = summarize_cluster(&path, &client(), 7, FeedScope::All)
            .await
            .unwrap();
        assert_eq!(overview, "**新规**公布，厂商回应。");
        let ready = view(&path);
        assert_eq!((ready.state.as_str(), ready.basis), ("ready", 2));
        assert!(ready.generated_at.is_some());
        assert_eq!(ready.additions.len(), 1);
        assert_eq!(ready.additions[0].sources[0].feed_title, "乙报");
        assert_eq!(
            ready.conflicts[0]
                .sources
                .iter()
                .map(|s| s.article_id)
                .collect::<Vec<_>>(),
            [1, 2]
        );

        // 新来一篇：综述照常显示，状态变为有新报道，并列出新来的是哪篇；组头的「新 N」同步。
        db::open_at(&path)
            .unwrap()
            .execute(
                "INSERT INTO articles(id,feed_id,title,score,published_at,cluster_id,created_at) VALUES (3,1,'细则',5,3000,7,1)",
                [],
            )
            .unwrap();
        let stale = view(&path);
        assert_eq!(stale.state, "stale");
        assert_eq!(stale.overview, "**新规**公布，厂商回应。");
        assert_eq!(
            stale.fresh.iter().map(|m| m.article_id).collect::<Vec<_>>(),
            [3]
        );
        let page = crate::queries::focus_page(
            &db::open_at(&path).unwrap(),
            None,
            &[],
            &[],
            false,
            FeedScope::All,
        )
        .unwrap();
        let header = page.articles.iter().find(|row| row.row_kind == 1).unwrap();
        assert_eq!(header.cluster_fresh, Some(1));
        assert_eq!(header.cluster_sources.as_ref().map(Vec::len), Some(2));
    }

    #[tokio::test]
    async fn an_empty_overview_fails_and_keeps_the_previous_summary() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(reply(
                json!({"overview": "旧综述", "additions": [], "conflicts": []}),
            ))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .respond_with(reply(
                json!({"overview": "  ", "additions": [], "conflicts": []}),
            ))
            .mount(&server)
            .await;
        let (_dir, path) = cluster_db(Some(&server.uri()));
        summarize_cluster(&path, &client(), 7, FeedScope::All)
            .await
            .unwrap();
        // 摘要更新让输入变了，第二次请求不会命中补全缓存。
        db::open_at(&path)
            .unwrap()
            .execute(
                "UPDATE articles SET summary_json='{\"one_liner\":\"改过的一句话\"}' WHERE id=1",
                [],
            )
            .unwrap();
        let err = summarize_cluster(&path, &client(), 7, FeedScope::All)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("已保留上一次的结果"), "{err:#}");
        assert_eq!(view(&path).overview, "旧综述");
    }

    #[tokio::test]
    async fn without_a_model_the_view_lists_one_liners_and_generation_refuses() {
        let (_dir, path) = cluster_db(None);
        let conn = db::open_at(&path).unwrap();
        // 新库里摘要也没选模型；把综述的绑定清掉，确认不会调用任何服务。
        ai_store::set_task_binding(&conn, TASK_CLUSTER_SUMMARY, None, None).unwrap();
        let none = view(&path);
        assert_eq!((none.state.as_str(), none.model_ready), ("none", false));
        assert_eq!(none.members.len(), 2);
        assert!(
            summarize_cluster(&path, &client(), 7, FeedScope::All)
                .await
                .is_err()
        );
        // 不足两篇的组没法汇总。
        conn.execute("UPDATE articles SET cluster_id=NULL WHERE id=2", [])
            .unwrap();
        let err = summarize_cluster(&path, &client(), 7, FeedScope::All)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("不足两篇"));
    }
}
