//! 加进来的模型与模型目录的查询、写入和获取（阶段 19-6 / 19-7）。存储在 `rrss_core::model_pool`。

use std::path::Path;

use reqwest::Client;
use rrss_core::ai_store;
use rrss_core::model_pool::{self, EntryPatch, ModelRef, RemoveOutcome};
use rusqlite::Connection;

use crate::views::{
    ModelCatalogView, ModelChoice, ModelPoolView, PoolEntryView, SaveModelPoolArgs,
    UpdatePoolModelArgs,
};

fn entry_view(
    e: model_pool::PoolEntry,
    uses: Vec<String>,
    engine_context_window: Option<i64>,
) -> PoolEntryView {
    let effective = e.effective().into();
    let detected = e.probe.as_ref().map(|p| p.caps).unwrap_or_default().into();
    PoolEntryView {
        chat: e.chat,
        thinking: e.thinking_setting(),
        context_window: e.context_window,
        engine_context_window,
        max_output: e.max_output,
        compat_json: e.compat.as_ref().map(ToString::to_string),
        detected,
        overrides: e.overrides.into(),
        effective,
        tested_at: e.probe.as_ref().and_then(|p| p.tested_at),
        recommended_compat_json: e
            .probe
            .as_ref()
            .filter(|p| {
                p.recommended_compat
                    .as_object()
                    .is_some_and(|o| !o.is_empty())
            })
            .map(|p| p.recommended_compat.to_string()),
        uses,
        display_name: e.display_name,
        provider_id: e.provider_id,
        model: e.model,
    }
}

fn view(conn: &Connection, outcome: RemoveOutcome) -> rrss_core::Result<ModelPoolView> {
    let bindings = ai_store::list_task_bindings(conn)?;
    let pool = model_pool::pool(conn)?
        .into_iter()
        .map(|e| {
            let uses = ai_store::ALL_TASKS
                .iter()
                .filter(|task| {
                    bindings.iter().any(|b| {
                        b.task == **task
                            && b.provider_id == Some(e.provider_id)
                            && b.model.as_deref().map(str::trim) == Some(e.model.as_str())
                    })
                })
                .map(|task| (*task).to_string())
                .collect();
            let engine_window = crate::pi::context::engine_window(
                conn,
                &format!("rrss-{}/{}", e.provider_id, e.model),
            )?;
            Ok(entry_view(e, uses, engine_window))
        })
        .collect::<rrss_core::Result<Vec<_>>>()?;
    let catalogs = model_pool::catalogs(conn)?
        .into_iter()
        .map(|c| ModelCatalogView {
            provider_id: c.provider_id,
            models: c.models,
            fetched_at: c.fetched_at,
        })
        .collect();
    Ok(ModelPoolView {
        pool,
        catalogs,
        replaced: outcome.replaced,
        unset: outcome.unset,
    })
}

pub fn model_pool(conn: &Connection) -> rrss_core::Result<ModelPoolView> {
    view(conn, RemoveOutcome::default())
}

fn refs(list: &[ModelChoice]) -> Vec<ModelRef> {
    list.iter()
        .map(|c| ModelRef::new(c.provider_id, &c.model))
        .collect()
}

pub fn save_model_pool(
    conn: &Connection,
    args: &SaveModelPoolArgs,
) -> rrss_core::Result<ModelPoolView> {
    let replace = args
        .replace
        .as_ref()
        .map(|c| ModelRef::new(c.provider_id, &c.model));
    crate::configuration::write(conn, |tx| {
        let outcome = model_pool::update_pool_in_transaction(
            tx,
            &refs(&args.add),
            &refs(&args.remove),
            replace.as_ref(),
        )?;
        view(tx, outcome)
    })
}

pub fn update_pool_model(
    conn: &Connection,
    args: &UpdatePoolModelArgs,
) -> rrss_core::Result<ModelPoolView> {
    let clearable = |v: Option<i64>| v.map(|n| (n != 0).then_some(n));
    let compat = match args.compat_json.as_deref() {
        None => None,
        Some(raw) if raw.trim().is_empty() => Some(None),
        Some(raw) => Some(Some(serde_json::from_str(raw).map_err(|_| {
            rrss_core::Error::InvalidCompat("兼容设置必须是 JSON 对象".into())
        })?)),
    };
    model_pool::update_entry(
        conn,
        &ModelRef::new(args.provider_id, &args.model),
        &EntryPatch {
            chat: args.chat,
            display_name: args.display_name.clone(),
            thinking: args.thinking.clone(),
            context_window: clearable(args.context_window),
            max_output: clearable(args.max_output),
            compat,
            overrides: args.overrides.map(Into::into),
        },
    )?;
    model_pool(conn)
}

/// 用户显式「获取模型列表」：成功后整体覆盖该连接的目录；失败时目录不动，返回已清洗凭据的错误。
pub async fn fetch_model_catalog(
    db_path: &Path,
    client: &Client,
    provider_id: i64,
) -> anyhow::Result<ModelPoolView> {
    let provider = crate::execution::core_db(db_path.to_path_buf(), move |conn| {
        ai_store::get_provider(conn, provider_id)
    })
    .await?;
    let models = rrss_ai::list_models(
        client,
        provider.base_url.trim(),
        provider.api_key.as_deref().filter(|k| !k.trim().is_empty()),
    )
    .await?;
    crate::execution::with_db(db_path.to_path_buf(), move |conn| {
        model_pool::replace_catalog(conn, provider_id, &models)?;
        Ok(model_pool(conn)?)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use rrss_core::ai_store::NewProvider;
    use rrss_core::db;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn provider(conn: &Connection, base_url: String) -> i64 {
        ai_store::insert_provider(
            conn,
            &NewProvider {
                name: "p".into(),
                base_url,
                api_key: None,
                protocol: "chat".into(),
            },
        )
        .unwrap()
    }

    #[tokio::test]
    async fn a_failed_fetch_keeps_the_remembered_catalog() {
        let server = MockServer::start().await;
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("rrss.db");
        let conn = db::open_at(&db_path).unwrap();
        let id = provider(&conn, format!("{}/v1", server.uri()));
        let client = Client::new();
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"data": [{"id": "b"}, {"id": "a"}]})),
            )
            .up_to_n_times(1)
            .mount(&server)
            .await;
        let view = fetch_model_catalog(&db_path, &client, id).await.unwrap();
        assert_eq!(view.catalogs[0].models, ["a", "b"]);

        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;
        assert!(fetch_model_catalog(&db_path, &client, id).await.is_err());
        assert_eq!(model_pool(&conn).unwrap().catalogs[0].models, ["a", "b"]);
    }

    #[test]
    fn the_view_lists_uses_and_edits_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let conn = db::open_at(&dir.path().join("rrss.db")).unwrap();
        let id = provider(&conn, "http://p/v1".into());
        let choice = |m: &str| ModelChoice {
            provider_id: id,
            model: m.into(),
        };
        save_model_pool(
            &conn,
            &SaveModelPoolArgs {
                add: vec![choice("x"), choice("y")],
                remove: vec![],
                replace: None,
            },
        )
        .unwrap();
        assert!(
            model_pool(&conn)
                .unwrap()
                .pool
                .iter()
                .all(|e| e.engine_context_window.is_none())
        );
        crate::pi::context::cache_windows(&conn, &serde_json::json!({"models":[{"provider":format!("rrss-{id}"),"id":"y","contextWindow":16000}]})).unwrap();
        assert_eq!(
            model_pool(&conn)
                .unwrap()
                .pool
                .iter()
                .find(|e| e.model == "y")
                .unwrap()
                .engine_context_window,
            Some(16000)
        );
        ai_store::set_task_binding(&conn, ai_store::TASK_FOCUS_SUMMARY, Some(id), Some("x"))
            .unwrap();
        let v = update_pool_model(
            &conn,
            &UpdatePoolModelArgs {
                provider_id: id,
                model: "y".into(),
                chat: Some(true),
                display_name: Some("Y".into()),
                thinking: Some("low".into()),
                context_window: Some(32_000),
                max_output: None,
                compat_json: Some(r#"{"supportsStrictMode":false}"#.into()),
                overrides: None,
            },
        )
        .unwrap();
        let x = v.pool.iter().find(|e| e.model == "x").unwrap();
        assert_eq!(x.uses, [ai_store::TASK_FOCUS_SUMMARY]);
        let y = v.pool.iter().find(|e| e.model == "y").unwrap();
        assert!(y.chat && y.display_name == "Y" && y.thinking == "low");
        assert_eq!(y.context_window, Some(32_000));
        assert_eq!(y.engine_context_window, Some(16000));
        assert_eq!(
            y.compat_json.as_deref(),
            Some(r#"{"supportsStrictMode":false}"#)
        );
        // 0 与空白清空。
        let v = update_pool_model(
            &conn,
            &UpdatePoolModelArgs {
                provider_id: id,
                model: "y".into(),
                chat: None,
                display_name: None,
                thinking: None,
                context_window: Some(0),
                max_output: None,
                compat_json: Some(" ".into()),
                overrides: None,
            },
        )
        .unwrap();
        let y = v.pool.iter().find(|e| e.model == "y").unwrap();
        assert_eq!((y.context_window, y.compat_json.as_deref()), (None, None));

        let v = save_model_pool(
            &conn,
            &SaveModelPoolArgs {
                add: vec![],
                remove: vec![choice("x")],
                replace: Some(choice("y")),
            },
        )
        .unwrap();
        assert_eq!(v.replaced, [ai_store::TASK_FOCUS_SUMMARY]);
        assert_eq!(v.pool.len(), 1);
        assert_eq!(model_pool(&conn).unwrap().replaced, Vec::<String>::new());
    }
}
