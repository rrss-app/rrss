use tauri::State;

use rrss_app::queries;
use rrss_app::views::*;

use crate::state::{AppState, with_db};

db_task!(feed_tree() -> FeedTree => queries::feed_tree);

db_task!(feeds_page(scope: String, list_scope: String, cursor: Option<FeedsCursor>) -> FeedsPage =>
    |conn| queries::feeds_page(conn, &scope, &list_scope, cursor));

db_task!(feed_counts() -> FeedCountsView => queries::feed_counts);

db_task!(mark_scope_read(scope: String) -> Vec<i64> =>
    |conn| rrss_app::actions::mark_scope_read(conn, &scope));

#[tauri::command]
pub async fn add_feed(state: State<'_, AppState>, url: String) -> Result<AddFeedResult, String> {
    state
        .scheduler
        .add_feed(&url)
        .await
        .map_err(|e| e.to_string())
}

db_task!(save_feed(args: SaveFeedArgs) -> FeedTree =>
    |conn| rrss_app::actions::save_feed(conn, &args));

db_task!(unsubscribe(id: i64) -> () => |conn| rrss_app::actions::unsubscribe(conn, id));

db_task!(
    /// 批量退订会删除这些源的文章和本地媒体，放到阻塞线程。
    unsubscribe_feeds(ids: Vec<i64>) -> FeedTree =>
        |conn| rrss_app::actions::unsubscribe_many(conn, &ids)
);

db_task!(dismiss_feed_health(id: i64) -> FeedTree =>
    |conn| rrss_app::actions::dismiss_feed_health(conn, id));

db_task!(create_folder(name: String, parent_folder_id: Option<i64>) -> i64 =>
    |conn| rrss_app::actions::create_folder(conn, &name, parent_folder_id));

db_task!(move_tree_items(items: Vec<String>, parent_folder_id: Option<i64>, before: Option<String>) -> Vec<TreeLayout> =>
    |conn| rrss_app::actions::move_tree_items(conn, items, parent_folder_id, before));

db_task!(restore_tree_layout(layout: Vec<TreeLayout>) -> () =>
    |conn| rrss_app::actions::restore_tree_layout(conn, layout));

db_task!(rename_folder(id: i64, name: String) -> () =>
    |conn| rrss_app::actions::rename_folder(conn, id, &name));

db_task!(delete_folder(id: i64) -> () => |conn| rrss_app::actions::delete_folder(conn, id));

#[tauri::command]
pub async fn import_opml(state: State<'_, AppState>, xml: String) -> Result<String, String> {
    let path = state.db_path.clone();
    with_db("import_opml", path, move |conn| {
        let report = rrss_app::actions::import_opml(conn, &xml).map_err(|e| e.to_string())?;
        Ok(format!(
            "新增 {} 个订阅，跳过 {} 个重复，新建 {} 个文件夹",
            report.imported, report.skipped_same_url, report.folders
        ))
    })
    .await
}

db_task!(export_opml() -> String => rrss_app::actions::export_opml);

command_handler![
    feed_tree,
    feeds_page,
    feed_counts,
    mark_scope_read,
    add_feed,
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
