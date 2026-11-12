//! 连接与模型的显式测试（阶段 19-7）。
//!
//! - 「测试」连接只请求一次模型列表，检查地址与密钥；结果记到连接上。
//! - 「测试能力」按模型调用一次，结果存进模型记录，不再放在会过期、会被清空的 `ai_cache` 里。
//!
//! 已保存的连接由宿主按 id 取库里的密钥，前端拿不到完整密钥（阶段 19-9）。

use std::path::Path;
use std::time::Instant;

use reqwest::Client;
use rrss_ai::Protocol;
use rrss_core::error::Error;
use rrss_core::model_pool::{self, Caps, ModelRef, Probe, Support};
use rrss_core::{ai_store, db};

use crate::views::{ConnectionTestArgs, ConnectionTestView, ModelPoolView};

/// HTTP(S) 根地址，不含凭据、查询参数或片段。错误里不带原始地址。
pub fn validate_base_url(raw: &str) -> rrss_core::Result<String> {
    let url = reqwest::Url::parse(raw.trim())
        .map_err(|_| Error::InvalidProvider("连接地址必须是有效的 HTTP(S) 地址".into()))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(Error::InvalidProvider(
            "连接地址须为 HTTP(S)，且不能包含凭据、查询参数或片段".into(),
        ));
    }
    Ok(raw.trim().to_string())
}

/// 草稿给了密钥就用草稿的；否则已保存的连接用库里的。
fn resolve_key(
    db_path: &Path,
    provider_id: i64,
    draft: Option<&str>,
) -> rrss_core::Result<Option<String>> {
    if let Some(key) = draft.map(str::trim).filter(|k| !k.is_empty()) {
        return Ok(Some(key.to_string()));
    }
    if provider_id <= 0 {
        return Ok(None);
    }
    let conn = db::open_at(db_path)?;
    Ok(ai_store::get_provider(&conn, provider_id)?
        .api_key
        .filter(|k| !k.trim().is_empty()))
}

/// 取模型列表；草稿没改地址与密钥时和已保存的连接一样。
pub async fn list_models(
    db_path: &Path,
    client: &Client,
    provider_id: i64,
    base_url: &str,
    api_key: Option<&str>,
) -> anyhow::Result<Vec<String>> {
    let base_url = validate_base_url(base_url)?;
    let api_key = api_key.map(str::to_owned);
    let db_path = db_path.to_path_buf();
    let key =
        crate::execution::blocking(move || resolve_key(&db_path, provider_id, api_key.as_deref()))
            .await??;
    Ok(rrss_ai::list_models(client, &base_url, key.as_deref()).await?)
}

/// 「测试」连接。测的是已保存的地址与密钥时，把结果记到连接上。
pub async fn test_connection(
    db_path: &Path,
    client: &Client,
    args: ConnectionTestArgs,
) -> rrss_core::Result<ConnectionTestView> {
    let base_url = validate_base_url(&args.base_url)?;
    let prepare_path = db_path.to_path_buf();
    let (saved, key) = crate::execution::blocking(move || -> rrss_core::Result<_> {
        let draft_key = args
            .api_key
            .as_deref()
            .map(str::trim)
            .filter(|k| !k.is_empty());
        let saved = if args.provider_id > 0 {
            let conn = db::open_at(&prepare_path)?;
            let p = ai_store::get_provider(&conn, args.provider_id)?;
            (p.base_url.trim() == args.base_url.trim() && draft_key.is_none()).then_some(p.id)
        } else {
            None
        };
        Ok((
            saved,
            resolve_key(&prepare_path, args.provider_id, draft_key)?,
        ))
    })
    .await
    .map_err(std::io::Error::other)??;
    let started = Instant::now();
    let result = rrss_ai::list_models(client, &base_url, key.as_deref()).await;
    let latency_ms = started.elapsed().as_millis() as u64;
    let view = match result {
        Ok(models) => ConnectionTestView {
            ok: true,
            latency_ms,
            models: models.len() as i64,
            error: String::new(),
        },
        Err(err) => ConnectionTestView {
            ok: false,
            latency_ms,
            models: 0,
            error: format!("{err:#}"),
        },
    };
    if let Some(id) = saved {
        let note = if view.ok {
            view.models.to_string()
        } else {
            view.error.clone()
        };
        let ok = view.ok;
        crate::execution::core_db(db_path.to_path_buf(), move |conn| {
            ai_store::set_provider_test(conn, id, ok, &note)
        })
        .await?;
    }
    Ok(view)
}

fn support(s: rrss_ai::Support) -> Support {
    match s {
        rrss_ai::Support::Supported => Support::Supported,
        rrss_ai::Support::Unsupported => Support::Unsupported,
        rrss_ai::Support::Unknown => Support::Unknown,
    }
}

/// 「测试能力」：调用一次模型，结果存进模型记录（图片输入不自动测）。
pub async fn probe_pool_model(
    db_path: &Path,
    client: &Client,
    provider_id: i64,
    model: &str,
) -> rrss_core::Result<ModelPoolView> {
    let r = ModelRef::new(provider_id, model);
    let read_ref = r.clone();
    let (provider, previous) = crate::execution::core_db(db_path.to_path_buf(), move |conn| {
        let previous = model_pool::get(conn, &read_ref)?
            .ok_or_else(|| Error::InvalidModelChoice(format!("{} 不在模型列表里", read_ref.model)))?
            .probe;
        Ok((ai_store::get_provider(conn, provider_id)?, previous))
    })
    .await?;
    let report = rrss_ai::probe_capabilities(
        client,
        provider.base_url.trim(),
        provider.api_key.as_deref().filter(|k| !k.trim().is_empty()),
        &r.model,
        Protocol::parse(&provider.protocol),
    )
    .await;
    let probe = Probe {
        caps: Caps {
            tool_calling: support(report.tool_calling),
            structured_output: support(report.structured_output),
            image_input: previous.map(|p| p.caps.image_input).unwrap_or_default(),
            reasoning: support(report.reasoning),
        },
        tested_at: Some(rrss_core::feeds::now_ms()),
        recommended_compat: report.recommended_compat,
    };
    crate::execution::core_db(db_path.to_path_buf(), move |conn| {
        model_pool::set_probe(conn, &r, &probe)?;
        crate::model_pool::model_pool(conn)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use rrss_core::ai_store::NewProvider;
    use wiremock::matchers::{body_partial_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn saved(conn: &rusqlite::Connection, base_url: &str, key: Option<&str>) -> i64 {
        ai_store::insert_provider(
            conn,
            &NewProvider {
                name: "saved".into(),
                base_url: base_url.into(),
                api_key: key.map(Into::into),
                protocol: "chat".into(),
            },
        )
        .unwrap()
    }

    #[tokio::test]
    async fn model_list_and_saved_test_errors_clean_credentials_and_keep_the_error_type() {
        let server = MockServer::start().await;
        let key = "k\"\\z";
        let body = format!(
            "rejected {key}\n{}\nauthorization: bearer other-canary\nretry later",
            serde_json::json!({"detail": key})
        );
        Mock::given(method("GET"))
            .and(header("authorization", format!("Bearer {key}")))
            .respond_with(ResponseTemplate::new(401).set_body_string(body))
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let conn = db::open_at(&db_path).unwrap();
        let base = format!("{}/v1", server.uri());
        let id = saved(&conn, &base, Some(&format!(" {key} ")));
        let client = Client::new();
        let assert_clean = |text: &str| {
            assert!(text.contains("401") && text.contains("\nretry later"));
            assert!(
                !text.contains(key)
                    && !text.contains(r#"k\"\\z"#)
                    && !text.contains("other-canary")
            );
        };
        let view = test_connection(
            &db_path,
            &client,
            ConnectionTestArgs {
                provider_id: id,
                base_url: base.clone(),
                api_key: None,
            },
        )
        .await
        .unwrap();
        assert!(!view.ok);
        assert_clean(&view.error);
        assert_clean(
            &ai_store::get_provider(&conn, id)
                .unwrap()
                .last_test
                .unwrap()
                .note,
        );
        for error in [
            list_models(&db_path, &client, id, &base, None)
                .await
                .unwrap_err(),
            crate::model_pool::fetch_model_catalog(&db_path, &client, id)
                .await
                .unwrap_err(),
        ] {
            assert_clean(&format!("{error:#}"));
            assert!(matches!(
                error.downcast_ref::<rrss_ai::Error>(),
                Some(rrss_ai::Error::Http { status: 401, .. })
            ));
            // 类型仍可被日志识别，不把服务端自由文本带回日志。
            assert_eq!(crate::log::error(error.as_ref()), "http status 401");
        }
    }

    #[tokio::test]
    async fn a_saved_connection_is_tested_with_its_stored_key_and_remembers_the_result() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .and(header("authorization", "Bearer stored-secret-key"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"data": [{"id": "a"}, {"id": "b"}]})),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(401).set_body_string("bad key"))
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("rrss.db");
        let conn = db::open_at(&db_path).unwrap();
        let base = format!("{}/v1", server.uri());
        let id = saved(&conn, &base, Some("stored-secret-key"));
        let client = Client::new();
        let args = |key: Option<&str>| ConnectionTestArgs {
            provider_id: id,
            base_url: base.clone(),
            api_key: key.map(Into::into),
        };

        let ok = test_connection(&db_path, &client, args(None))
            .await
            .unwrap();
        assert!(ok.ok && ok.models == 2, "{ok:?}");
        let test = ai_store::get_provider(&conn, id)
            .unwrap()
            .last_test
            .unwrap();
        assert!(test.ok && test.note == "2");

        // 草稿换了密钥：结果只给这次编辑，不记到连接上。
        let bad = test_connection(&db_path, &client, args(Some("draft-other-key")))
            .await
            .unwrap();
        assert!(!bad.ok && !bad.error.contains("draft-other-key"), "{bad:?}");
        assert!(
            ai_store::get_provider(&conn, id)
                .unwrap()
                .last_test
                .unwrap()
                .ok
        );

        let models = list_models(&db_path, &client, id, &base, None)
            .await
            .unwrap();
        assert_eq!(models, ["a", "b"]);
        assert!(
            test_connection(
                &db_path,
                &client,
                ConnectionTestArgs {
                    provider_id: 0,
                    base_url: "https://user:pw@example.com/v1".into(),
                    api_key: None,
                }
            )
            .await
            .is_err()
        );
    }

    #[tokio::test]
    async fn probing_a_model_stores_its_capabilities_on_the_model() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .and(body_partial_json(
                serde_json::json!({"response_format":{"type":"json_schema"}}),
            ))
            .respond_with(ResponseTemplate::new(400).set_body_string("response_format unsupported"))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "choices":[{"message":{"reasoning_content":"hm","tool_calls":[{"function":{"name":"rrss_probe"}}]}}]
            })))
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("rrss.db");
        let conn = db::open_at(&db_path).unwrap();
        let id = saved(&conn, &format!("{}/v1", server.uri()), None);
        assert!(
            probe_pool_model(&db_path, &Client::new(), id, "m")
                .await
                .is_err(),
            "not added yet"
        );
        model_pool::update_pool(&conn, &[ModelRef::new(id, "m")], &[], None).unwrap();

        let view = probe_pool_model(&db_path, &Client::new(), id, "m")
            .await
            .unwrap();
        let e = &view.pool[0];
        use crate::views::CapabilitySupport as S;
        assert_eq!(
            (
                e.detected.tool_calling,
                e.detected.structured_output,
                e.detected.reasoning,
                e.detected.image_input
            ),
            (S::Supported, S::Unsupported, S::Supported, S::Unknown)
        );
        assert!(e.tested_at.is_some());
        // 清空 AI 缓存不影响测试结果。
        conn.execute("DELETE FROM ai_cache", []).unwrap();
        let again = crate::model_pool::model_pool(&conn).unwrap();
        assert_eq!(again.pool[0].detected.tool_calling, S::Supported);
    }
}
