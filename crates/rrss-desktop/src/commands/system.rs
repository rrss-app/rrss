use tauri::{AppHandle, Manager, State};
use tauri_plugin_dialog::DialogExt;

use rrss_app::queries;
use rrss_app::views::*;

use crate::state::{AppState, with_db};

#[tauri::command]
pub async fn log_client(
    level: ClientLogLevel,
    scope: String,
    message: String,
    stack: Option<String>,
) {
    let _ = rrss_app::execution::blocking(move || {
        rrss_app::log::client::record(level, &scope, &message, stack.as_deref());
    })
    .await;
}

/// 切换本次运行的文件日志级别，返回新的日志状态。
#[tauri::command]
pub async fn log_set_level(detailed: bool) -> Result<LogStatusView, UiError> {
    rrss_app::execution::blocking(move || {
        let handle =
            rrss_app::log::current().ok_or_else(|| UiError::from("日志没有初始化".to_string()))?;
        let level = if detailed {
            rrss_app::log::Level::Detailed
        } else {
            rrss_app::log::Level::Default
        };
        handle
            .set_level(level)
            .map_err(|_| UiError::from("日志文件不可用，无法切换记录级别".to_string()))?;
        tracing::info!(detailed, "file log level changed");
        handle
            .status_view()
            .ok_or_else(|| UiError::from("日志文件不可用".to_string()))
    })
    .await
    .map_err(UiError::from)?
}

/// 先选保存位置，再在阻塞线程打包；返回保存的路径，取消时为空。
#[tauri::command]
pub async fn log_export(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Option<String>, UiError> {
    let documents = app.path().document_dir().ok();
    let Some(dest) = super::backup::pick(move || {
        let mut dialog = app
            .dialog()
            .file()
            .set_title("导出诊断包")
            .set_file_name(rrss_app::log::default_export_name())
            .add_filter("诊断包", &["zip"]);
        if let Some(dir) = documents {
            dialog = dialog.set_directory(dir);
        }
        dialog.blocking_save_file()
    })
    .await?
    else {
        return Ok(None);
    };
    let db_path = state.db_path.clone();
    super::logged("log_export", async move {
        tokio::task::spawn_blocking(move || {
            rrss_app::log::export_diagnostics(&db_path, &dest)
                .map(|()| dest.display().to_string())
                .map_err(|error| UiError::from(format!("导出诊断包失败：{error}")))
        })
        .await
        .map_err(|error| UiError::from(anyhow::Error::from(error)))?
    })
    .await
    .map(Some)
}

/// 统计会扫描向量与 FTS 表，大库上耗时明显，不能占用主线程。
#[tauri::command]
pub async fn diagnostics(state: State<'_, AppState>) -> Result<DiagnosticsPage, UiError> {
    let path = state.db_path.clone();
    with_db("diagnostics", path.clone(), move |conn| {
        Ok(queries::diagnostics(conn, &path))
    })
    .await
}

/// 存储占用要扫一遍库与数据目录，放到阻塞线程。
#[tauri::command]
pub async fn storage_page(state: State<'_, AppState>) -> Result<StoragePage, UiError> {
    let path = state.db_path.clone();
    with_db("storage_page", path.clone(), move |conn| {
        queries::storage_page(conn, &path).map_err(UiError::from)
    })
    .await
}

/// 清理与压缩可能耗时（`VACUUM` 需要重写整个库），同样不占主线程。
#[tauri::command]
pub async fn storage_clean(state: State<'_, AppState>, action: String) -> Result<String, UiError> {
    let path = state.db_path.clone();
    tracing::info!(%action, "storage cleanup requested");
    with_db("storage_clean", path.clone(), move |conn| {
        rrss_app::actions::storage_clean(conn, &path, &action).map_err(|e| {
            let e = UiError::from(e);
            tracing::error!(%action, code = e.code(), detail = %e.log_detail(), "storage cleanup failed");
            e
        })
    })
    .await
}

db_task!(digest_list(period: String) -> Vec<DigestSummary> => |conn| Ok::<_, rrss_core::Error>(queries::digest_summaries(conn, &period)));

db_task!(settings_schema() -> SettingsSchema => rrss_app::settings_schema::schema);

db_task!(set_setting(id: String, value: String) -> SettingValue =>
    |conn| rrss_app::actions::set_setting(conn, &id, &value));

db_task!(digest(id: i64) -> DigestView => |conn| queries::digest(conn, id));

#[tauri::command]
pub async fn generate_digest(state: State<'_, AppState>, period: String) -> Result<(), UiError> {
    let period_str = if period == "weekly" {
        "weekly"
    } else {
        "daily"
    };
    tracing::info!(period = %period_str, "manual generate_digest triggered");
    state
        .scheduler
        .generate_digest(period_str)
        .await
        .map_err(|e| {
            let e = UiError::from(e);
            tracing::error!(period = %period_str, code = e.code(), detail = %e.log_detail(), "manual generate_digest failed");
            e
        })?;
    tracing::info!(period = %period_str, "manual generate_digest completed successfully");
    Ok(())
}

#[tauri::command]
pub async fn summarize_article(state: State<'_, AppState>, id: i64) -> Result<String, UiError> {
    tracing::info!(article_id = id, "manual summarize_article requested");
    let res = state.scheduler.summarize_article(id).await.map_err(|e| {
        let e = UiError::from(e);
        tracing::error!(article_id = id, code = e.code(), detail = %e.log_detail(), "manual summarize_article failed");
        e
    })?;
    tracing::info!(
        article_id = id,
        summary_len = res.len(),
        "manual summarize_article completed"
    );
    Ok(res)
}

#[tauri::command]
pub async fn refresh(state: State<'_, AppState>) -> Result<RefreshStarted, UiError> {
    state.scheduler.start_refresh().await.map_err(UiError::from)
}

#[tauri::command]
pub fn scheduler_status(state: State<'_, AppState>) -> SchedulerSnapshot {
    state.scheduler.snapshot()
}

#[tauri::command]
pub async fn probe_env() -> Result<serde_json::Value, UiError> {
    rrss_app::execution::blocking(|| {
        let fonts: Vec<&'static str> = rrss_core::platform::installed_cjk_fonts().unwrap_or_default();
        serde_json::json!({
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "webkit_note": "Linux uses WebKitGTK 4.1; Windows uses WebView2",
            "cjk_fonts_detected": fonts,
            "ime_note": "Use the probe page input; IME composition should produce CJK in the WebView text field",
            "protocol": rrss_app::queries::media_href(0).replace('0', "<id>"),
        })
    })
    .await
    .map_err(UiError::from)
}

/// 自绘标题栏：窗口三键按平台的样子画。
#[tauri::command]
pub fn window_chrome() -> WindowChromeView {
    queries::window_chrome()
}

command_handler![
    log_client,
    log_set_level,
    log_export,
    window_chrome,
    digest_list,
    digest,
    settings_schema,
    set_setting,
    diagnostics,
    storage_page,
    storage_clean,
    refresh,
    scheduler_status,
    generate_digest,
    summarize_article,
    probe_env,
];
