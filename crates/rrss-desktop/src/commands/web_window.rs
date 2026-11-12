//! Stable Tauri webview adapters. Policy and directory ownership live in rrss-app.
use std::collections::BTreeMap;
use std::sync::Arc;

use rrss_app::{WebClosed, WebNavigated, WebViewBounds, web_window};
use rrss_core::{
    network::{ProxyEnvironment, WebviewRoute},
    platform,
    url_norm::{is_http_url, web_navigation_allowed},
};
use tauri::webview::{NewWindowResponse, PermissionResponse, WebviewBuilder};
use tauri::{Emitter, Manager, WebviewUrl};
use tokio::sync::{Mutex, oneshot};

/// External web content may only publish events to the main window.
pub(crate) fn emit_main(
    app: &tauri::AppHandle,
    event: &str,
    payload: impl serde::Serialize + Clone,
) {
    let _ = app.emit_to("main", event, payload);
}

pub struct WebWindows {
    pub(crate) inner: Arc<Mutex<WebWindowsInner>>,
    pub(super) verify:
        std::sync::Mutex<BTreeMap<String, std::sync::Arc<super::web_verify::Verification>>>,
}
pub(crate) struct WebWindowsInner {
    pub(crate) session: Result<web_window::WebSession, &'static str>,
    /// The one original-page child webview, if it exists.
    current: Option<String>,
}

impl WebWindows {
    pub fn new(session: Result<web_window::WebSession, &'static str>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(WebWindowsInner {
                session,
                current: None,
            })),
            verify: Default::default(),
        }
    }

    fn on_destroyed(&self, app: &tauri::AppHandle, label: &str) {
        if let Ok(mut verify) = self.verify.lock()
            && let Some(session) = verify.remove(label)
        {
            session.cancel();
        }
        emit_main(
            app,
            "web:closed",
            WebClosed {
                label: label.into(),
            },
        );
    }
}

fn host_error() -> String {
    "原网页操作失败，请重试。".into()
}

fn finite_bounds(bounds: WebViewBounds) -> Result<WebViewBounds, String> {
    if ![bounds.x, bounds.y, bounds.width, bounds.height]
        .into_iter()
        .all(f64::is_finite)
    {
        return Err("原网页位置无效。".into());
    }
    Ok(WebViewBounds {
        x: bounds.x,
        y: bounds.y,
        width: bounds.width.max(0.0),
        height: bounds.height.max(0.0),
    })
}

fn publish_navigation(webview: &tauri::Webview, url: &tauri::Url) {
    if !is_http_url(url) {
        return;
    }
    let title = web_window::title(url);
    emit_main(
        webview.app_handle(),
        "web:navigated",
        WebNavigated {
            label: webview.label().into(),
            url: url.to_string(),
            title,
        },
    );
}

/// `close` only queues removal. Tauri has no child-webview destroyed event, so the
/// following main-thread task is the signal that the webview has left the event loop.
async fn destroy_webview(
    app: &tauri::AppHandle,
    windows: &WebWindows,
    webview: &tauri::Webview,
) -> Result<(), String> {
    let label = webview.label().to_string();
    let (sender, receiver) = oneshot::channel();
    webview.close().map_err(|_| host_error())?;
    app.run_on_main_thread(move || {
        let _ = sender.send(());
    })
    .map_err(|_| host_error())?;
    receiver.await.map_err(|_| host_error())?;
    windows.on_destroyed(app, &label);
    Ok(())
}

async fn close_current(
    app: &tauri::AppHandle,
    windows: &WebWindows,
    inner: &mut WebWindowsInner,
) -> Result<(), String> {
    if let Some(label) = inner.current.clone()
        && let Some(webview) = app.get_webview(&label)
    {
        destroy_webview(app, windows, &webview).await?;
    }
    inner.current = None;
    Ok(())
}

#[tauri::command]
pub async fn open_web_view(
    app: tauri::AppHandle,
    state: tauri::State<'_, crate::AppState>,
    windows: tauri::State<'_, WebWindows>,
    url: String,
    bounds: WebViewBounds,
) -> Result<String, String> {
    let bounds = finite_bounds(bounds)?;
    let mut inner = windows.inner.lock().await;
    inner.session.as_ref().map_err(|error| error.to_string())?;
    let plan = crate::state::with_db("open_web_view", state.db_path.clone(), move |conn| {
        let native = platform::system_proxy();
        let env = ProxyEnvironment::from_lookup(|key| std::env::var(key).ok());
        web_window::prepare(conn, &url, &env, &native, platform::Os::current())
            .map_err(|e| e.to_string())
    })
    .await?;
    // One original page at a time: fully close the previous child before creating another.
    close_current(&app, &windows, &mut inner).await?;
    // No absent proxy setting may silently turn a direct route into system proxying.
    let direct_args = if plan.route == WebviewRoute::Direct {
        Some(platform::webview_direct_args(platform::Os::current()).map_err(|e| e.to_string())?)
    } else {
        None
    };
    let label = inner
        .session
        .as_mut()
        .map_err(|error| error.to_string())?
        .next_label(&plan.route)
        .map_err(|e| e.to_string())?;
    let navigation_app = app.clone();
    let navigation_label = label.clone();
    let initial_url = plan.url.clone();
    let popup_app = app.clone();
    let popup_label = label.clone();
    // Native proxy, permissions and context menus must precede all remote loads.
    let mut builder = WebviewBuilder::new(
        &label,
        WebviewUrl::External(tauri::Url::parse("about:blank").unwrap()),
    )
    .data_directory(
        inner
            .session
            .as_ref()
            .map_err(|error| error.to_string())?
            .directory
            .clone(),
    )
    .devtools(false)
    .on_navigation(move |url| {
        if !web_navigation_allowed(url) {
            return false;
        }
        if let Some(window) = navigation_app.get_webview(&navigation_label) {
            super::web_verify::navigated(&window);
            publish_navigation(&window, url);
        }
        true
    })
    .on_new_window(move |url, _| {
        if web_navigation_allowed(&url)
            && let Some(window) = popup_app.get_webview(&popup_label)
        {
            // Queue a current-webview navigation; never create/allow a popup.
            let _ = window.navigate(url);
        }
        NewWindowResponse::Deny
    })
    .on_permission_request(|_, _| PermissionResponse::Deny)
    .on_page_load(|window, payload| {
        if matches!(payload.event(), tauri::webview::PageLoadEvent::Finished) {
            publish_navigation(&window, payload.url());
            super::web_verify::finished(window, payload.url());
        } else {
            super::web_verify::navigated(&window);
        }
    });
    if !plan.javascript {
        builder = builder.disable_javascript();
    }
    if let WebviewRoute::Proxy(url) = &plan.route {
        builder = builder.proxy_url(url.clone());
    }
    if let Some(args) = direct_args.filter(|args| !args.is_empty()) {
        builder = builder.additional_browser_args(args);
    }
    // A failed build may already have cached its native browser environment.
    // Pin before building, so a later attempt cannot silently reuse another route.
    inner
        .session
        .as_mut()
        .map_err(|error| error.to_string())?
        .opened(plan.route.clone());
    windows
        .verify
        .lock()
        .map_err(|_| host_error())?
        .insert(label.clone(), Default::default());
    // add_child deadlocks on Windows when called from a synchronous command.
    let parent = app.get_window("main").ok_or_else(host_error)?;
    let webview = match parent.add_child(
        crate::webview_native::download_handler(builder).focused(false),
        tauri::LogicalPosition::new(bounds.x, bounds.y),
        tauri::LogicalSize::new(bounds.width, bounds.height),
    ) {
        Ok(webview) => webview,
        Err(_) => {
            windows
                .verify
                .lock()
                .map_err(|_| host_error())?
                .remove(&label);
            return Err(host_error());
        }
    };
    // Remember it before native setup so a failed close still blocks a second child.
    inner.current = Some(label.clone());
    if let Err(error) = crate::webview_native::configure(&webview, plan.route, bounds).await {
        destroy_webview(&app, &windows, &webview).await?;
        inner.current = None;
        return Err(error);
    }
    if webview.navigate(initial_url.clone()).is_err() {
        destroy_webview(&app, &windows, &webview).await?;
        inner.current = None;
        return Err(host_error());
    }
    publish_navigation(&webview, &initial_url);
    Ok(label)
}

fn valid_web_label(label: &str) -> bool {
    label
        .strip_prefix("web-")
        .is_some_and(|suffix| !suffix.is_empty() && suffix.bytes().all(|b| b.is_ascii_digit()))
}

pub(super) fn web_view(app: &tauri::AppHandle, label: &str) -> Result<tauri::Webview, String> {
    if !valid_web_label(label) {
        return Err(host_error());
    }
    app.get_webview(label).ok_or_else(host_error)
}

#[tauri::command]
pub async fn close_web_view(
    app: tauri::AppHandle,
    windows: tauri::State<'_, WebWindows>,
    label: String,
) -> Result<(), String> {
    let mut inner = windows.inner.lock().await;
    if !valid_web_label(&label) {
        return Err(host_error());
    }
    // Another close or a new open already dropped this child. Do not close twice.
    if inner.current.as_deref() != Some(label.as_str()) {
        return Ok(());
    }
    let Some(webview) = app.get_webview(&label) else {
        inner.current = None;
        return Ok(());
    };
    destroy_webview(&app, &windows, &webview).await?;
    inner.current = None;
    Ok(())
}

#[tauri::command]
pub fn set_web_view_bounds(
    app: tauri::AppHandle,
    label: String,
    bounds: WebViewBounds,
) -> Result<(), String> {
    let bounds = finite_bounds(bounds)?;
    let webview = web_view(&app, &label)?;
    crate::webview_native::place(&webview, bounds)
}

#[tauri::command]
pub fn set_web_view_visible(
    app: tauri::AppHandle,
    label: String,
    visible: bool,
) -> Result<(), String> {
    let webview = web_view(&app, &label)?;
    if visible {
        webview.show().map_err(|_| host_error())?;
        webview.set_focus().map_err(|_| host_error())?;
    } else {
        webview.hide().map_err(|_| host_error())?;
        app.get_webview("main")
            .ok_or_else(host_error)?
            .set_focus()
            .map_err(|_| host_error())?;
    }
    Ok(())
}

#[tauri::command]
pub async fn web_view_action(
    app: tauri::AppHandle,
    label: String,
    action: String,
) -> Result<(), String> {
    if action == "close" {
        return close_web_view(app.clone(), app.state(), label).await;
    }
    let webview = web_view(&app, &label)?;
    crate::webview_native::dispatch(&webview, &action).await
}

#[tauri::command]
pub async fn clear_web_data(
    app: tauri::AppHandle,
    windows: tauri::State<'_, WebWindows>,
) -> Result<(), String> {
    let mut inner = windows.inner.clone().lock_owned().await;
    close_current(&app, &windows, &mut inner).await?;
    // The owned guard keeps new windows out until cleanup finishes, even if IPC is dropped.
    rrss_app::execution::blocking(move || {
        inner
            .session
            .as_mut()
            .map_err(|error| error.to_string())?
            .clear(&uuid::Uuid::new_v4().to_string())
            .map_err(|_| "原网页数据未能完全删除，请重启 RRSS 后重试。".into())
    })
    .await
    .map_err(|_| host_error())?
}

/// Local-looking HTTP origins must not bypass the remote-origin ACL check:
/// custom app commands without an app manifest are available to local origins.
pub fn main_only(
    handler: impl Fn(tauri::ipc::Invoke<tauri::Wry>) -> bool + Send + Sync + 'static,
) -> impl Fn(tauri::ipc::Invoke<tauri::Wry>) -> bool + Send + Sync + 'static {
    move |invoke| {
        if invoke.message.webview_ref().label() != "main" {
            invoke.resolver.reject("此窗口无权调用应用命令");
            return true;
        }
        handler(invoke)
    }
}

command_handler![
    open_web_view,
    close_web_view,
    set_web_view_bounds,
    set_web_view_visible,
    web_view_action,
    clear_web_data,
];
