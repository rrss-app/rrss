use gtk::prelude::{BoxExt, Cast, ContainerExt, OverlayExt};
use rrss_core::network::WebviewRoute;
use std::{cell::RefCell, rc::Rc};
use tauri::{Manager, Webview};
use webkit2gtk::{
    ContextMenuExt, DownloadExt, HitTestResultExt, NetworkProxyMode, PermissionRequestExt,
    SettingsExt, URIRequestExt, WebContextExt, WebViewExt, WebsiteDataManagerExt, gio,
    gio::prelude::*,
};

pub(super) fn configure(
    handle: tauri::webview::PlatformWebview,
    window: &Webview,
    route: &WebviewRoute,
    bounds: rrss_app::WebViewBounds,
) -> Result<(), String> {
    let view = handle.inner();
    let context = view.context().ok_or("原网页缺少 WebKit context")?;
    if *route == WebviewRoute::Direct {
        // wry 0.57 new_gtk sets Custom on this same data manager before
        // create_webview, then load_uri. We create only about:blank until here.
        context
            .website_data_manager()
            .ok_or("原网页缺少数据管理器")?
            .set_network_proxy_settings(NetworkProxyMode::NoProxy, None);
    }
    // Async Clipboard (WebKit >= 2.42) emits permission-request; wry's Other
    // branch already denies it. Also disable legacy execCommand paste access.
    view.settings()
        .ok_or("原网页缺少 WebKit settings")?
        .set_javascript_can_access_clipboard(false);
    view.connect_permission_request(|_, request| {
        request.deny();
        true
    });
    let target = window.clone();
    view.connect_context_menu(move |_, menu, _, hit| {
        menu.remove_all();
        if let Some(url) = hit.link_uri() {
            for (id, text, copy) in [
                ("open", "在系统浏览器打开", false),
                ("copy", "复制链接", true),
            ] {
                let action = gio::SimpleAction::new(id, None);
                let target = target.clone();
                let url = url.to_string();
                action.connect_activate(move |_, _| super::link_action(&target, &url, copy));
                menu.append(&webkit2gtk::ContextMenuItem::from_gaction(
                    &action, text, None,
                ));
            }
            false
        } else {
            true
        } // Suppress all non-link defaults, including inspect.
    });
    let owner = view.downgrade();
    let app = window.app_handle().clone();
    let label = window.label().to_owned();
    let escape_app = app.clone();
    let escape_label = label.clone();
    // Weak references avoid retaining downloads or webviews in the shared context.
    let last = Rc::new(RefCell::new(
        None::<webkit2gtk::glib::WeakRef<webkit2gtk::Download>>,
    ));
    context.connect_download_started(move |_, download| {
        let is_owner = owner
            .upgrade()
            .is_some_and(|view| download.web_view().as_ref() == Some(&view));
        let seen = last.borrow().as_ref().and_then(|d| d.upgrade()).as_ref() == Some(download);
        if should_notify(is_owner, seen) {
            *last.borrow_mut() = Some(download.downgrade());
            if let Some(url) = download.request().and_then(|request| request.uri()) {
                super::emit_main(
                    &app,
                    "web:download-blocked",
                    rrss_app::WebDownloadBlocked {
                        label: label.clone(),
                        url: url.to_string(),
                    },
                );
            }
        }
        // Every callback denies, including downloads with no associated view.
        download.cancel();
    });
    // Tauri packs this child into the window GtkBox beside the main webview.
    // Move both into one Overlay before any remote navigation.
    embed(&view, bounds)?;
    gtk::prelude::WidgetExt::connect_key_press_event(&view, move |_, event| {
        if event.keyval() == gtk::gdk::keys::constants::Escape && no_modifier(event) {
            super::emit_escape(&escape_app, &escape_label);
            gtk::glib::Propagation::Stop
        } else {
            gtk::glib::Propagation::Proceed
        }
    });
    Ok(())
}

const OVERLAY_NAME: &str = "rrss-web-overlay";

fn no_modifier(event: &gtk::gdk::EventKey) -> bool {
    !event.state().intersects(
        gtk::gdk::ModifierType::SHIFT_MASK
            | gtk::gdk::ModifierType::CONTROL_MASK
            | gtk::gdk::ModifierType::MOD1_MASK
            | gtk::gdk::ModifierType::MOD3_MASK
            | gtk::gdk::ModifierType::MOD4_MASK
            | gtk::gdk::ModifierType::MOD5_MASK
            | gtk::gdk::ModifierType::SUPER_MASK
            | gtk::gdk::ModifierType::HYPER_MASK
            | gtk::gdk::ModifierType::META_MASK,
    )
}

fn embed(view: &webkit2gtk::WebView, bounds: rrss_app::WebViewBounds) -> Result<(), String> {
    use gtk::prelude::WidgetExt;
    let parent = view.parent().ok_or("原网页缺少父容器")?;
    let box_ = parent
        .downcast::<gtk::Box>()
        .map_err(|_| "原网页父容器不是 GtkBox")?;
    let mut children = Vec::new();
    box_.foreach(|child| children.push(child.clone()));
    let overlay = if let Some(existing) = children.iter().find_map(|child| {
        (child.widget_name().as_str() == OVERLAY_NAME)
            .then(|| child.clone().downcast::<gtk::Overlay>().ok())
            .flatten()
    }) {
        existing
    } else {
        let main = children
            .iter()
            .find_map(|child| {
                if child == view.upcast_ref::<gtk::Widget>() {
                    return None;
                }
                child.clone().downcast::<webkit2gtk::WebView>().ok()
            })
            .ok_or("原网页找不到主界面")?;
        let index = children
            .iter()
            .position(|child| child == main.upcast_ref::<gtk::Widget>())
            .ok_or("原网页找不到主界面")?;
        // Keep a Rust reference across unparent so GTK does not finalize the main view.
        box_.remove(&main);
        let overlay = gtk::Overlay::new();
        overlay.set_widget_name(OVERLAY_NAME);
        overlay.add(&main);
        box_.pack_start(&overlay, true, true, 0);
        box_.reorder_child(&overlay, index as i32);
        main.show();
        overlay.show();
        overlay
    };
    box_.remove(view);
    overlay.add_overlay(view);
    view.set_halign(gtk::Align::Start);
    view.set_valign(gtk::Align::Start);
    view.set_hexpand(false);
    view.set_vexpand(false);
    position(view, bounds);
    view.show();
    Ok(())
}

fn position(view: &webkit2gtk::WebView, bounds: rrss_app::WebViewBounds) {
    use gtk::prelude::WidgetExt;
    view.set_margin_start(bounds.x.round() as i32);
    view.set_margin_top(bounds.y.round() as i32);
    view.set_size_request(
        (bounds.width.round() as i32).max(1),
        (bounds.height.round() as i32).max(1),
    );
}

pub(super) fn place(webview: &Webview, bounds: rrss_app::WebViewBounds) -> Result<(), String> {
    webview
        .with_webview(move |handle| position(&handle.inner(), bounds))
        .map_err(|_| super::failure())
}

fn should_notify(is_owner: bool, seen: bool) -> bool {
    is_owner && !seen
}

pub(super) fn history(window: &Webview, back: bool) -> tauri::Result<()> {
    window.with_webview(move |handle| {
        if back {
            handle.inner().go_back();
        } else {
            handle.inner().go_forward();
        }
    })
}

pub(super) fn page_title(
    handle: tauri::webview::PlatformWebview,
) -> Result<Option<String>, String> {
    Ok(handle.inner().title().map(|s| s.to_string()))
}

pub(super) fn snapshot(
    handle: tauri::webview::PlatformWebview,
    mut send: tokio::sync::oneshot::Sender<Result<Vec<u8>, String>>,
) {
    let view = handle.inner();
    // Neither WebView nor InputStream crosses threads. GioFuture owns its object
    // and cancels its GCancellable when dropped on navigation/timeout/closure.
    webkit2gtk::glib::MainContext::default().spawn_local(async move {
        let stream = tokio::select! {
            _ = send.closed() => return,
            result = view.save_future(webkit2gtk::SaveMode::Mhtml) => result,
        };
        let result = match stream {
            Err(_) => Err("WebKit 网页快照失败".into()),
            Ok(stream) => {
                let result = tokio::select! {
                    _ = send.closed() => Err("网页快照已取消".into()),
                    result = read_snapshot(&stream) => result,
                };
                let _ = stream
                    .close_future(webkit2gtk::glib::Priority::DEFAULT)
                    .await;
                result
            }
        };
        let _ = send.send(result);
    });
}

async fn read_snapshot(stream: &gio::InputStream) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    loop {
        let chunk = stream
            .read_bytes_future(64 * 1024, webkit2gtk::glib::Priority::DEFAULT)
            .await
            .map_err(|_| "WebKit 网页快照读取失败")?;
        if bytes.len() + chunk.len() > rrss_app::web_snapshot::MAX_SNAPSHOT_BYTES {
            return Err("快照超过 30 MiB 上限".into());
        }
        if chunk.is_empty() {
            return Ok(bytes);
        }
        bytes.extend_from_slice(&chunk);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn shared_context_notifies_only_once_for_the_originating_view() {
        assert!(super::should_notify(true, false));
        assert!(!super::should_notify(false, false));
        assert!(!super::should_notify(false, true));
        assert!(!super::should_notify(true, true));
    }
}

/// 窗口图标沿用 Tauri 默认（512 px PNG，由桌面环境缩放）。
pub(super) fn window_icons(_: &tauri::WebviewWindow) {}
