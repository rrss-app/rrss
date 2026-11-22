use tauri::State;

use rrss_app::queries;
use rrss_app::views::*;

use crate::state::AppState;

/// Trees carry each feed's rate-limit end, which lives in the scheduler, not the database.
async fn tree_task(
    state: &State<'_, AppState>,
    name: &'static str,
    task: impl FnOnce(&rrss_core::db::PooledConnection) -> rrss_core::Result<FeedTree> + Send + 'static,
) -> Result<FeedTree, UiError> {
    let mut tree = crate::state::with_db(name, state.db_path.clone(), move |conn| {
        task(conn).map_err(UiError::from)
    })
    .await?;
    state.scheduler.annotate_limits(&mut tree);
    Ok(tree)
}

#[tauri::command]
pub async fn feed_tree(state: State<'_, AppState>) -> Result<FeedTree, UiError> {
    tree_task(&state, "feed_tree", |conn| queries::feed_tree(conn)).await
}

db_task!(feeds_page(scope: String, list_scope: String, cursor: Option<FeedsCursor>) -> FeedsPage =>
    |conn| queries::feeds_page(conn, &scope, &list_scope, cursor));

db_task!(feed_counts() -> FeedCountsView => queries::feed_counts);

#[tauri::command]
pub async fn discover_feeds(
    state: State<'_, AppState>,
    request: DiscoverFeedsRequest,
) -> Result<DiscoverFeedsResult, DiscoverFeedsError> {
    state
        .discovery
        .run(&state.network, &state.db_path, request)
        .await
}

db_task!(mark_scope_read(scope: String) -> Vec<i64> =>
    |conn| rrss_app::actions::mark_scope_read(conn, &scope));

#[tauri::command]
pub async fn add_feed(state: State<'_, AppState>, url: String) -> Result<AddFeedResult, UiError> {
    state.scheduler.add_feed(&url).await.map_err(UiError::from)
}

#[tauri::command]
pub async fn save_feed(
    state: State<'_, AppState>,
    args: SaveFeedArgs,
) -> Result<FeedTree, UiError> {
    let interval_changed = args.interval.is_some();
    let tree = tree_task(&state, "save_feed", move |conn| {
        rrss_app::actions::save_feed(conn, &args)
    })
    .await?;
    if interval_changed {
        state.scheduler.reschedule();
    }
    Ok(tree)
}

db_task!(unsubscribe(id: i64) -> () => |conn| rrss_app::actions::unsubscribe(conn, id));

/// 批量退订会删除这些源的文章和本地媒体，放到阻塞线程。
#[tauri::command]
pub async fn unsubscribe_feeds(
    state: State<'_, AppState>,
    ids: Vec<i64>,
) -> Result<FeedTree, UiError> {
    tree_task(&state, "unsubscribe_feeds", move |conn| {
        rrss_app::actions::unsubscribe_many(conn, &ids)
    })
    .await
}

#[tauri::command]
pub async fn dismiss_feed_health(state: State<'_, AppState>, id: i64) -> Result<FeedTree, UiError> {
    tree_task(&state, "dismiss_feed_health", move |conn| {
        rrss_app::actions::dismiss_feed_health(conn, id)
    })
    .await
}

/// 手动重试单个订阅：等到结果写入后返回，前端随后重读订阅树。
#[tauri::command]
pub async fn refresh_feed(state: State<'_, AppState>, id: i64) -> Result<(), UiError> {
    state
        .scheduler
        .refresh_feed(id)
        .await
        .map_err(UiError::from)
}

db_task!(create_folder(name: String, parent_folder_id: Option<i64>) -> i64 =>
    |conn| rrss_app::actions::create_folder(conn, &name, parent_folder_id));

db_task!(move_tree_items(items: Vec<String>, parent_folder_id: Option<i64>, before: Option<String>) -> Vec<TreeLayout> =>
    |conn| rrss_app::actions::move_tree_items(conn, items, parent_folder_id, before));

db_task!(restore_tree_layout(layout: Vec<TreeLayout>) -> () =>
    |conn| rrss_app::actions::restore_tree_layout(conn, layout));

db_task!(rename_folder(id: i64, name: String) -> () =>
    |conn| rrss_app::actions::rename_folder(conn, id, &name));

db_task!(delete_folder(id: i64) -> () => |conn| rrss_app::actions::delete_folder(conn, id));

db_task!(import_opml(xml: String) -> OpmlImportView =>
    |conn| rrss_app::actions::import_opml(conn, &xml).map(Into::into));

db_task!(export_opml() -> String => rrss_app::actions::export_opml);

command_handler![
    feed_tree,
    feeds_page,
    feed_counts,
    discover_feeds,
    mark_scope_read,
    add_feed,
    refresh_feed,
    save_feed,
    unsubscribe,
    create_folder,
    rename_folder,
    delete_folder,
    import_opml,
    export_opml,
    unsubscribe_feeds,
    dismiss_feed_health,
    move_tree_items,
    restore_tree_layout,
];
