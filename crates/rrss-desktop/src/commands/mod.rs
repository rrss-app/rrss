//! 命令只做适配：查询 / 动作在 `rrss_app`。纯开库读写用 `db_task!` 生成，
//! 需要网络、事件或应用句柄的命令照常手写。

/// 数据库工作统一放到阻塞线程；调用方负责 await 有顺序依赖的命令。
macro_rules! db_task {
    ($(#[$meta:meta])* $name:ident($($arg:ident: $ty:ty),* $(,)?) -> $ret:ty => $body:expr) => {
        $(#[$meta])*
        #[tauri::command]
        pub async fn $name(state: tauri::State<'_, crate::state::AppState>, $($arg: $ty),*) -> Result<$ret, String> {
            let path = state.db_path.clone();
            crate::state::with_db(stringify!($name), path, move |conn| ($body)(conn).map_err(|e| e.to_string())).await
        }
    };
}

pub(crate) async fn logged<T, E: std::fmt::Display>(
    name: &'static str,
    future: impl std::future::Future<Output = Result<T, E>>,
) -> Result<T, E> {
    use tracing::Instrument as _;
    let log = rrss_app::log::CommandLog::start(name);
    let result = future.instrument(log.span().clone()).await;
    log.finish(&result);
    result
}

/// 同一份模块清单生成 Tauri 处理器和分派用的命令名；不改变命令的执行方式。
macro_rules! command_handler {
    ($($name:ident),* $(,)?) => {
        pub(super) const COMMAND_NAMES: &[&str] = &[$(stringify!($name)),*];

        pub(super) fn handle(invoke: tauri::ipc::Invoke<tauri::Wry>) -> bool {
            let handler: fn(tauri::ipc::Invoke<tauri::Wry>) -> bool =
                tauri::generate_handler![$($name),*];
            handler(invoke)
        }
    };
}

/// 只按模块装配；已有模块增删命令无需改这里或 lib.rs。
macro_rules! command_modules {
    ($($vis:vis mod $module:ident;)*) => {
        $($vis mod $module;)*

        pub fn invoke_handler(invoke: tauri::ipc::Invoke<tauri::Wry>) -> bool {
            let command = invoke.message.command();
            $(if $module::COMMAND_NAMES.contains(&command) {
                return $module::handle(invoke);
            })*
            false
        }
    };
}

command_modules! {
    pub mod ai;
    pub mod backup;
    pub mod chat;
    pub mod feeds;
    pub mod focus;
    pub mod memories;
    pub mod pi;
    pub mod reader;
    pub mod rules;
    pub mod search;
    pub mod signals;
    pub mod system;
    pub mod translate;
    mod web_verify;
    pub mod web_window;
    pub mod window;
}
pub use web_verify::extract_web_view;

pub use ai::*;
pub use backup::*;
pub use chat::*;
pub use feeds::*;
pub use focus::*;
pub use memories::*;
pub use pi::*;
pub use reader::*;
pub use rules::*;
pub use search::*;
pub use signals::*;
pub use system::*;
pub use translate::*;
pub use web_window::*;
pub use window::*;
