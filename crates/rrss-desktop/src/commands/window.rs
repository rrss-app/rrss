//! 窗口与托盘（后台驻留）：只转接 `crate::tray`，关闭后怎么做由 `rrss_app::window_close` 决定。

use rrss_app::views::{TrayLabels, UiError};
use rrss_app::window_close::CloseChoice;
use tauri::AppHandle;

use crate::state::AppState;

/// 关闭询问的结果：隐藏到托盘或退出程序，勾了「记住」就写进设置。
#[tauri::command]
pub async fn window_close_choice(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    choice: CloseChoice,
    remember: bool,
) -> Result<(), UiError> {
    crate::tray::close_choice(&app, state.db_path.clone(), choice, remember).await
}

/// 建托盘或换菜单文字；托盘不可用时返回 false。
#[tauri::command]
pub fn tray_install(app: AppHandle, labels: TrayLabels) -> Result<bool, UiError> {
    crate::tray::install(&app, labels).map_err(UiError::from)
}

command_handler![window_close_choice, tray_install];
