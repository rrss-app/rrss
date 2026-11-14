//! Host scheduling only; MIME, extraction, persistence and state policy are app code.
use std::sync::{Arc, Mutex};

use rrss_app::{UiError, WebVerifyState, WebVerifyStateEvent, web_verify};
use tauri::{Manager, Webview};
use tokio::sync::watch;

pub(super) struct Verification {
    revision: watch::Sender<u64>,
    published: Mutex<Option<WebVerifyStateEvent>>,
    capture: Arc<tokio::sync::Mutex<()>>,
}

impl Default for Verification {
    fn default() -> Self {
        Self {
            revision: watch::channel(0).0,
            published: Mutex::new(None),
            capture: Arc::new(tokio::sync::Mutex::new(())),
        }
    }
}

impl Verification {
    pub(super) fn cancel(&self) {
        let _guard = self.published.lock().unwrap_or_else(|e| e.into_inner());
        self.revision.send_modify(|n| *n += 1);
    }
}

fn session(window: &Webview) -> Result<Arc<Verification>, String> {
    window
        .state::<super::WebWindows>()
        .verify
        .lock()
        .map_err(|_| "网页抽取状态不可用")?
        .get(window.label())
        .cloned()
        .ok_or_else(|| "原网页已关闭".into())
}

pub(super) fn navigated(window: &Webview) {
    if let Ok(session) = session(window) {
        session.cancel();
    }
}

pub(super) fn finished(window: Webview, url: &tauri::Url) {
    if !rrss_core::url_norm::is_http_url(url) {
        return;
    }
    if let Ok(session) = session(&window) {
        let revision = session.revision.subscribe();
        let url = url.to_string();
        tauri::async_runtime::spawn(async move {
            let _ = run(window, session, revision, url, true).await;
        });
    }
}

fn publish(
    window: &Webview,
    session: &Verification,
    revision: &watch::Receiver<u64>,
    event: &WebVerifyStateEvent,
) {
    let mut previous = session.published.lock().unwrap_or_else(|e| e.into_inner());
    if revision.has_changed().unwrap_or(true) {
        return;
    }
    if previous.as_ref() == Some(event) {
        return;
    }
    super::web_window::emit_main(window.app_handle(), "web:verify-state", event);
    *previous = Some(event.clone());
}

async fn run(
    window: Webview,
    session: Arc<Verification>,
    mut revision: watch::Receiver<u64>,
    url: String,
    automatic: bool,
) -> Result<WebVerifyStateEvent, String> {
    let ticket = revision.clone();
    tokio::select! {
        biased;
        _ = revision.changed() => Err("页面已导航或窗口已关闭，抽取已取消".into()),
        result = async {
            if window.url().ok().as_ref().map(|u| u.as_str()) != Some(url.as_str()) {
                return Err("页面地址已改变，抽取已取消".into());
            }
            let title = crate::webview_native::page_title(&window).await;
            let title_error = title.as_ref().err().cloned().map(Err);
            let title = title.ok().flatten();
            let event = web_verify::event(window.label(), &url, title.clone(), title_error.as_ref());
            publish(&window, &session, &ticket, &event);
            if title_error.is_some() || (automatic && event.state == WebVerifyState::WaitingVerification) {
                return Ok(event);
            }
            if automatic { tokio::time::sleep(std::time::Duration::from_millis(1500)).await; }
            let capture = session.capture.clone().lock_owned().await;
            let result = match crate::webview_native::snapshot_mhtml(&window).await {
                Err(error) => Err(error),
                Ok(bytes) => {
                    // Compare again after the native operation; no snapshot may be
                    // attributed to a different URL after a redirect/navigation.
                    if window.url().ok().as_ref().map(|u| u.as_str()) != Some(url.as_str()) {
                        return Err("页面地址已改变，抽取已取消".into());
                    }
                    let state = window.state::<crate::AppState>();
                    let (db, data, page) = (state.db_path.clone(), state.data_dir.clone(), url.clone());
                    let ticket = ticket.clone();
                    tokio::task::spawn_blocking(move || {
                        // A blocking store cannot be aborted once started. Keep
                        // its lock even if the observer is cancelled, so an older
                        // document can never overwrite a newer manual extraction.
                        let _capture = capture;
                        if ticket.has_changed().unwrap_or(true) {
                            return Err("页面已改变，抽取已取消".into());
                        }
                        rrss_app::web_snapshot::store_snapshot(&db, &data, &page, &bytes)
                    })
                        .await.unwrap_or_else(|_| Err("网页快照处理失败".into()))
                }
            };
            let event = web_verify::event(window.label(), &url, title, Some(&result));
            publish(&window, &session, &ticket, &event);
            Ok(event)
        } => result,
    }
}

#[tauri::command]
pub async fn extract_web_view(
    app: tauri::AppHandle,
    label: String,
) -> Result<WebVerifyStateEvent, UiError> {
    let window = super::web_window::web_view(&app, &label)?;
    let session = session(&window).map_err(UiError::from)?;
    session.cancel(); // Manual requests replace a pending automatic delay/extraction.
    let revision = session.revision.subscribe();
    let url = window
        .url()
        .map_err(|_| UiError::from("无法读取原网页地址".to_string()))?
        .to_string();
    run(window, session, revision, url, false)
        .await
        .map_err(UiError::from)
}

command_handler![extract_web_view,];
