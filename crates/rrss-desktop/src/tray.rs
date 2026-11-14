//! 后台驻留：拦下主窗口的关闭、托盘图标与菜单、单实例唤回（阶段 14 · 后台驻留）。
//! 关闭后怎么做由 `rrss_app::window_close` 按设置决定；这里只动窗口和托盘，不读写设置。

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use rrss_app::views::TrayLabels;
use rrss_app::window_close::{CloseChoice, CloseDecision};
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, Wry};

use crate::state::{self, AppState};

const MAIN: &str = "main";
const TRAY_ID: &str = "main";
/// 设置为「每次询问」时，交给前端弹关闭询问。
pub const CLOSE_REQUESTED_EVENT: &str = "window:close-requested";
/// 从托盘隐藏后唤回：前端焦点列表直接采用最新内容。
pub const RESTORED_EVENT: &str = "window:restored";

/// 托盘菜单项（换语言时改文字）与「窗口是否藏在托盘里」。
#[derive(Default)]
pub struct Tray {
    items: Mutex<Option<[MenuItem<Wry>; 3]>>,
    hidden: AtomicBool,
}

/// 主窗口的关闭请求已被拦下：按设置询问、隐藏、最小化或退出。设置在工作线程里读，不占界面线程。
pub fn close_requested(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let tray = has_tray(&app);
        let db_path = app
            .try_state::<AppState>()
            .map(|state| state.db_path.clone());
        let decision = match db_path {
            Some(db_path) => state::with_db("window_close_requested", db_path, move |conn| {
                rrss_app::window_close::decide(conn, tray).map_err(rrss_app::UiError::from)
            })
            .await
            // 读不到设置时退回询问，宁可多问一次也不替用户退出或藏起窗口。
            .unwrap_or(CloseDecision::Ask),
            // 启动还没装好状态就关窗：直接退出。
            None => CloseDecision::Quit,
        };
        apply(&app, decision);
    });
}

/// 关闭询问的结果：勾了「记住」就写进设置，然后照做。
pub async fn close_choice(
    app: &AppHandle,
    db_path: std::path::PathBuf,
    choice: CloseChoice,
    remember: bool,
) -> Result<(), rrss_app::UiError> {
    let tray = has_tray(app);
    let decision = state::with_db("window_close_choice", db_path, move |conn| {
        rrss_app::window_close::choose(conn, choice, remember, tray)
            .map_err(rrss_app::UiError::from)
    })
    .await?;
    apply(app, decision);
    Ok(())
}

/// 建托盘（已建时只换菜单文字）。文字来自前端；托盘不可用时不建，返回 false。
pub fn install(app: &AppHandle, labels: TrayLabels) -> Result<bool, String> {
    let tray = app.state::<Tray>();
    let mut items = tray
        .items
        .lock()
        .map_err(|_| "tray state poisoned".to_string())?;
    if let Some([open, refresh, quit]) = items.as_ref() {
        for (item, text) in [
            (open, &labels.open),
            (refresh, &labels.refresh),
            (quit, &labels.quit),
        ] {
            item.set_text(text).map_err(|error| error.to_string())?;
        }
        return Ok(true);
    }
    if !rrss_core::platform::tray_available() {
        tracing::info!("system tray unavailable; closing to tray falls back to minimize");
        return Ok(false);
    }
    let open = MenuItem::with_id(app, "open", &labels.open, true, None::<&str>)
        .map_err(|e| e.to_string())?;
    let refresh = MenuItem::with_id(app, "refresh", &labels.refresh, true, None::<&str>)
        .map_err(|e| e.to_string())?;
    let quit = MenuItem::with_id(app, "quit", &labels.quit, true, None::<&str>)
        .map_err(|e| e.to_string())?;
    let separator = PredefinedMenuItem::separator(app).map_err(|e| e.to_string())?;
    let menu =
        Menu::with_items(app, &[&open, &refresh, &separator, &quit]).map_err(|e| e.to_string())?;
    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip("RRSS")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "open" => show_main(app),
            "refresh" => refresh_now(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app).map_err(|error| error.to_string())?;
    *items = Some([open, refresh, quit]);
    Ok(true)
}

/// 唤回主窗口（托盘、再次启动）。若窗口是隐藏在托盘里的，通知前端这是一次「唤回」。
pub fn show_main(app: &AppHandle) {
    let Some(window) = app.get_webview_window(MAIN) else {
        return;
    };
    let restored = app.state::<Tray>().hidden.swap(false, Ordering::SeqCst);
    let _ = window.show();
    let _ = window.unminimize();
    let _ = window.set_focus();
    if restored && let Err(error) = app.emit(RESTORED_EVENT, ()) {
        tracing::warn!(%error, "emit window restored");
    }
}

fn has_tray(app: &AppHandle) -> bool {
    app.tray_by_id(TRAY_ID).is_some()
}

fn apply(app: &AppHandle, decision: CloseDecision) {
    let Some(window) = app.get_webview_window(MAIN) else {
        return;
    };
    match decision {
        CloseDecision::Ask => {
            if let Err(error) = app.emit(CLOSE_REQUESTED_EVENT, ()) {
                tracing::warn!(%error, "emit close requested");
            }
        }
        CloseDecision::Hide => {
            app.state::<Tray>().hidden.store(true, Ordering::SeqCst);
            if let Err(error) = window.hide() {
                tracing::warn!(%error, "hide main window");
            }
        }
        CloseDecision::Minimize => {
            if let Err(error) = window.minimize() {
                tracing::warn!(%error, "minimize main window");
            }
        }
        CloseDecision::Quit => app.exit(0),
    }
}

/// 托盘「立即刷新」：后台抓取一次，不打开窗口。
fn refresh_now(app: &AppHandle) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let scheduler = state.scheduler.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(error) = scheduler.start_refresh().await {
            tracing::warn!(error = %error, "tray refresh failed to start");
        }
    });
}
