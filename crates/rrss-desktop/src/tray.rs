//! 后台驻留：拦下主窗口的关闭、托盘图标与菜单、单实例唤回。
//! 关闭策略由 `rrss_app::window_close` 定义；此处适配窗口、托盘与已保存策略的内存副本。

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use rrss_app::views::TrayLabels;
use rrss_app::window_close::{CloseAction, CloseChoice, CloseDecision};
use rrss_app::{UiError, ui_error::host::HostError};
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
/// 「记住退出」要先写设置再退出，最多等这么久；写不进也照样退出，只是下次还会再问。
const REMEMBER_QUIT_WAIT: Duration = Duration::from_secs(2);

/// 托盘菜单项（换语言时改文字）、「窗口是否藏在托盘里」与已提交的关闭设置。
#[derive(Default)]
pub struct Tray {
    items: Mutex<Option<[MenuItem<Wry>; 3]>>,
    hidden: AtomicBool,
    close_action: Mutex<Option<CloseAction>>,
    close_updates: std::sync::Arc<tokio::sync::Mutex<()>>,
}

impl Tray {
    pub fn new(action: CloseAction) -> Self {
        Self {
            close_action: Mutex::new(Some(action)),
            ..Self::default()
        }
    }
    fn remember_action(&self, action: CloseAction) {
        if let Ok(mut slot) = self.close_action.lock() {
            *slot = Some(action);
        }
    }

    fn remembered_action(&self) -> Option<CloseAction> {
        self.close_action.lock().ok().and_then(|slot| *slot)
    }
}

/// The close decision is loaded at startup and updated after each successful setting write.
/// No database work or async scheduling is needed to ask, hide or quit.
pub fn close_requested(app: &AppHandle) {
    let started = std::time::Instant::now();
    let action = app
        .state::<Tray>()
        .remembered_action()
        .unwrap_or(CloseAction::Ask);
    apply(app, action.decision(has_tray(app)));
    tracing::debug!(
        elapsed_ms = started.elapsed().as_secs_f64() * 1000.0,
        "close decision dispatched"
    );
}

pub async fn save_close_setting(
    app: &AppHandle,
    db_path: std::path::PathBuf,
    value: String,
) -> Result<rrss_app::views::SettingValue, UiError> {
    let update = app.state::<Tray>().close_updates.clone().lock_owned().await;
    let app = app.clone();
    state::with_db("set_setting", db_path, move |conn| {
        // Cancellation of the caller must not allow a later write to overtake this cache update.
        let _update = update;
        let result = rrss_app::actions::set_setting(conn, rrss_app::window_close::SETTING, &value)
            .map_err(UiError::from)?;
        app.state::<Tray>()
            .remember_action(CloseAction::parse(&result.value));
        Ok(result)
    })
    .await
}

/// 关闭询问的结果：照做不等数据库；勾了「记住」就写进设置。
/// 隐藏先做再写；退出要先写（退出后就写不了），最多等 `REMEMBER_QUIT_WAIT`。
pub async fn close_choice(
    app: &AppHandle,
    db_path: std::path::PathBuf,
    choice: CloseChoice,
    remember: bool,
) -> Result<(), UiError> {
    let decision = CloseAction::from(choice).decision(has_tray(app));
    if !remember {
        apply(app, decision);
        return Ok(());
    }
    let write = save_close_setting(
        app,
        db_path,
        match choice {
            CloseChoice::Tray => "tray",
            CloseChoice::Quit => "quit",
        }
        .into(),
    );
    if decision == CloseDecision::Quit {
        match tokio::time::timeout(REMEMBER_QUIT_WAIT, write).await {
            Ok(Ok(_)) => {}
            Ok(Err(error)) => tracing::warn!(code = error.code(), "remember close choice failed"),
            Err(_) => tracing::warn!(error_kind = "timeout", "remember close choice timed out"),
        }
        apply(app, decision);
        return Ok(());
    }
    apply(app, decision);
    write.await?;
    Ok(())
}

/// 建托盘（已建时只换菜单文字）。文字来自前端；托盘不可用时不建，返回 false。
// Menu and icon failures are internal; the diagnostic keeps Tauri's wording.
fn tray_error(error: tauri::Error) -> UiError {
    UiError::from(anyhow::Error::from(error))
}

pub fn install(app: &AppHandle, labels: TrayLabels) -> Result<bool, UiError> {
    let tray = app.state::<Tray>();
    let mut items = tray
        .items
        .lock()
        .map_err(|_| UiError::from(HostError::TrayStateUnavailable))?;
    if let Some([open, refresh, quit]) = items.as_ref() {
        for (item, text) in [
            (open, &labels.open),
            (refresh, &labels.refresh),
            (quit, &labels.quit),
        ] {
            item.set_text(text).map_err(tray_error)?;
        }
        return Ok(true);
    }
    if !rrss_core::platform::tray_available() {
        tracing::info!("system tray unavailable; closing to tray falls back to minimize");
        return Ok(false);
    }
    let open =
        MenuItem::with_id(app, "open", &labels.open, true, None::<&str>).map_err(tray_error)?;
    let refresh = MenuItem::with_id(app, "refresh", &labels.refresh, true, None::<&str>)
        .map_err(tray_error)?;
    let quit =
        MenuItem::with_id(app, "quit", &labels.quit, true, None::<&str>).map_err(tray_error)?;
    let separator = PredefinedMenuItem::separator(app).map_err(tray_error)?;
    let menu = Menu::with_items(app, &[&open, &refresh, &separator, &quit]).map_err(tray_error)?;
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
    builder.build(app).map_err(tray_error)?;
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
            if let Err(error) = app.emit(CLOSE_REQUESTED_EVENT, rrss_core::feeds::now_ms()) {
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
