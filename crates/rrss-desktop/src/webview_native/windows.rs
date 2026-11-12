use rrss_core::network::WebviewRoute;
use tauri::{Manager, Webview};
use webview2_com::{
    AcceleratorKeyPressedEventHandler, CallDevToolsProtocolMethodCompletedHandler,
    ContextMenuRequestedEventHandler, CustomItemSelectedEventHandler,
    Microsoft::Web::WebView2::Win32::*, PermissionRequestedEventHandler,
    ScreenCaptureStartingEventHandler, take_pwstr,
};
use windows::core::{HSTRING, Interface, PWSTR};

pub(super) fn configure(
    handle: tauri::webview::PlatformWebview,
    window: &Webview,
    _: &WebviewRoute,
    _: rrss_app::WebViewBounds,
) -> Result<(), String> {
    // COM calls run on Tauri's UI thread. The initial document is about:blank;
    // any unsupported interface or failed registration aborts before navigation.
    unsafe { install(handle, window) }
        .map_err(|_| "WebView2 原生功能不可用，请更新 WebView2 Runtime 后重试。".into())
}

unsafe fn install(
    handle: tauri::webview::PlatformWebview,
    window: &Webview,
) -> windows::core::Result<()> {
    unsafe {
        let view = handle.controller().CoreWebView2()?;
        let mut token = 0;
        // Deny every native permission, including future kinds absent from wry.
        view.add_PermissionRequested(
            &PermissionRequestedEventHandler::create(Box::new(|_, args| {
                if let Some(args) = args {
                    args.SetState(COREWEBVIEW2_PERMISSION_STATE_DENY)?;
                }
                Ok(())
            })),
            &mut token,
        )?;
        // getDisplayMedia uses a distinct event, not PermissionRequested.
        view.cast::<ICoreWebView2_27>()?.add_ScreenCaptureStarting(
            &ScreenCaptureStartingEventHandler::create(Box::new(|_, args| {
                if let Some(args) = args {
                    args.SetCancel(true)?;
                }
                Ok(())
            })),
            &mut token,
        )?;
        let escape_app = window.app_handle().clone();
        let escape_label = window.label().to_owned();
        handle.controller().add_AcceleratorKeyPressed(
            &AcceleratorKeyPressedEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else {
                    return Ok(());
                };
                let mut kind = COREWEBVIEW2_KEY_EVENT_KIND(0);
                args.KeyEventKind(&mut kind)?;
                let mut virtual_key = 0u32;
                args.VirtualKey(&mut virtual_key)?;
                if (kind.0 == COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN.0
                    || kind.0 == COREWEBVIEW2_KEY_EVENT_KIND_SYSTEM_KEY_DOWN.0)
                    && virtual_key == 0x1B
                {
                    args.SetHandled(true)?;
                    super::emit_escape(&escape_app, &escape_label);
                }
                Ok(())
            })),
            &mut token,
        )?;
        let environment = handle.environment().cast::<ICoreWebView2Environment9>()?;
        let target_window = window.clone();
        view.cast::<ICoreWebView2_11>()?.add_ContextMenuRequested(
            &ContextMenuRequestedEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else { return Ok(()) };
                // Fail closed: suppress the browser menu until replacement is complete.
                args.SetHandled(true)?;
                let items = args.MenuItems()?;
                let mut count = 0;
                items.Count(&mut count)?;
                for _ in 0..count {
                    items.RemoveValueAtIndex(0)?;
                }
                let target = args.ContextMenuTarget()?;
                let mut has_link = windows::core::BOOL::default();
                target.HasLinkUri(&mut has_link)?;
                if !has_link.as_bool() {
                    return Ok(());
                }
                let mut uri = PWSTR::null();
                target.LinkUri(&mut uri)?;
                let url = take_pwstr(uri);
                for (index, (text, copy)) in [("在系统浏览器打开", false), ("复制链接", true)]
                    .into_iter()
                    .enumerate()
                {
                    let item = environment.CreateContextMenuItem(
                        &HSTRING::from(text),
                        None::<&windows::Win32::System::Com::IStream>,
                        COREWEBVIEW2_CONTEXT_MENU_ITEM_KIND_COMMAND,
                    )?;
                    let window = target_window.clone();
                    let url = url.clone();
                    item.add_CustomItemSelected(
                        &CustomItemSelectedEventHandler::create(Box::new(move |_, _| {
                            super::link_action(&window, &url, copy);
                            Ok(())
                        })),
                        &mut 0,
                    )?;
                    items.InsertValueAtIndex(index as u32, &item)?;
                }
                args.SetHandled(false)?;
                Ok(())
            })),
            &mut token,
        )?;
        Ok(())
    }
}

pub(super) fn history(window: &Webview, back: bool) -> tauri::Result<()> {
    window.with_webview(move |handle| unsafe {
        if let Ok(view) = handle.controller().CoreWebView2() {
            let _ = if back {
                view.GoBack()
            } else {
                view.GoForward()
            };
        }
    })
}

pub(super) fn page_title(
    handle: tauri::webview::PlatformWebview,
) -> Result<Option<String>, String> {
    unsafe {
        let mut title = PWSTR::null();
        handle
            .controller()
            .CoreWebView2()
            .and_then(|view| view.DocumentTitle(&mut title))
            .map_err(|_| "WebView2 网页标题读取失败")?;
        Ok(Some(take_pwstr(title)))
    }
}

pub(super) fn snapshot(
    handle: tauri::webview::PlatformWebview,
    send: tokio::sync::oneshot::Sender<Result<Vec<u8>, String>>,
) {
    // Invocation and callback stay on the creating STA/UI thread. The helper
    // copies PCWSTR into an owned String before the COM callback returns.
    unsafe {
        if let Ok(view) = handle.controller().CoreWebView2() {
            let _ = view.CallDevToolsProtocolMethod(
                &HSTRING::from("Page.captureSnapshot"),
                &HSTRING::from(r#"{"format":"mhtml"}"#),
                &CallDevToolsProtocolMethodCompletedHandler::create(Box::new(
                    move |status, json| {
                        tauri::async_runtime::spawn_blocking(move || {
                            #[derive(serde::Deserialize)]
                            struct Snapshot {
                                data: String,
                            }
                            let result = (|| {
                                status.map_err(|_| "WebView2 网页快照失败")?;
                                // JSON escapes can expand each byte to six ASCII bytes.
                                if json.len()
                                    > rrss_app::web_snapshot::MAX_SNAPSHOT_BYTES * 6 + 1024
                                {
                                    return Err("快照超过 30 MiB 上限".into());
                                }
                                let snapshot: Snapshot = serde_json::from_str(&json)
                                    .map_err(|_| "WebView2 网页快照结果损坏")?;
                                if snapshot.data.len() > rrss_app::web_snapshot::MAX_SNAPSHOT_BYTES
                                {
                                    return Err("快照超过 30 MiB 上限".into());
                                }
                                Ok(snapshot.data.into_bytes())
                            })();
                            let _ = send.send(result);
                        });
                        Ok(())
                    },
                )),
            );
        }
    }
}

/// tauri-build 把 `icons/icon.ico`（16–256 px 多尺寸）嵌进 exe 时用的资源 ID。
const APP_ICON_RESOURCE: u16 = 32512;

/// 按窗口当前 DPI，从 exe 内嵌的多尺寸图标里取任务栏（大）与标题栏（小）两种尺寸，
/// 让系统用现成的那一层，而不是把 256 px 位图临时缩小。取不到时保留 Tauri 设的图标。
/// 窗口移到 DPI 不同的显示器时不重取；图标随主窗口存活到进程结束，不释放。
pub(super) fn window_icons(window: &tauri::WebviewWindow) {
    use windows::Win32::Foundation::{HINSTANCE, LPARAM, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
    use windows::Win32::UI::WindowsAndMessaging::{
        ICON_BIG, ICON_SMALL, IMAGE_ICON, LR_DEFAULTCOLOR, LoadImageW, SM_CXICON, SM_CXSMICON,
        SendMessageW, WM_SETICON,
    };
    use windows::core::PCWSTR;

    let Ok(hwnd) = window.hwnd() else { return };
    let Ok(module) = (unsafe { GetModuleHandleW(PCWSTR::null()) }) else {
        return;
    };
    let instance = HINSTANCE(module.0);
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    for (kind, metric) in [(ICON_BIG, SM_CXICON), (ICON_SMALL, SM_CXSMICON)] {
        let size = unsafe { GetSystemMetricsForDpi(metric, dpi) };
        let name = PCWSTR(APP_ICON_RESOURCE as usize as *const u16);
        let Ok(icon) = (unsafe {
            LoadImageW(
                Some(instance),
                name,
                IMAGE_ICON,
                size,
                size,
                LR_DEFAULTCOLOR,
            )
        }) else {
            continue;
        };
        unsafe {
            SendMessageW(
                hwnd,
                WM_SETICON,
                Some(WPARAM(kind as usize)),
                Some(LPARAM(icon.0 as isize)),
            );
        }
    }
}
