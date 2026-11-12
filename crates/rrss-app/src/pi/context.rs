//! 引擎报告的模型上下文窗口。仅在已经启动的进程握手后缓存，不为查询启动进程。
use rusqlite::Connection;
use serde_json::Value;

const WINDOWS_KEY: &str = "pi_engine_context_windows";

pub fn cache_windows(conn: &Connection, models: &Value) -> anyhow::Result<()> {
    let windows: std::collections::BTreeMap<String, i64> = models
        .get("models")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|m| {
            let provider = m.get("provider")?.as_str()?;
            let id = m.get("id")?.as_str()?;
            let window = m.get("contextWindow")?.as_i64().filter(|w| *w > 0)?;
            Some((format!("{provider}/{id}"), window))
        })
        .collect();
    rrss_core::db::set_setting(conn, WINDOWS_KEY, &serde_json::to_string(&windows)?)?;
    Ok(())
}

pub fn engine_window(conn: &Connection, model: &str) -> rrss_core::Result<Option<i64>> {
    let windows: std::collections::BTreeMap<String, i64> =
        rrss_core::db::get_setting(conn, WINDOWS_KEY)?
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
    Ok(windows.get(model).copied().filter(|w| *w > 0))
}

pub fn window(conn: &Connection, model: &str) -> rrss_core::Result<Option<i64>> {
    if let Some((provider, id)) = model.strip_prefix("rrss-").and_then(|m| m.split_once('/'))
        && let Ok(provider) = provider.parse::<i64>()
        && let Some(entry) = rrss_core::model_pool::pool(conn)?
            .into_iter()
            .find(|e| e.provider_id == provider && e.model == id)
        && let Some(window) = entry.context_window
    {
        return Ok(Some(window));
    }
    engine_window(conn, model)
}

/// 当前上下文统计与累计账单严格分开；null / 缺字段时交给最后一条助手 usage 回退。
pub fn current_used(stats: &Value) -> Option<i64> {
    stats
        .get("contextUsage")?
        .get("tokens")?
        .as_i64()
        .filter(|n| *n >= 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn current_context_never_uses_cumulative_billing() {
        assert_eq!(
            current_used(&json!({"contextUsage":{"tokens":123}, "tokens":{"total":999}})),
            Some(123)
        );
        assert_eq!(
            current_used(&json!({"contextUsage":{"tokens":null}, "tokens":{"total":999}})),
            None
        );
        assert_eq!(current_used(&json!({"tokens":{"total":999}})), None);
    }
    #[test]
    fn windows_prefer_user_value_then_engine_and_never_start_a_process() {
        let dir = tempfile::tempdir().unwrap();
        let conn = rrss_core::db::open_at(&dir.path().join("db")).unwrap();
        assert_eq!(window(&conn, "rrss-1/sample").unwrap(), None);
        cache_windows(
            &conn,
            &json!({"models":[{"provider":"rrss-1","id":"sample","contextWindow":32000}]}),
        )
        .unwrap();
        assert_eq!(window(&conn, "rrss-1/sample").unwrap(), Some(32000));
        conn.execute("INSERT INTO ai_providers (id, name, base_url, api_key, protocol, model, created_at) VALUES (1,'test','http://localhost','','chat','',1)", []).unwrap();
        rrss_core::model_pool::update_pool(
            &conn,
            &[rrss_core::model_pool::ModelRef::new(1, "sample")],
            &[],
            None,
        )
        .unwrap();
        rrss_core::model_pool::update_entry(
            &conn,
            &rrss_core::model_pool::ModelRef::new(1, "sample"),
            &rrss_core::model_pool::EntryPatch {
                context_window: Some(Some(64000)),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(window(&conn, "rrss-1/sample").unwrap(), Some(64000));
    }
}
