//! 阶段 23 · V1：探针请求与重建，沿用 API 网络用途、嵌入队列与预算。
use super::{
    embeddings::{EmbedQueue, embed_and_save},
    spawn_db,
};
use crate::{network::NetworkClients, views::VectorSpaceStatusView};
use anyhow::Result;
use rrss_core::{ai_store, network::NetworkScope, rag, vector_space as spaces};
use std::{path::Path, sync::OnceLock};
type Notifier = Box<dyn Fn(&VectorSpaceStatusView) + Send + Sync>;
static NOTIFY: OnceLock<Notifier> = OnceLock::new();
pub fn set_notifier(notify: impl Fn(&VectorSpaceStatusView) + Send + Sync + 'static) {
    let _ = NOTIFY.set(Box::new(notify));
}

pub async fn check(db: &Path, client: &reqwest::Client, force: bool) -> Result<bool> {
    let capacity = crate::scheduler::model_capacity::Capacity::for_database(db);
    let mut failures = capacity.checks.lock().await;
    let plan = spawn_db(db.to_path_buf(), move |conn| {
        let space = spaces::building(conn)?.or(spaces::active(conn)?);
        let due = force
            || space
                .as_ref()
                .and_then(|s| s.checked_at)
                .is_none_or(|t| t <= rrss_core::feeds::now_ms() - spaces::CHECK_INTERVAL_MS);
        let ep = match &space {
            Some(s) => spaces::endpoint(conn, s)?,
            None => ai_store::resolve_embed(conn)?,
        };
        Ok(ep.map(|ep| (space.map(|s| s.id), ep, due)))
    })
    .await?;
    let Some((id, ep, due)) = plan else {
        return Ok(false);
    };
    let now = tokio::time::Instant::now();
    let key = (
        id,
        ep.base_url.clone(),
        ai_store::hash_input(&serde_json::json!([ep.model, ep.api_key]).to_string()),
    );
    failures.retain(|_, at| now.saturating_duration_since(*at) < super::retry::DELAY);
    if !force && failures.contains_key(&key) {
        anyhow::bail!("最近一次向量核对失败，已暂停本轮写入；可立即核对重试。");
    }
    if !due {
        return Ok(false);
    }
    // 失败也节流；后续写入必须继续暂停，不能把未核对当成通过。
    failures.insert(key.clone(), now);
    let mut queue = EmbedQueue::new(
        db,
        client,
        &ep,
        spaces::PROBES.iter().map(|s| s.to_string()).collect(),
    );
    let mut vectors = vec![Vec::new(); spaces::PROBES.len()];
    while let Some(batch) = queue.next().await {
        for (i, result) in batch? {
            vectors[i] = result.map_err(|_| anyhow::anyhow!("向量探针核对失败，请重试。"))?;
        }
    }
    drop(queue);
    let result = spawn_db(db.to_path_buf(), move |conn| {
        rrss_core::db::atomic_write(conn, |conn| {
            let current = spaces::building(conn)?.or(spaces::active(conn)?);
            if current.as_ref().map(|s| s.id) != id {
                return Ok(None);
            }
            let space = match current {
                Some(s) => Some(s),
                None => spaces::ensure_for_embedding(conn, &ep, vectors[0].len())?,
            };
            space
                .map(|s| spaces::record_probes(conn, s.id, &vectors))
                .transpose()
        })
    })
    .await?;
    failures.remove(&key);
    if result == Some(false) {
        tracing::warn!(
            error_kind = "vector_fingerprint_mismatch",
            "vector fingerprint differs; writes paused"
        );
        if let Some(notify) = NOTIFY.get() {
            notify(&spawn_db(db.to_path_buf(), crate::queries::vector_space_status).await?);
        }
    }
    Ok(result.unwrap_or(false))
}
fn sync_chunks(conn: &rusqlite::Connection) -> rrss_core::Result<()> {
    for article in rag::rebuild_articles(conn)? {
        rag::sync_article_chunks(conn, &article, article.tier)?;
    }
    Ok(())
}
pub async fn rebuild(db: &Path, client: &reqwest::Client) -> Result<()> {
    // One finite step; the scheduler resumes remaining staging work later.
    const REBUILD_BATCH: usize = super::embeddings::EMBED_INPUTS_MAX;
    let capacity = crate::scheduler::model_capacity::Capacity::for_database(db);
    let plan = spawn_db(db.to_path_buf(), move |conn| {
        rrss_core::db::atomic_write(conn, |conn| {
            let Some(space) = spaces::write_space(conn)?.filter(|s| s.rebuild_from.is_some())
            else {
                return Ok(None);
            };
            let Some(ep) = spaces::endpoint(conn, &space)? else {
                return Ok(None);
            };
            sync_chunks(conn)?;
            let context = ai_store::hash_input(
                &serde_json::json!([space.id, ep.base_url, ep.model, ep.api_key]).to_string(),
            );
            let retries = {
                let mut retries = capacity.retries.lock().unwrap();
                for kind in [
                    super::retry::Kind::RebuildChunk,
                    super::retry::Kind::RebuildInterest,
                    super::retry::Kind::RebuildMemory,
                ] {
                    retries.context(kind, &context);
                }
                retries.clone()
            };
            let inputs =
                spaces::pending_rebuild_batch(conn, space.id, REBUILD_BATCH, |kind, id, text| {
                    retries.allowed(
                        super::retry::Kind::rebuild(kind),
                        id,
                        &ai_store::hash_input(text),
                    )
                })?;
            Ok(Some((space, ep, inputs)))
        })
    })
    .await?;
    let Some((space, ep, inputs)) = plan else {
        return Ok(());
    };
    let texts = inputs.iter().map(|(_, _, text)| text.clone()).collect();
    let id = space.id;
    let inputs = std::sync::Arc::new(inputs);
    embed_and_save(
        db,
        client,
        &ep,
        (0..inputs.len() as i64).collect(),
        texts,
        move |conn, done| {
            use super::retry::Update;
            let mut updates = Vec::new();
            for (i, result) in done {
                let (kind, row_id, text) = &inputs[*i as usize];
                let retry_kind = super::retry::Kind::rebuild(kind);
                match result {
                    Ok(vector) => {
                        spaces::stage(conn, &space, kind, *row_id, Some(text), vector)?;
                        updates.push(Update::Success {
                            kind: retry_kind,
                            id: *row_id,
                        });
                    }
                    Err(_) => updates.push(Update::Failed {
                        kind: retry_kind,
                        id: *row_id,
                        input: ai_store::hash_input(text),
                        terminal: true,
                    }),
                }
            }
            Ok(updates)
        },
    )
    .await?;
    spawn_db(db.to_path_buf(), move |conn| {
        rrss_core::db::atomic_write(conn, |conn| {
            if spaces::write_space(conn)?.is_none_or(|space| space.id != id) {
                return Ok(false);
            }
            sync_chunks(conn)?;
            spaces::finish_rebuild(conn, id)
        })
    })
    .await?;
    Ok(())
}
pub async fn check_now(db: &Path, network: &NetworkClients) -> Result<VectorSpaceStatusView> {
    check(db, &network.client(NetworkScope::Api).await?, true).await?;
    spawn_db(db.to_path_buf(), crate::queries::vector_space_status).await
}
pub async fn restore(db: &Path, network: &NetworkClients) -> Result<VectorSpaceStatusView> {
    spawn_db(db.to_path_buf(), |conn| {
        crate::configuration::write(conn, |conn| {
            spaces::cancel_rebuild(conn)?;
            spaces::restore_binding(conn)
        })
    })
    .await?;
    check_now(db, network).await
}
#[cfg(test)]
mod tests {
    use super::*;
    use rrss_core::{db, interests, memories, storage};
    use rusqlite::params;
    use std::path::PathBuf;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, Request, ResponseTemplate};

    fn integer(conn: &rusqlite::Connection, sql: &str, params: impl rusqlite::Params) -> i64 {
        conn.query_row(sql, params, |row| row.get(0)).unwrap()
    }

    struct DimsFixture {
        _dir: tempfile::TempDir,
        path: PathBuf,
        server: MockServer,
    }
    impl DimsFixture {
        async fn new(setting: Option<&str>) -> Self {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .respond_with(|req: &Request| {
                    let body: serde_json::Value = req.body_json().unwrap();
                    assert!(body.get("dimensions").is_none());
                    let inputs = body["input"]
                        .as_array()
                        .cloned()
                        .unwrap_or_else(|| vec![body["input"].clone()]);
                    let data: Vec<_> = inputs
                        .iter()
                        .enumerate()
                        .map(|(i, _)| serde_json::json!({"index":i,"embedding":vec![1.0_f32;3072]}))
                        .collect();
                    ResponseTemplate::new(200)
                        .set_body_json(serde_json::json!({"data":data,"usage":{"prompt_tokens":1}}))
                })
                .mount(&server)
                .await;
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("dims.db");
            let conn = db::open_at(&path).unwrap();
            db::set_setting(&conn, "pi_auto_download", "0").unwrap();
            let provider = crate::test_net::fake_provider(&conn, "fake", server.uri());
            ai_store::set_task_binding(&conn, "embed", Some(provider), Some("fixture")).unwrap();
            if let Some(setting) = setting {
                crate::actions::set_vector_dims(&conn, setting).unwrap();
            }
            conn.execute_batch("INSERT INTO feeds(id,url,created_at) VALUES(1,'https://fixture.invalid/feed',0);
                INSERT INTO articles(id,feed_id,title,summary,created_at) VALUES(1,1,'fixture','body',0);").unwrap();
            interests::insert_interest(&conn, "prototype", "manual").unwrap();
            memories::insert_memory(&conn, memories::MemoryKind::Semantic, "fact", "test", 1.0)
                .unwrap();
            let fixture = Self {
                _dir: dir,
                path,
                server,
            };
            fixture.embed().await;
            fixture
        }
        async fn embed(&self) {
            super::super::embeddings::run_embed_queue(&self.path, &reqwest::Client::new())
                .await
                .unwrap();
        }
        async fn requests(&self) -> usize {
            self.server.received_requests().await.unwrap().len()
        }
    }

    fn assert_dimensions(conn: &rusqlite::Connection, dims: i64) {
        let active = spaces::active(conn).unwrap().unwrap();
        for table in ["chunks", "interests", "memories", "vector_space_probes"] {
            let (count, shortest, longest): (i64, i64, i64) = conn.query_row(
                &format!("SELECT COUNT(*),MIN(length(embedding)/4),MAX(length(embedding)/4) FROM {table} WHERE space_id=?1"),
                [active.id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            ).unwrap();
            assert!(count > 0, "{table}");
            assert_eq!((shortest, longest), (dims, dims), "{table}");
        }
    }

    #[tokio::test]
    async fn auto_fits_3072_service_dimensions_and_checks_fitted_probes() {
        let fixture = DimsFixture::new(None).await;
        let conn = db::open_at(&fixture.path).unwrap();
        let status = crate::queries::vector_space_status(&conn).unwrap();
        let space = status.effective.unwrap();
        assert_eq!((space.dims, space.source_dims), (1024, 3072));
        assert_eq!(status.dims_setting, "auto");
        assert_dimensions(&conn, 1024);
        assert!(
            check(&fixture.path, &reqwest::Client::new(), true)
                .await
                .unwrap()
        );
        let ep = ai_store::resolve_embed(&conn).unwrap().unwrap();
        assert!(
            spaces::ensure_for_embedding(&conn, &ep, 3072)
                .unwrap()
                .is_some()
        );
        for source_dims in [1024, 2048, 4096] {
            assert!(
                spaces::ensure_for_embedding(&conn, &ep, source_dims)
                    .unwrap()
                    .is_none()
            );
        }
        conn.execute(
            "UPDATE vector_spaces SET source_dims=0 WHERE id=?1",
            [space.id],
        )
        .unwrap();
        assert!(
            spaces::ensure_for_embedding(&conn, &ep, 1024)
                .unwrap()
                .is_some()
        );
        assert!(
            spaces::ensure_for_embedding(&conn, &ep, 3072)
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn stale_space_rejects_dimension_changes_without_writes_or_requests() {
        let fixture = DimsFixture::new(None).await;
        let conn = db::open_at(&fixture.path).unwrap();
        let active = spaces::active(&conn).unwrap().unwrap();
        conn.execute(
            "UPDATE vector_spaces SET state='stale' WHERE id=?1",
            [active.id],
        )
        .unwrap();
        let before = fixture.requests().await;
        for value in ["256", "1024", "native"] {
            let err = crate::actions::set_vector_dims(&conn, value).unwrap_err();
            assert_eq!(
                err.to_string(),
                "向量库已停写，请先恢复原配置或重建，再改维度。"
            );
        }
        let status = crate::queries::vector_space_status(&conn).unwrap();
        assert_eq!(status.dims_setting, "auto");
        assert!(status.building.is_none());
        let effective = status.effective.unwrap();
        assert_eq!(
            (effective.id, effective.state.as_str()),
            (active.id, "stale")
        );
        assert_eq!(integer(&conn, "SELECT COUNT(*) FROM vector_spaces", []), 1);
        assert_eq!(integer(&conn, "SELECT COUNT(*) FROM vector_staging", []), 0);
        assert_dimensions(&conn, 1024);
        assert_eq!(fixture.requests().await, before);
    }

    #[tokio::test]
    async fn native_keeps_all_3072_service_dimensions() {
        let fixture = DimsFixture::new(Some("native")).await;
        let conn = db::open_at(&fixture.path).unwrap();
        let space = spaces::active(&conn).unwrap().unwrap();
        assert_eq!((space.dims, space.source_dims), (3072, 3072));
        assert_dimensions(&conn, 3072);
    }

    #[tokio::test]
    async fn rebuild_yields_after_a_bounded_step_and_resumes_remaining_inputs() {
        let fixture = DimsFixture::new(None).await;
        let _runtime = NetworkClients::new(&fixture.path);
        let conn = db::open_at(&fixture.path).unwrap();
        for id in 2..=130 {
            conn.execute(
                "INSERT INTO articles(id,feed_id,title,created_at) VALUES(?1,1,?2,0)",
                rusqlite::params![id, format!("article-{id}")],
            )
            .unwrap();
        }
        let status = crate::actions::set_vector_dims(&conn, "2048").unwrap();
        let building = status.building.unwrap().space.id;
        fixture.embed().await;
        let (written, total) = spaces::rebuild_progress(&conn, building).unwrap();
        assert!(
            written > 0 && written < total,
            "one finite request must leave a resumable backlog"
        );
        assert!(
            super::super::embeddings::has_pending(&fixture.path)
                .await
                .unwrap()
        );
        for _ in 0..2 {
            fixture.embed().await;
        }
        assert!(spaces::building(&conn).unwrap().is_none());
        assert_eq!(spaces::active(&conn).unwrap().unwrap().id, building);
        assert!(
            !super::super::embeddings::has_pending(&fixture.path)
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn shrinking_retires_the_old_space_without_requesting_embeddings() {
        let fixture = DimsFixture::new(None).await;
        let conn = db::open_at(&fixture.path).unwrap();
        let old = spaces::active(&conn).unwrap().unwrap();
        let before = fixture.requests().await;
        let status = crate::actions::set_vector_dims(&conn, " 256 ").unwrap();
        assert!(status.building.is_none());
        assert_eq!(status.dims_setting, "256");
        let space = status.effective.unwrap();
        assert_ne!(space.id, old.id);
        assert_eq!((space.dims, space.source_dims), (256, 3072));
        assert_eq!(
            integer(
                &conn,
                "SELECT COUNT(*) FROM vector_spaces WHERE id=?1 AND state='retired'",
                [old.id]
            ),
            1
        );
        assert_dimensions(&conn, 256);
        let local = rag::title_embedding(&conn, 1).unwrap().unwrap();
        let fresh = spaces::fit(vec![1.0; 3072], 256);
        assert!(local.iter().zip(fresh).all(|(a, b)| (a - b).abs() < 1e-7));
        assert_eq!(fixture.requests().await, before);
    }

    #[tokio::test]
    async fn shrinking_with_unembedded_chunks_waits_for_the_existing_rebuild_job() {
        let fixture = DimsFixture::new(None).await;
        let conn = db::open_at(&fixture.path).unwrap();
        conn.execute(
            "INSERT INTO articles(id,feed_id,title,created_at) VALUES(2,1,'missing',0)",
            [],
        )
        .unwrap();
        sync_chunks(&conn).unwrap();
        let before = fixture.requests().await;
        let status = crate::actions::set_vector_dims(&conn, "256").unwrap();
        let building = status.building.unwrap();
        assert_eq!(building.space.dims, 256);
        assert!(building.written > 0 && building.written < building.total);
        assert_eq!(fixture.requests().await, before);
        assert!(rag::title_embedding(&conn, 2).unwrap().is_none());
        fixture.embed().await;
        assert!(fixture.requests().await > before);
        assert!(spaces::building(&conn).unwrap().is_none());
        assert_eq!(
            spaces::active(&conn).unwrap().unwrap().id,
            building.space.id
        );
        assert_eq!(rag::title_embedding(&conn, 2).unwrap().unwrap().len(), 256);
    }

    #[tokio::test]
    async fn enlarging_rebuilds_via_the_service() {
        let fixture = DimsFixture::new(None).await;
        let conn = db::open_at(&fixture.path).unwrap();
        let before = fixture.requests().await;
        let status = crate::actions::set_vector_dims(&conn, "native").unwrap();
        assert_eq!(status.effective.unwrap().dims, 1024);
        let building = status.building.unwrap();
        assert_eq!(
            (
                building.space.dims,
                building.space.source_dims,
                building.written
            ),
            (0, 0, 0)
        );
        assert_eq!(fixture.requests().await, before);
        fixture.embed().await;
        assert!(fixture.requests().await > before);
        let space = spaces::active(&conn).unwrap().unwrap();
        assert_eq!((space.id, space.dims), (building.space.id, 3072));
        assert_dimensions(&conn, 3072);
    }

    #[tokio::test]
    async fn cancellation_during_a_rebuild_batch_rejects_late_vectors_and_space_switch() {
        let fixture = DimsFixture::new(None).await;
        let conn = db::open_at(&fixture.path).unwrap();
        let original = spaces::active(&conn).unwrap().unwrap().id;
        let building = crate::actions::set_vector_dims(&conn, "native")
            .unwrap()
            .building
            .unwrap()
            .space
            .id;
        fixture.server.reset().await;
        let path = fixture.path.clone();
        let cancelled = Arc::new(AtomicBool::new(false));
        let did_cancel = cancelled.clone();
        Mock::given(method("POST")).respond_with(move |request: &Request| {
            let body: serde_json::Value = request.body_json().unwrap();
            let inputs = body["input"].as_array().cloned().unwrap_or_else(|| vec![body["input"].clone()]);
            let probes = inputs.first().and_then(serde_json::Value::as_str) == Some(spaces::PROBES[0]);
            if !probes && !did_cancel.swap(true, Ordering::SeqCst) {
                crate::actions::cancel_vector_rebuild(&db::open_at(&path).unwrap()).unwrap();
            }
            let data: Vec<_> = inputs.iter().enumerate().map(|(index, _)| serde_json::json!({"index":index,"embedding":vec![1.0_f32;3072]})).collect();
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"data":data,"usage":{"prompt_tokens":1}}))
        }).mount(&fixture.server).await;
        fixture.embed().await;
        assert!(cancelled.load(Ordering::SeqCst));
        assert!(spaces::building(&conn).unwrap().is_none());
        assert_eq!(spaces::active(&conn).unwrap().unwrap().id, original);
        assert_eq!(
            integer(
                &conn,
                "SELECT COUNT(*) FROM vector_staging WHERE space_id=?1",
                [building]
            ),
            0
        );
        assert_dimensions(&conn, 1024);
    }

    #[tokio::test]
    async fn rebuild_skips_unembedded_unread_bodies_and_exhausted_chunks() {
        let fixture = DimsFixture::new(None).await;
        let conn = db::open_at(&fixture.path).unwrap();
        conn.execute_batch(
            "INSERT INTO articles(id,feed_id,title,content_html,read,starred,created_at) VALUES
             (2,1,'Previously read','',1,0,0),
             (3,1,'Read with failed body','<p>Exhausted body</p>',1,0,0),
             (4,1,'Starred','<p>Eligible starred body</p>',0,1,0);",
        )
        .unwrap();
        conn.execute(
            "UPDATE articles SET content_html=?1 WHERE id=2",
            [format!("<p>{}</p>", "u".repeat(1000))],
        )
        .unwrap();
        sync_chunks(&conn).unwrap();
        let existing = integer(
            &conn,
            "SELECT id FROM chunks WHERE article_id=2 AND ordinal=2",
            [],
        );
        rag::set_chunk_embedding(&conn, existing, "fixture", &vec![1.0; 1024]).unwrap();
        rrss_core::articles::set_read(&conn, 2, false).unwrap();
        conn.execute_batch("UPDATE chunks SET attempts=3 WHERE id IN (
            SELECT id FROM chunks WHERE (article_id=2 AND ordinal=2) OR (article_id=3 AND ordinal=1));").unwrap();
        let excluded: Vec<(i64, String)> = conn.prepare(
            "SELECT id,text FROM chunks WHERE (article_id=2 AND ordinal=1) OR (article_id=3 AND ordinal=1)"
        ).unwrap().query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap().collect::<rusqlite::Result<_>>().unwrap();
        assert_eq!(excluded.len(), 2);
        let existing_text: String = conn
            .query_row("SELECT text FROM chunks WHERE id=?1", [existing], |r| {
                r.get(0)
            })
            .unwrap();
        let before = fixture.requests().await;
        let building = crate::actions::set_vector_dims(&conn, "native")
            .unwrap()
            .building
            .unwrap();
        assert_eq!(building.total, 8);
        fixture.embed().await;
        assert!(spaces::building(&conn).unwrap().is_none());
        assert_eq!(
            spaces::active(&conn).unwrap().unwrap().id,
            building.space.id
        );
        assert_dimensions(&conn, 3072);
        let sent: Vec<String> = fixture.server.received_requests().await.unwrap()[before..]
            .iter()
            .flat_map(|request| {
                let body: serde_json::Value = request.body_json().unwrap();
                body["input"]
                    .as_array()
                    .cloned()
                    .unwrap_or_else(|| vec![body["input"].clone()])
                    .into_iter()
                    .map(|v| v.as_str().unwrap().to_owned())
                    .collect::<Vec<_>>()
            })
            .collect();
        for (id, text) in &excluded {
            assert!(!sent.contains(text));
            assert_eq!(
                integer(
                    &conn,
                    "SELECT COUNT(*) FROM chunks WHERE id=?1 AND embedding IS NULL AND space_id IS NULL",
                    [id]
                ),
                1
            );
        }
        assert!(sent.contains(&existing_text));
        assert!(sent.contains(&"Eligible starred body".to_string()));
        assert_eq!(
            integer(
                &conn,
                "SELECT attempts FROM chunks WHERE article_id=3 AND ordinal=1",
                []
            ),
            3
        );
        assert_eq!(
            integer(&conn, "SELECT space_id FROM chunks WHERE id=?1", [existing]),
            building.space.id
        );

        let before = fixture.requests().await;
        let shrunk = crate::actions::set_vector_dims(&conn, "256").unwrap();
        assert!(shrunk.building.is_none());
        assert_ne!(shrunk.effective.unwrap().id, building.space.id);
        assert_dimensions(&conn, 256);
        assert_eq!(fixture.requests().await, before);
        assert_eq!(integer(&conn, "SELECT COUNT(*) FROM vector_staging", []), 0);
    }

    #[tokio::test]
    async fn cancel_dims_changes_restores_native_or_the_effective_dimension_number() {
        for (setting, next, expected) in
            [(None, "2048", "1024"), (Some("native"), "1024", "native")]
        {
            let fixture = DimsFixture::new(setting).await;
            let conn = db::open_at(&fixture.path).unwrap();
            let active = spaces::active(&conn).unwrap().unwrap();
            interests::insert_interest(&conn, "not embedded", "manual").unwrap();
            assert!(
                crate::actions::set_vector_dims(&conn, next)
                    .unwrap()
                    .building
                    .is_some()
            );
            assert!(matches!(
                crate::actions::set_vector_dims(&conn, "512"),
                Err(rrss_core::Error::VectorSpaceRebuildInProgress)
            ));
            assert_eq!(spaces::dims_setting(&conn).unwrap(), next);
            let cancelled = crate::actions::cancel_vector_rebuild(&conn).unwrap();
            assert!(cancelled.building.is_none());
            assert_eq!(cancelled.dims_setting, expected);
            assert_eq!(cancelled.effective.unwrap().id, active.id);
            assert_eq!(integer(&conn, "SELECT COUNT(*) FROM vector_staging", []), 0);
        }
        let fixture = DimsFixture::new(None).await;
        let conn = db::open_at(&fixture.path).unwrap();
        let active = spaces::active(&conn).unwrap().unwrap();
        conn.execute(
            "UPDATE vector_spaces SET state='stale' WHERE id=?1",
            [active.id],
        )
        .unwrap();
        db::set_setting_enabled(&conn, spaces::ALLOW_MODEL_CHANGE, true).unwrap();
        spaces::prepare_binding(&conn, active.provider_id, Some(&active.model)).unwrap();
        assert!(spaces::building(&conn).unwrap().is_some());
        assert_eq!(
            crate::actions::cancel_vector_rebuild(&conn)
                .unwrap()
                .dims_setting,
            "auto"
        );
    }

    #[test]
    fn dimension_setting_validates_and_does_not_use_the_settings_registry() {
        let dir = tempfile::tempdir().unwrap();
        let conn = db::open_at(&dir.path().join("settings.db")).unwrap();
        for value in ["", "0", "-1", "1.5", "other", "999999999999999999999999999"] {
            assert!(crate::actions::set_vector_dims(&conn, value).is_err());
        }
        assert_eq!(spaces::dims_setting(&conn).unwrap(), "auto");
        assert!(crate::actions::set_setting(&conn, spaces::DIMS_SETTING, "native").is_err());
        let status = crate::actions::set_vector_dims(&conn, " native ").unwrap();
        assert_eq!(status.dims_setting, "native");
        assert!(status.effective.is_none() && status.building.is_none());
        conn.execute("INSERT INTO vector_spaces(base_url,model,dims,prefix_version,normalized,state,created_at)
            VALUES('','legacy',3,1,0,'active',0)", []).unwrap();
        let status = crate::actions::set_vector_dims(&conn, "auto").unwrap();
        assert!(status.building.is_none());
        let space = status.effective.unwrap();
        assert_eq!((space.dims, space.source_dims), (3, 0));
        assert_eq!(integer(&conn, "SELECT COUNT(*) FROM vector_spaces", []), 1);
    }

    #[test]
    fn unread_preserves_existing_body_vectors_and_rebuild_keeps_the_higher_tier() {
        let dir = tempfile::tempdir().unwrap();
        let conn = db::open_at(&dir.path().join("tiers.db")).unwrap();
        conn.execute_batch(
            "INSERT INTO vector_spaces(base_url,model,dims,prefix_version,normalized,state,created_at)
             VALUES('','m',2,1,0,'active',0);
             INSERT INTO feeds(id,url,created_at) VALUES(1,'https://fixture.invalid/feed',0);
             INSERT INTO articles(id,feed_id,title,content_html,read,created_at)
             VALUES(1,1,'Old read article','<p>Original body</p>',1,0);",
        )
        .unwrap();
        conn.execute(
            "UPDATE articles SET content_html=?1 WHERE id=1",
            [format!("<p>{}</p>", "b".repeat(1000))],
        )
        .unwrap();
        let article = rag::due_embed_articles(&conn, 1).unwrap().remove(0);
        for chunk in rag::sync_article_chunks(&conn, &article, article.tier).unwrap() {
            rag::set_chunk_embedding(&conn, chunk.id, "m", &[1.0, 0.0]).unwrap();
        }
        rrss_core::articles::set_read(&conn, 1, false).unwrap();
        conn.execute_batch(
            "UPDATE chunks SET embedding=NULL,space_id=NULL WHERE article_id=1 AND ordinal=2;
             UPDATE chunks SET space_id=NULL WHERE article_id=1 AND ordinal=1;",
        )
        .unwrap();
        let body_snapshot = || {
            conn.query_row(
            "SELECT group_concat(snapshot) FROM (
             SELECT json_array(id,ordinal,kind,text,hex(embedding),model,source_hash,attempts,last_error,space_id) AS snapshot
             FROM chunks WHERE article_id=1 AND ordinal>0 ORDER BY ordinal)",
            [], |r| r.get::<_, String>(0),
        ).unwrap()
        };
        let before = body_snapshot();
        let mut unread = rag::rebuild_articles(&conn).unwrap().remove(0);
        assert_eq!(unread.tier, 2);
        unread.content_html = Some("<p>Should not replace existing body</p>".into());
        assert!(
            rag::sync_article_chunks(&conn, &unread, 1)
                .unwrap()
                .is_empty()
        );
        assert_eq!(body_snapshot(), before);
        assert_eq!(
            integer(&conn, "SELECT embed_tier FROM articles WHERE id=1", []),
            2
        );
        assert!(rag::due_embed_articles(&conn, 1).unwrap().is_empty());

        conn.execute_batch(
            "INSERT INTO articles(id,feed_id,title,content_html,starred,created_at)
             VALUES(2,1,'Starred','<p>New body</p>',1,0);",
        )
        .unwrap();
        sync_chunks(&conn).unwrap();
        assert_eq!(body_snapshot(), before);
        assert_eq!(
            integer(&conn, "SELECT embed_tier FROM articles WHERE id=2", []),
            2
        );
        assert_eq!(
            integer(
                &conn,
                "SELECT COUNT(*) FROM chunks WHERE article_id=2 AND ordinal>0",
                []
            ),
            1
        );
    }

    #[tokio::test]
    async fn rebuild_cancel_switch_and_probe_restore_use_fake_embeddings() {
        let server = MockServer::start().await;
        let drift = Arc::new(AtomicBool::new(false));
        let response_drift = drift.clone();
        let unavailable = Arc::new(AtomicBool::new(false));
        let response_unavailable = unavailable.clone();
        Mock::given(method("POST"))
            .respond_with(move |req: &Request| {
                if response_unavailable.load(Ordering::SeqCst) {
                    return ResponseTemplate::new(503);
                }
                let body: serde_json::Value = req.body_json().unwrap();
                let flipped = response_drift.load(Ordering::SeqCst);
                let vector = if body["model"] == "text-embedding-3-large" || flipped {
                    vec![0.0, 1.0]
                } else {
                    vec![1.0, 0.0]
                };
                let inputs = body["input"]
                    .as_array()
                    .cloned()
                    .unwrap_or_else(|| vec![body["input"].clone()]);
                let data = inputs
                    .iter()
                    .enumerate()
                    .map(|(i, _)| serde_json::json!({"index":i,"embedding":vector}))
                    .collect::<Vec<_>>();
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"data":data,"usage":{"prompt_tokens":1}}))
            })
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rebuild.db");
        let _runtime = NetworkClients::new(&path);
        let conn = db::open_at(&path).unwrap();
        db::set_setting(&conn, "pi_auto_download", "0").unwrap();
        let provider = crate::test_net::fake_provider(&conn, "fake", server.uri());
        let bind = |model| {
            ai_store::set_task_binding(&conn, "embed", Some(provider), Some(model)).unwrap();
        };
        bind("text-embedding-3-small");
        conn.execute_batch("INSERT INTO feeds(id,url,created_at) VALUES(1,'https://fixture.invalid/feed',0);
            INSERT INTO articles(id,feed_id,title,summary,created_at) VALUES(1,1,'fixture','body',0);").unwrap();
        interests::insert_interest(&conn, "prototype", "manual").unwrap();
        memories::insert_memory(&conn, memories::MemoryKind::Semantic, "fact", "test", 1.0)
            .unwrap();
        let estimate = crate::queries::vector_space_status(&conn).unwrap();
        assert_eq!(estimate.estimate_chunks, 1);
        assert!(estimate.estimate_chars > 0);
        assert!(estimate.effective.is_none());
        assert!(!estimate.allow_model_change);
        rrss_core::model_pool::update_pool(
            &conn,
            &[
                rrss_core::model_pool::ModelRef::new(provider, "text-embedding-3-small"),
                rrss_core::model_pool::ModelRef::new(provider, "text-embedding-3-large"),
            ],
            &[],
            None,
        )
        .unwrap();
        let save_next = || {
            crate::actions::save_model_bindings(
                &conn,
                &crate::views::SaveModelBindingsArgs {
                    bindings: vec![crate::views::TaskModelInput {
                        task: "embed".into(),
                        provider_id: provider,
                        model: "text-embedding-3-large".into(),
                    }],
                },
            )
        };
        let client = reqwest::Client::new();
        let embed = || super::super::embeddings::run_embed_queue(&path, &client);
        embed().await.unwrap();
        conn.execute_batch("INSERT INTO vector_spaces(id,base_url,model,dims,prefix_version,normalized,state,created_at) VALUES(9,'','legacy',2,1,0,'retired',0);
            INSERT INTO articles(id,feed_id,title,pruned_at,created_at) VALUES(2,1,'pruned',1,0);
            INSERT INTO chunks(article_id,ordinal,kind,text,embedding,model,space_id,source_hash) VALUES(2,0,'title_summary','pruned',X'0000803f00000000','legacy',9,'fixture');").unwrap();
        let old = spaces::active(&conn).unwrap().unwrap();
        let original = rag::title_embedding(&conn, 1).unwrap().unwrap();
        let requests = server.received_requests().await.unwrap().len();
        embed().await.unwrap();
        assert_eq!(server.received_requests().await.unwrap().len(), requests); // 6 小时节流。
        assert!(matches!(
            save_next(),
            Err(rrss_core::Error::VectorSpaceRebuildRequired { .. })
        ));
        assert_eq!(
            ai_store::resolve_embed(&conn).unwrap().unwrap().model,
            "text-embedding-3-small"
        );
        db::set_setting_enabled(&conn, spaces::ALLOW_MODEL_CHANGE, true).unwrap();
        save_next().unwrap();
        let first = spaces::building(&conn).unwrap().unwrap();
        check(&path, &client, false).await.unwrap();
        let (kind, row_id, text) = spaces::pending_rebuild(&conn, first.id)
            .unwrap()
            .into_iter()
            .find(|(kind, _, _)| kind == "interest")
            .unwrap();
        let first = spaces::building(&conn).unwrap().unwrap();
        spaces::stage(&conn, &first, &kind, row_id, Some(&text), &[0.0, 1.0]).unwrap();
        let blob: Vec<u8> = conn
            .query_row(
                "SELECT embedding FROM vector_staging WHERE space_id=?1",
                [first.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(blob, rag::f32s_to_le(&[0.0, 1.0]));
        conn.execute("UPDATE interests SET label='revised' WHERE id=?1", [row_id])
            .unwrap();
        spaces::stage(&conn, &first, &kind, row_id, Some(&text), &[0.0, 1.0]).unwrap();
        assert_eq!(spaces::rebuild_progress(&conn, first.id).unwrap(), (0, 3));
        spaces::stage(&conn, &first, &kind, row_id, Some("revised"), &[0.0, 1.0]).unwrap();
        assert!(!spaces::finish_rebuild(&conn, first.id).unwrap());
        assert_eq!(rag::title_embedding(&conn, 1).unwrap().unwrap(), original);
        let progress = crate::queries::vector_space_status(&conn)
            .unwrap()
            .building
            .unwrap();
        assert_eq!(progress.written, 1);
        assert!(progress.total > progress.written);
        crate::actions::cancel_vector_rebuild(&conn).unwrap();
        assert!(spaces::building(&conn).unwrap().is_none());
        assert_eq!(
            integer(
                &conn,
                "SELECT COUNT(*) FROM vector_staging WHERE space_id=?1",
                [first.id]
            ),
            0
        );
        assert_eq!(spaces::active(&conn).unwrap().unwrap().id, old.id);
        assert_eq!(
            ai_store::resolve_embed(&conn).unwrap().unwrap().model,
            "text-embedding-3-small"
        );
        assert_eq!(rag::title_embedding(&conn, 1).unwrap().unwrap(), original);
        assert_eq!(
            integer(
                &conn,
                "SELECT COUNT(*) FROM vector_space_probes WHERE space_id=?1",
                [first.id]
            ),
            0
        );
        bind("text-embedding-3-large");
        conn.execute_batch(
            "CREATE TRIGGER fail_activation BEFORE UPDATE OF state ON vector_spaces
            WHEN NEW.state='active' BEGIN SELECT RAISE(ABORT,'activation fixture'); END;",
        )
        .unwrap();
        assert!(embed().await.is_err());
        assert_eq!(spaces::active(&conn).unwrap().unwrap().id, old.id);
        assert_eq!(rag::title_embedding(&conn, 1).unwrap().unwrap(), original);
        let building = spaces::building(&conn).unwrap().unwrap();
        assert_eq!(
            spaces::rebuild_progress(&conn, building.id).unwrap(),
            (3, 3)
        );
        conn.execute_batch("DROP TRIGGER fail_activation;").unwrap();
        embed().await.unwrap();
        let active = spaces::active(&conn).unwrap().unwrap();
        assert_ne!(active.id, old.id);
        assert!(spaces::building(&conn).unwrap().is_none());
        assert_eq!(active.model, "text-embedding-3-large");
        assert_eq!(
            rag::title_embedding(&conn, 1).unwrap().unwrap(),
            vec![0.0, 1.0]
        );
        assert_eq!(
            conn.query_row(
                "SELECT state FROM vector_spaces WHERE id=?1",
                [old.id],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            "retired"
        );
        for table in ["chunks", "interests", "memories"] {
            assert_eq!(
                integer(
                    &conn,
                    &format!(
                        "SELECT COUNT(*) FROM {table} WHERE space_id IS NOT ?1 {}",
                        if table == "chunks" {
                            "AND article_id=1"
                        } else {
                            ""
                        }
                    ),
                    [active.id],
                ),
                0
            );
        }
        // 已覆盖的旧向量不归档；只有未重建的 retired 段仍需清理。
        assert_eq!(storage::old_vector_count(&conn).unwrap(), 1);
        conn.execute("INSERT INTO vector_staging(space_id,kind,row_id,embedding,input_text) VALUES(9,'chunk',99,X'0000803f00000000','leftover')",[]).unwrap();
        assert_eq!(storage::old_vector_count(&conn).unwrap(), 2);
        assert_eq!(
            storage::clean(&conn, dir.path(), storage::Cleanup::OldVectors, None).unwrap(),
            2
        );
        assert_eq!(storage::old_vector_count(&conn).unwrap(), 0);
        assert_eq!(integer(&conn, "SELECT COUNT(*) FROM vector_staging", []), 0);
        // 切回原模型，探针漂移后暂停写入，仍保留检索向量。
        bind("text-embedding-3-small");
        embed().await.unwrap();
        drift.store(true, Ordering::SeqCst);
        assert!(!check(&path, &client, true).await.unwrap());
        let stale = crate::queries::vector_space_status(&conn)
            .unwrap()
            .effective
            .unwrap();
        assert_eq!(stale.state, "stale");
        assert!(stale.stale_reason.is_some());
        assert!(stale.checked_at.is_some());
        let requests = server.received_requests().await.unwrap().len();
        conn.execute("UPDATE interests SET embedding=NULL", [])
            .unwrap();
        let error = embed().await.unwrap_err();
        assert_eq!(
            crate::scheduler::background::pause_reason(&error),
            "vector_space"
        );
        assert_eq!(server.received_requests().await.unwrap().len(), requests);
        assert_eq!(rag::title_embedding(&conn, 1).unwrap().unwrap(), original);
        conn.execute(
            "UPDATE ai_providers SET base_url='https://changed.invalid' WHERE id=?1",
            params![provider],
        )
        .unwrap();
        drift.store(false, Ordering::SeqCst);
        let restored = restore(&path, &crate::network::NetworkClients::new(&path))
            .await
            .unwrap();
        assert_eq!(restored.effective.unwrap().state, "active");
        assert_eq!(
            ai_store::resolve_embed(&conn).unwrap().unwrap().base_url,
            server.uri()
        );
        unavailable.store(true, Ordering::SeqCst);
        assert!(check(&path, &client, true).await.is_err());
        let requests = server.received_requests().await.unwrap().len();
        assert!(check(&path, &client, false).await.is_err());
        assert_eq!(server.received_requests().await.unwrap().len(), requests);
        unavailable.store(false, Ordering::SeqCst);
        assert!(
            check(&path, &client, true).await.is_err(),
            "explicit checks respect service cooldown"
        );
        tokio::time::pause();
        tokio::time::advance(std::time::Duration::from_secs(3600)).await;
        tokio::time::resume();
        assert!(check(&path, &client, true).await.unwrap());
    }
}
