use std::sync::Arc;

use rrss_app::network::NetworkClients;
use rrss_app::queries;
use rrss_core::db;
use tauri::Manager;

pub mod commands;
pub mod events;
mod mcp_debug;
pub mod protocol;
pub mod state;
mod tray;
mod webview_native;

pub use commands::*;
pub use events::TauriChatSink;
pub use protocol::media_payload;
pub use state::{AppState, tool_path};

/// 启动桌面宿主。
/// 沙箱配置与日志初始化在二进制入口完成。
pub fn run() -> anyhow::Result<()> {
    let data_dir = rrss_core::data_dir()?;
    std::fs::create_dir_all(&data_dir)?;
    let db_path = rrss_core::db_path()?;
    // 恢复与升级前备份都要在建连接池之前：恢复要换库文件，备份要赶在迁移前。
    if let Some(result) = rrss_core::backup::apply_pending(&data_dir, &db_path)? {
        tracing::info!(ok = result.ok, message = %result.message, "pending restore handled");
    }
    rrss_core::backup::before_upgrade(&db_path, &data_dir, rrss_app::actions::APP_VERSION)?;
    let _ = db::open_at(&db_path)?;
    let network = NetworkClients::new(&db_path);

    let web_windows = WebWindows::new(rrss_app::web_window::WebSession::initialize(
        &data_dir,
        &uuid::Uuid::new_v4().to_string(),
    ));
    let protocol_db = db_path.clone();
    // 单实例要最先注册：再次启动时不开第二个进程，唤回已有窗口。
    mcp_debug::attach(
        tauri::Builder::default().plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            tray::show_main(app)
        })),
    )
    .manage(web_windows)
    .manage(tray::Tray::default())
    .on_window_event(|window, event| {
        if window.label() != "main" {
            return;
        }
        match event {
            // 关闭先拦下：按设置询问、隐藏到托盘或退出（阶段 14 · 后台驻留）。
            tauri::WindowEvent::CloseRequested { api, .. } => {
                api.prevent_close();
                tray::close_requested(window.app_handle());
            }
            tauri::WindowEvent::Destroyed => window.app_handle().exit(0),
            _ => {}
        }
    })
    .plugin(tauri_plugin_dialog::init())
    .plugin(tauri_plugin_clipboard_manager::init())
    .plugin(tauri_plugin_opener::init())
    .register_asynchronous_uri_scheme_protocol("rrss", move |ctx, request, responder| {
        let db_path = protocol_db.clone();
        let allowed = rrss_core::media::media_window_allowed(ctx.webview_label());
        tauri::async_runtime::spawn(async move {
            let mut response = if !allowed {
                tauri::http::Response::builder()
                    .status(403)
                    .body(Vec::new())
                    .unwrap()
            } else {
                let path = request.uri().path().to_string();
                let range = request
                    .headers()
                    .get(tauri::http::header::RANGE)
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_string);
                let id = queries::media_id_from_path(&path);
                let result = match id {
                    Some(id) => media_payload(id, &db_path, range.as_deref()),
                    None => Err(format!("bad path: {path}")),
                };
                match result {
                    Ok(payload) => {
                        let mut builder = tauri::http::Response::builder()
                            .status(payload.status)
                            .header(tauri::http::header::CONTENT_TYPE, payload.mime)
                            .header(tauri::http::header::CONTENT_LENGTH, payload.body.len())
                            .header(
                                tauri::http::header::CACHE_CONTROL,
                                "public, max-age=31536000, immutable",
                            );
                        if payload.accept_ranges {
                            builder = builder.header(tauri::http::header::ACCEPT_RANGES, "bytes");
                        }
                        if let Some(range) = payload.content_range {
                            builder = builder.header(tauri::http::header::CONTENT_RANGE, range);
                        }
                        builder
                            .body(payload.body)
                            .unwrap_or_else(|_| tauri::http::Response::new(Vec::new()))
                    }
                    Err(_) => tauri::http::Response::builder()
                        .status(404)
                        .body(Vec::new())
                        .unwrap(),
                }
            };
            for (name, value) in rrss_core::media::MEDIA_SECURITY_HEADERS {
                response.headers_mut().insert(
                    name.parse::<tauri::http::HeaderName>().unwrap(),
                    value.parse().unwrap(),
                );
            }
            responder.respond(response);
        });
    })
    .setup(move |app| {
        // 对话运行时要在 tokio 运行时里建（它会起事件泵与空闲回收任务）。
        let chat = tauri::async_runtime::block_on({
            let cfg = rrss_app::pi::SupervisorConfig {
                db_path: db_path.clone(),
                data_dir: data_dir.clone(),
                tool_path: state::tool_path(),
                idle_tick: std::time::Duration::from_secs(5),
            };
            let network = network.clone();
            let sink = Arc::new(events::TauriChatSink {
                app: app.handle().clone(),
            });
            async move { rrss_app::pi::ChatRuntime::start(cfg, network, sink, true) }
        });
        let vector_app = app.handle().clone();
        rrss_app::jobs::vector_space::set_notifier(move |view| {
            use tauri::Emitter;
            if let Err(error) = vector_app.emit("vector-space:stale", view) {
                tracing::warn!(%error, "emit vector space warning");
            }
        });
        let scheduler = tauri::async_runtime::block_on(async {
            let feed_app = app.handle().clone();
            rrss_app::scheduler::Scheduler::start(db_path.clone(), network.clone(), move |event| {
                use tauri::Emitter;
                if let Err(error) = feed_app.emit("feed:updated", event) {
                    tracing::warn!(%error, "emit feed update");
                }
            })
        });
        let scheduled = scheduler.clone();
        let scheduler_app = app.handle().clone();
        scheduler.set_observer(move |snapshot| {
            use tauri::Emitter;
            if let Err(error) = scheduler_app.emit("scheduler:updated", snapshot) {
                tracing::warn!(%error, "emit scheduler update");
            }
        });
        tauri::async_runtime::spawn(async move {
            scheduled.run_loop().await;
        });
        let install_app = app.handle().clone();
        rrss_app::pi::install::set_install_notifier(move |view| {
            use tauri::Emitter;
            if let Err(error) = install_app.emit("pi:install", view) {
                tracing::warn!(%error, "emit pi install progress");
            }
        });
        let db_pi = db_path.clone();
        let data_pi = data_dir.clone();
        tauri::async_runtime::spawn(async move {
            let _ = rrss_app::pi::install::ensure_pi_binary(&db_pi, &data_pi, false).await;
        });
        let translation = rrss_app::translate::runtime::Runtime::new(
            &db_path,
            Arc::new(events::TauriTranslationSink {
                app: app.handle().clone(),
            }),
        )?;
        if let Some(main) = app.get_webview_window("main") {
            webview_native::window_icons(&main);
        }
        app.manage(AppState {
            db_path,
            data_dir,
            network,
            discovery: rrss_app::discover::Runtime::default(),
            scheduler,
            chat,
            translation,
        });
        Ok(())
    })
    .invoke_handler(main_only(commands::invoke_handler))
    .build(tauri::generate_context!())?
    .run(|app, event| {
        if matches!(event, tauri::RunEvent::Exit) {
            let scheduler = app.state::<AppState>().scheduler.clone();
            if !tauri::async_runtime::block_on(scheduler.shutdown()) {
                tracing::warn!(
                    error_kind = "shutdown_timeout",
                    "background shutdown grace period expired"
                );
            }
        }
        if matches!(event, tauri::RunEvent::Exit)
            && let Ok(windows) = app.state::<WebWindows>().inner.try_lock()
        {
            if let Ok(session) = &windows.session {
                let _ = session.cleanup();
            }
        }
        if matches!(event, tauri::RunEvent::Exit) {
            rrss_app::log::end_session();
        }
    });
    Ok(())
}
