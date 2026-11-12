use tauri::State;

use rrss_app::views::*;
use rrss_app::{actions, queries};

use crate::state::AppState;

db_task!(threads() -> Vec<ThreadView> => queries::threads);

db_task!(search_threads(query: String, limit: i64) -> Vec<ThreadSearchHitView> => |conn| queries::search_threads(conn, &query, limit));
db_task!(thread_list() -> ThreadListView => queries::thread_list);
db_task!(archived_threads() -> Vec<ThreadView> => queries::archived_threads);
db_task!(rename_thread(thread_id: i64, title: String) -> () => |conn| actions::rename_thread(conn, thread_id, &title));
db_task!(set_thread_folder(thread_id: i64, folder_id: Option<i64>) -> () => |conn| actions::set_thread_folder(conn, thread_id, folder_id));
db_task!(set_thread_pinned(thread_id: i64, pinned: bool) -> () => |conn| actions::set_thread_pinned(conn, thread_id, pinned));
db_task!(archive_thread(thread_id: i64) -> () => |conn| actions::archive_thread(conn, thread_id));
db_task!(unarchive_thread(thread_id: i64) -> () => |conn| actions::unarchive_thread(conn, thread_id));
db_task!(create_thread_folder(name: String) -> ThreadFolderView => |conn| actions::create_thread_folder(conn, &name));
db_task!(rename_thread_folder(folder_id: i64, name: String) -> () => |conn| actions::rename_thread_folder(conn, folder_id, &name));
db_task!(delete_thread_folder(folder_id: i64) -> () => |conn| actions::delete_thread_folder(conn, folder_id));
db_task!(reorder_thread_folders(ids: Vec<i64>) -> () => |conn| actions::reorder_thread_folders(conn, &ids));
db_task!(set_thread_folder_collapsed(folder_id: i64, collapsed: bool) -> () => |conn| actions::set_thread_folder_collapsed(conn, folder_id, collapsed));

#[tauri::command]
pub async fn delete_thread(state: State<'_, AppState>, thread_id: i64) -> Result<(), String> {
    let guard = state
        .chat
        .lock_thread_deletion(thread_id)
        .await
        .map_err(|e| e.message)?;
    let data_dir = state.data_dir.clone();
    crate::state::with_db("delete_thread", state.db_path.clone(), move |conn| {
        let _guard = guard;
        actions::delete_thread(conn, thread_id, &data_dir).map_err(|e| e.to_string())
    })
    .await
}

db_task!(messages(thread_id: i64, before_id: Option<i64>, limit: Option<i64>) -> MessagesPage =>
    |conn| queries::messages(conn, thread_id, before_id, limit));

db_task!(digest_thread(digest_id: i64) -> DigestThreadView =>
    |conn| queries::digest_thread(conn, digest_id));

db_task!(new_thread() -> i64 => |conn| rrss_app::actions::new_thread(conn, None));

db_task!(article_previews(ids: Vec<i64>) -> Vec<ArticlePreviewView> =>
    |conn| queries::article_previews(conn, &ids));

#[tauri::command]
pub async fn send_message(
    state: State<'_, AppState>,
    args: SendMessageArgs,
) -> Result<i64, String> {
    super::logged("send_message", async {
        state.chat.send(args).await.map_err(|e| e.message)
    })
    .await
}

#[tauri::command]
pub async fn abort_generation(state: State<'_, AppState>) -> Result<(), String> {
    super::logged("abort_generation", async {
        state.chat.abort().await.map_err(|e| e.message)
    })
    .await
}

/// 设置页「重启引擎」：有序关闭后按当前配置重新启动（含托管下载与握手）。
#[tauri::command]
pub async fn restart_pi(state: State<'_, AppState>) -> Result<(), String> {
    super::logged("restart_pi", async {
        state.chat.restart_engine().await.map_err(|e| e.message)
    })
    .await
}

#[tauri::command]
pub async fn stop_pi(state: State<'_, AppState>) -> Result<(), String> {
    state.chat.stop_engine().await;
    Ok(())
}

#[tauri::command]
pub async fn confirm_fetch(
    state: State<'_, AppState>,
    id: String,
    ok: bool,
    remember: bool,
) -> Result<(), String> {
    state
        .chat
        .confirm(&id, ok, remember)
        .await
        .map_err(|e| e.message)
}

command_handler![
    threads,
    thread_list,
    search_threads,
    archived_threads,
    rename_thread,
    set_thread_folder,
    set_thread_pinned,
    archive_thread,
    unarchive_thread,
    delete_thread,
    create_thread_folder,
    rename_thread_folder,
    delete_thread_folder,
    reorder_thread_folders,
    set_thread_folder_collapsed,
    messages,
    new_thread,
    article_previews,
    send_message,
    digest_thread,
    abort_generation,
    restart_pi,
    stop_pi,
    confirm_fetch,
];
