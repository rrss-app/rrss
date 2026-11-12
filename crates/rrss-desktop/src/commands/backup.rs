//! 备份与恢复、偏好导出导入。文件对话框在宿主里弹出，业务都在 `rrss_app::actions`。

use std::path::PathBuf;

use tauri::{AppHandle, Manager, State};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;

use rrss_app::actions;
use rrss_app::queries;
use rrss_app::views::*;

use crate::state::{AppState, with_db};

#[tauri::command]
pub async fn backup_page(state: State<'_, AppState>) -> Result<BackupPage, String> {
    let data_dir = state.data_dir.clone();
    with_db("backup_page", state.db_path.clone(), move |conn| {
        queries::backup_page(conn, &data_dir).map_err(|e| e.to_string())
    })
    .await
}

/// 弹出保存对话框后生成完整备份；用户取消时返回 `None`。
#[tauri::command]
pub async fn backup_create(
    app: AppHandle,
    include_secrets: bool,
    state: State<'_, AppState>,
) -> Result<Option<BackupDoneView>, String> {
    let data_dir = state.data_dir.clone();
    let db_path = state.db_path.clone();
    let documents = app.path().document_dir().ok();
    let Some(dest) = pick(move || {
        let mut dialog = app
            .dialog()
            .file()
            .set_title("保存备份")
            .set_file_name(queries::default_backup_name())
            .add_filter("RRSS 备份", &["zip"]);
        if let Some(dir) = documents {
            dialog = dialog.set_directory(dir);
        }
        dialog.blocking_save_file()
    })
    .await?
    else {
        return Ok(None);
    };
    tracing::info!(file = %rrss_core::logsafe::path(&dest), "manual backup requested");
    with_db("backup_create", db_path, move |conn| {
        actions::backup_create(conn, &data_dir, &dest, include_secrets).map_err(|e| {
            tracing::error!(error = %e, "manual backup failed");
            e.to_string()
        })
    })
    .await
    .map(Some)
}

/// 选择备份文件并检查；不给路径时弹出打开对话框，用户取消返回 `None`。
#[tauri::command]
pub async fn backup_inspect(
    app: AppHandle,
    state: State<'_, AppState>,
    path: Option<String>,
) -> Result<Option<RestoreCheckView>, String> {
    let data_dir = state.data_dir.clone();
    let path = match path {
        Some(p) => PathBuf::from(p),
        None => {
            let picked = pick(move || {
                app.dialog()
                    .file()
                    .set_title("选择备份文件")
                    .add_filter("RRSS 备份", &["zip"])
                    .blocking_pick_file()
            })
            .await?;
            match picked {
                Some(p) => p,
                None => return Ok(None),
            }
        }
    };
    tokio::task::spawn_blocking(move || actions::backup_inspect(&data_dir, &path))
        .await
        .map_err(|e| e.to_string())?
        .map(Some)
        .map_err(|e| e.to_string())
}

/// 备份当前数据、登记恢复并重启；成功时不会返回。
#[tauri::command]
pub async fn backup_restore(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
) -> Result<(), String> {
    let data_dir = state.data_dir.clone();
    let source = PathBuf::from(&path);
    tracing::info!(file = %rrss_core::logsafe::path(std::path::Path::new(&path)), "restore confirmed; backing up current data");
    with_db("backup_restore", state.db_path.clone(), move |conn| {
        actions::backup_restore(conn, &data_dir, &source).map_err(|e| {
            tracing::error!(error = %e, "restore preparation failed");
            e.to_string()
        })
    })
    .await?;
    app.restart();
}

#[tauri::command]
pub async fn backup_cancel_restore(state: State<'_, AppState>) -> Result<(), String> {
    let data_dir = state.data_dir.clone();
    tokio::task::spawn_blocking(move || actions::backup_cancel_restore(&data_dir))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

db_task!(backup_set_schedule(value: String) -> () => |conn| actions::set_backup_schedule(conn, &value));

#[tauri::command]
pub async fn backup_dismiss_notice(state: State<'_, AppState>) -> Result<(), String> {
    let data_dir = state.data_dir.clone();
    rrss_app::execution::blocking(move || actions::dismiss_restore_notice(&data_dir))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn backup_reveal(app: AppHandle, path: String) -> Result<(), String> {
    rrss_app::execution::blocking(move || {
        app.opener()
            .reveal_item_in_dir(path)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn prefs_export(
    state: State<'_, AppState>,
    rules: bool,
    interests: bool,
    memories: bool,
) -> Result<String, String> {
    let parts = rrss_core::prefs::Parts {
        rules,
        interests,
        memories,
    };
    with_db("prefs_export", state.db_path.clone(), move |conn| {
        actions::prefs_export(conn, parts).map_err(|e| e.to_string())
    })
    .await
}

db_task!(prefs_import(text: String) -> PrefsImportView => |conn| actions::prefs_import(conn, &text));

/// 原生文件对话框会阻塞，放到阻塞线程。
pub(crate) async fn pick<F>(f: F) -> Result<Option<PathBuf>, String>
where
    F: FnOnce() -> Option<tauri_plugin_dialog::FilePath> + Send + 'static,
{
    let picked = tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| e.to_string())?;
    picked
        .map(|p| p.into_path().map_err(|e| e.to_string()))
        .transpose()
}

db_task!(
    /// 保留预览要扫文章和引用，放到阻塞线程。
    retention_page() -> RetentionPage =>
        |conn| queries::retention_page(conn, rrss_core::feeds::now_ms())
);

db_task!(retention_set_days(days: i64) -> () => |conn| actions::set_retention_days(conn, days));

/// 立即清理：第一次会先做完整备份，大库上耗时明显。
#[tauri::command]
pub async fn retention_clean_now(state: State<'_, AppState>) -> Result<String, String> {
    let data_dir = state.data_dir.clone();
    tracing::info!("retention cleanup requested");
    with_db("retention_clean_now", state.db_path.clone(), move |conn| {
        actions::retention_clean_now(conn, &data_dir).map_err(|e| {
            tracing::error!(error = %e, "retention cleanup failed");
            e.to_string()
        })
    })
    .await
}

command_handler![
    backup_page,
    backup_create,
    backup_inspect,
    backup_restore,
    backup_cancel_restore,
    backup_set_schedule,
    backup_dismiss_notice,
    backup_reveal,
    prefs_export,
    prefs_import,
    retention_page,
    retention_set_days,
    retention_clean_now,
];
