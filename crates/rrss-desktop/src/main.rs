// Windows 发布版是图形界面程序：从开始菜单或快捷方式启动时不带出控制台窗口，日志只进日志文件。
// 调试版保留控制台，开发时能直接看到输出。其他平台忽略这个属性。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if rrss_app::tool::is_tool_invocation(&args) {
        rrss_core::platform::attach_parent_console();
        rrss_app::log::init(rrss_app::log::Mode::Tool);
        std::process::exit(rrss_app::tool::run(&args));
    }
    // SAFETY: main 尚未启动运行时、WebKit 或日志工作线程。
    let sandbox = unsafe { rrss_core::platform::init_webkit_sandbox() };
    rrss_app::log::init_desktop();
    if let Some(status) = sandbox
        && !status.requested
    {
        tracing::warn!(
            error = status.reason,
            "webkit sandbox disabled; continuing without isolation"
        );
    }
    if let Err(err) = rrss_desktop_lib::run() {
        tracing::error!(error = %rrss_app::log::error(err.as_ref()), "desktop startup failed");
        std::process::exit(1);
    }
}
