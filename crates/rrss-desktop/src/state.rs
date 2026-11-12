use std::path::PathBuf;
use std::sync::Arc;

use rrss_app::pi::ChatRuntime;

pub struct AppState {
    pub db_path: PathBuf,
    pub data_dir: PathBuf,
    pub scheduler: rrss_app::scheduler::Scheduler,
    pub network: rrss_app::network::NetworkClients,
    pub chat: Arc<ChatRuntime>,
    pub translation: Arc<rrss_app::translate::runtime::Runtime>,
}

pub fn tool_path() -> PathBuf {
    std::env::current_exe().unwrap_or_else(|_| PathBuf::from("rrss"))
}

pub async fn with_db<T, F>(command: &'static str, db_path: PathBuf, f: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce(&rrss_core::db::PooledConnection) -> Result<T, String> + Send + 'static,
{
    crate::commands::logged(command, async move {
        // 只有开库和工作线程失败记 warn；业务错误照常返回，由 CommandLog 记一条 info（Release 只写 warn）。
        rrss_app::execution::blocking(move || {
            let conn = rrss_core::db::open_at(&db_path).map_err(|error| {
                tracing::warn!(
                    command = rrss_core::logsafe::unless_in_span("command", command),
                    error = %rrss_app::log::error(&error),
                    "command database open failed"
                );
                error.to_string()
            })?;
            f(&conn)
        })
        .await
        .map_err(|error| {
            tracing::warn!(
                command = rrss_core::logsafe::unless_in_span("command", command),
                error = %rrss_app::log::error(error.as_ref()),
                "command worker failed"
            );
            error.to_string()
        })?
    })
    .await
}
