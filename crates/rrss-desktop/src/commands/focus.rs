use rrss_app::queries;
use rrss_app::views::*;

use crate::state::{AppState, with_db};

#[tauri::command]
pub async fn focus_stream(
    state: tauri::State<'_, AppState>,
    expanded_cluster: Option<i64>,
    exclude_ids: Option<Vec<i64>>,
    exclude_cluster_ids: Option<Vec<i64>>,
    more: Option<bool>,
    scope: Option<String>,
) -> Result<FocusPage, String> {
    let path = state.db_path.clone();
    let exclude_ids = exclude_ids.unwrap_or_default();
    let exclude_cluster_ids = exclude_cluster_ids.unwrap_or_default();
    let more = more.unwrap_or(false);
    let scope = scope.unwrap_or_else(|| "all".to_string());
    with_db("focus_stream", path, move |conn| {
        let scope = rrss_core::feed_scope::resolve(
            conn,
            rrss_core::feed_scope::FeedScope::parse_str(&scope),
        )
        .map_err(|e| e.to_string())?;
        queries::focus_page(
            conn,
            expanded_cluster,
            &exclude_ids,
            &exclude_cluster_ids,
            more,
            scope,
        )
        .map_err(|e| e.to_string())
    })
    .await
}

db_task!(set_interest_enabled(id: i64, enabled: bool) -> () =>
    |conn| rrss_app::actions::set_interest_enabled(conn, id, enabled));

db_task!(delete_interest(id: i64) -> () => |conn| rrss_app::actions::delete_interest(conn, id));

db_task!(add_interest(label: String) -> i64 =>
    |conn| rrss_app::actions::add_interest(conn, &label));

db_task!(interest_hits(id: i64) -> Vec<InterestHitView> => |conn| Ok::<_, rrss_core::Error>(queries::interest_hits(conn, id)));

db_task!(ranking_settings() -> RankingSettings => queries::ranking_settings);

db_task!(focus_cluster_members(cluster_id: i64, scope: Option<String>) -> Vec<ArticleView> =>
|conn| {
    let scope = rrss_core::feed_scope::resolve(
        conn,
        rrss_core::feed_scope::FeedScope::parse_str(scope.as_deref().unwrap_or("all")),
    )?;
    queries::focus_cluster_members(conn, cluster_id, scope)
});

db_task!(mark_cluster_read(cluster_id: i64, scope: Option<String>) -> Vec<i64> =>
    |conn| rrss_app::actions::mark_cluster_read(conn, cluster_id, scope.as_deref()));

db_task!(
    /// 相近报道综述：存着的结果与新旧状态，不调用模型。
    cluster_summary(cluster_id: i64, scope: Option<String>) -> ClusterSummaryView =>
        |conn| queries::cluster_summary(conn, cluster_id, scope.as_deref())
);

/// 生成（或更新）这组的综述，只在用户点击时调用；结束后读回最新状态。
#[tauri::command]
pub async fn generate_cluster_summary(
    state: tauri::State<'_, AppState>,
    cluster_id: i64,
    scope: Option<String>,
) -> Result<ClusterSummaryView, String> {
    tracing::info!(cluster_id, "manual cluster summary requested");
    state
        .scheduler
        .summarize_cluster(cluster_id, scope.as_deref().unwrap_or("all"))
        .await
        .map_err(|e| {
            tracing::warn!(cluster_id, error = %rrss_app::log::error(e.as_ref()), "manual cluster summary failed");
            e.to_string()
        })?;
    let path = state.db_path.clone();
    with_db("generate_cluster_summary", path, move |conn| {
        queries::cluster_summary(conn, cluster_id, scope.as_deref()).map_err(|e| e.to_string())
    })
    .await
}

command_handler![
    focus_stream,
    interest_hits,
    set_interest_enabled,
    delete_interest,
    add_interest,
    ranking_settings,
    focus_cluster_members,
    mark_cluster_read,
    cluster_summary,
    generate_cluster_summary,
];
