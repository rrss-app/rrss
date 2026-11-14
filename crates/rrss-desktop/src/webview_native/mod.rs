//! Native webview handles stay here; policy and persistence stay in rrss-app.
use rrss_core::network::WebviewRoute;
use rrss_core::url_norm::is_http_url;
use tauri::Manager;
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;

use crate::commands::web_window::emit_main;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux as native;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as native;
#[cfg(not(any(target_os = "linux", windows)))]
mod native {
    // 未实现：其他平台的原生菜单、权限与下载适配。
    pub fn configure(
        _: tauri::webview::PlatformWebview,
        _: &tauri::Webview,
        _: &rrss_core::network::WebviewRoute,
        _: rrss_app::WebViewBounds,
    ) -> Result<(), String> {
        Ok(())
    }
    pub fn history(_: &tauri::Webview, _: bool) -> tauri::Result<()> {
        Ok(())
    }
    pub fn window_icons(_: &tauri::WebviewWindow) {}
    pub fn page_title(_: tauri::webview::PlatformWebview) -> Result<Option<String>, String> {
        Err("原生网页快照未实现".into())
    }
    pub fn snapshot(
        _: tauri::webview::PlatformWebview,
        send: tokio::sync::oneshot::Sender<Result<Vec<u8>, String>>,
    ) {
        let _ = send.send(Err("原生网页快照未实现".into()));
    }
}

/// 主窗口的任务栏与标题栏图标。Windows 上 Tauri 只给窗口一张 256 px 位图，由系统临时缩小，
/// 「R。」的细笔画会糊；这里改用 exe 内嵌的多尺寸图标。其他平台沿用 Tauri 默认。
pub fn window_icons(window: &tauri::WebviewWindow) {
    native::window_icons(window)
}

pub async fn page_title(webview: &tauri::Webview) -> Result<Option<String>, String> {
    let (send, receive) = tokio::sync::oneshot::channel();
    webview
        .with_webview(move |handle| {
            let _ = send.send(native::page_title(handle));
        })
        .map_err(|_| failure())?;
    receive.await.map_err(|_| failure())?
}

pub async fn snapshot_mhtml(webview: &tauri::Webview) -> Result<Vec<u8>, String> {
    let (send, receive) = tokio::sync::oneshot::channel();
    webview
        .with_webview(move |handle| native::snapshot(handle, send))
        .map_err(|_| failure())?;
    tokio::time::timeout(std::time::Duration::from_secs(30), receive)
        .await
        .map_err(|_| "原生网页快照超时".to_string())?
        .map_err(|_| failure())?
}

const ACTIONS: &[&str] = &[
    "back",
    "forward",
    "reload",
    "copy",
    "open",
    "disable-js",
    "clear",
    "close",
];

fn known_action(action: &str) -> bool {
    ACTIONS.contains(&action)
}

pub async fn configure(
    webview: &tauri::Webview,
    route: WebviewRoute,
    bounds: rrss_app::WebViewBounds,
) -> Result<(), String> {
    let target = webview.clone();
    let (send, receive) = tokio::sync::oneshot::channel();
    webview
        .with_webview(move |handle| {
            let _ = send.send(native::configure(handle, &target, &route, bounds));
        })
        .map_err(|_| failure())?;
    // with_webview queues work on the UI thread. Do not navigate until it finishes.
    receive.await.map_err(|_| failure())?
}

pub(super) fn emit_escape(app: &tauri::AppHandle, label: &str) {
    emit_main(
        app,
        "web:escape",
        rrss_app::WebEscape {
            label: label.into(),
        },
    );
}

pub(super) fn failure() -> String {
    "原网页原生功能配置或操作失败，请重试。".into()
}

fn message(app: &tauri::AppHandle, text: impl Into<String>) {
    app.dialog()
        .message(text)
        .title("原网页 — RRSS")
        .show(|_| {});
}

fn link_result(app: &tauri::AppHandle, url: &tauri::Url, copy: bool) -> Result<(), String> {
    if copy {
        app.clipboard()
            .write_text(url.as_str())
            .map_err(|_| failure())
    } else {
        app.opener()
            .open_url(url.as_str(), None::<&str>)
            .map_err(|_| failure())
    }
}

pub(super) fn link_action(webview: &tauri::Webview, url: &str, copy: bool) {
    // Native hit-test results are untrusted too. Never forward javascript/file URLs.
    let Ok(url) = tauri::Url::parse(url) else {
        return;
    };
    if !is_http_url(&url) {
        return;
    }
    let app = webview.app_handle();
    if let Err(error) = link_result(app, &url, copy) {
        message(app, error);
    }
}

pub async fn dispatch(webview: &tauri::Webview, action: &str) -> Result<(), rrss_app::UiError> {
    if !known_action(action) {
        return Err(rrss_app::UiError::from("未知的原网页操作。".to_string()));
    }
    match action {
        "back" | "forward" => native::history(webview, action == "back")
            .map_err(|_| rrss_app::UiError::from(failure())),
        "reload" => webview
            .reload()
            .map_err(|_| rrss_app::UiError::from(failure())),
        "close" => {
            crate::commands::web_window::close_web_view(
                webview.app_handle().clone(),
                webview.state(),
                webview.label().to_string(),
            )
            .await
        }
        "copy" | "open" => {
            let url = webview.url().map_err(|_| failure())?;
            if !is_http_url(&url) {
                return Err(rrss_app::UiError::from(failure()));
            }
            link_result(webview.app_handle(), &url, action == "copy")
                .map_err(rrss_app::UiError::from)
        }
        "disable-js" | "clear" => {
            if action == "clear" {
                return crate::commands::web_window::clear_web_data(
                    webview.app_handle().clone(),
                    webview.state(),
                )
                .await;
            }
            let path = webview.state::<crate::AppState>().db_path.clone();
            let url = webview
                .url()
                .map(|u| u.to_string())
                .map_err(|_| failure())?;
            crate::state::with_db("disable_site_javascript", path, move |conn| {
                rrss_app::site_rules::disable_site_javascript(conn, &url)
                    .map_err(rrss_app::UiError::from)
            })
            .await
        }
        _ => Err(rrss_app::UiError::from("未知的原网页操作。".to_string())),
    }
}

/// Windows 和其他平台用逻辑像素 `set_bounds`。
/// Linux 的子 webview 在 GtkBox 里，`set_bounds` 无效，改为 Overlay 的边距和 size_request。
pub fn place(webview: &tauri::Webview, bounds: rrss_app::WebViewBounds) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        linux::place(webview, bounds)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let logical = tauri::LogicalRect {
            position: tauri::LogicalPosition::new(bounds.x, bounds.y),
            size: tauri::LogicalSize::new(bounds.width, bounds.height),
        };
        webview
            .set_bounds(tauri::Rect {
                position: logical.position.into(),
                size: logical.size.into(),
            })
            .map_err(|_| failure())
    }
}

// Linux handles downloads itself: wry installs one context-wide callback per
// window, without inspecting Download::web_view(), so its labels are unreliable.
pub fn download_handler(
    builder: tauri::webview::WebviewBuilder<tauri::Wry>,
) -> tauri::webview::WebviewBuilder<tauri::Wry> {
    #[cfg(target_os = "linux")]
    {
        builder
    }
    #[cfg(not(target_os = "linux"))]
    {
        builder.on_download(|view, event| {
            if let tauri::webview::DownloadEvent::Requested { url, .. } = event {
                emit_main(
                    view.app_handle(),
                    "web:download-blocked",
                    rrss_app::WebDownloadBlocked {
                        label: view.label().into(),
                        url: url.to_string(),
                    },
                );
            }
            false
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{ACTIONS, known_action};

    #[test]
    fn web_view_actions_are_whitelisted() {
        for action in ACTIONS {
            assert!(known_action(action), "{action}");
        }
        for action in ["reader", "unknown", "", "BACK", "close-window"] {
            assert!(!known_action(action), "{action}");
        }
    }
}
