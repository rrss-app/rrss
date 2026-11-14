use tauri::State;

use rrss_app::queries;
use rrss_app::views::*;

use crate::state::{AppState, with_db};

db_task!(network_proxy() -> NetworkProxyView => queries::network_proxy);

db_task!(save_network_proxy(args: SaveNetworkProxyArgs) -> NetworkProxyView =>
    |conn| rrss_app::network_proxy::save_network_proxy(conn, &args));

#[tauri::command]
pub async fn test_network_proxy(
    state: State<'_, AppState>,
    args: NetworkProxyTestArgs,
) -> Result<NetworkProxyTestView, UiError> {
    let db_path = state.db_path.clone();
    rrss_app::network_proxy::test_network_proxy(&db_path, args)
        .await
        .map_err(UiError::from)
}

db_task!(pi_config() -> PiConfigView => queries::pi_config);

db_task!(save_pi_config(args: SavePiConfigArgs) -> PiConfigView =>
    |conn| rrss_app::actions::save_pi_config(conn, &args));

#[tauri::command]
pub async fn pi_generated_config(
    state: State<'_, AppState>,
) -> Result<rrss_app::pi::launch::GeneratedConfig, UiError> {
    let data_dir = state.data_dir.clone();
    with_db("pi_generated_config", state.db_path.clone(), move |conn| {
        rrss_app::pi::launch::generated_config(conn, &data_dir)
            .map_err(|e| e.message)
    })
    .await
}

#[tauri::command]
pub async fn pi_available_models(state: State<'_, AppState>) -> Result<Vec<String>, UiError> {
    state
        .chat
        .available_models()
        .await
        .map_err(|e| e.message)
}

#[tauri::command]
pub async fn acknowledge_pi_version_notice(
    state: State<'_, AppState>,
    version: String,
) -> Result<(), UiError> {
    state
        .chat
        .acknowledge_version_notice(&version)
        .await
        .map_err(|e| e.message)
}

#[tauri::command]
pub async fn pi_page(state: State<'_, AppState>) -> Result<PiPage, UiError> {
    let path = state.db_path.clone();
    let data_dir = state.data_dir.clone();
    with_db("pi_page", path, move |conn| {
        Ok(queries::pi_page(conn, &data_dir))
    })
    .await
}

/// 子进程探测单独走阻塞线程池，切到 Pi 页不再等 `pi --version`。
#[tauri::command]
pub async fn pi_probe(state: State<'_, AppState>) -> Result<PiProbe, UiError> {
    let path = state.db_path.clone();
    let data_dir = state.data_dir.clone();
    with_db("pi_probe", path, move |conn| {
        Ok(queries::pi_probe(conn, &data_dir))
    })
    .await
}

#[tauri::command]
pub async fn save_pi_settings(
    state: State<'_, AppState>,
    args: SavePiSettingsArgs,
) -> Result<PiPage, UiError> {
    let path = state.db_path.clone();
    let data_dir = state.data_dir.clone();
    with_db("save_pi_settings", path, move |conn| {
        rrss_app::actions::save_pi_settings(conn, &data_dir, &args).map_err(UiError::from)
    })
    .await
}

db_task!(toggle_pi_tool(name: String, enabled: bool) -> () =>
    |conn| rrss_app::actions::toggle_pi_tool(conn, &name, enabled));

db_task!(upsert_mcp(input: rrss_app::McpInput) -> i64 =>
    |conn| rrss_app::mcp::save(conn, &input));

db_task!(preview_mcp_import(json: String) -> Vec<rrss_app::McpImportEntry> =>
    |conn| rrss_app::mcp::preview_import(conn, &json));

db_task!(import_mcp(json: String) -> Vec<rrss_app::McpImportEntry> =>
    |conn| rrss_app::mcp::import(conn, &json));

#[tauri::command]
pub async fn delete_mcp_server(state: State<'_, AppState>, id: i64) -> Result<(), UiError> {
    rrss_app::mcp::delete(&state.db_path, &state.data_dir, id)
        .await
        .map_err(UiError::from)
}

#[tauri::command]
pub async fn toggle_mcp_server(
    state: State<'_, AppState>,
    id: i64,
    enabled: bool,
) -> Result<(), UiError> {
    let data_dir = state.data_dir.clone();
    with_db("toggle_mcp_server", state.db_path.clone(), move |conn| {
        rrss_app::mcp::toggle(conn, &data_dir, id, enabled).map_err(UiError::from)
    })
    .await
}

db_task!(toggle_skill(name: String, enabled: bool) -> () =>
    |conn| rrss_app::actions::toggle_skill(conn, &name, enabled));

db_task!(set_pi_tool_description(name: String, description: String) -> () =>
    |conn| rrss_app::actions::set_pi_tool_description(conn, &name, &description));

#[tauri::command]
pub async fn pi_skill(state: State<'_, AppState>, name: String) -> Result<SkillDraft, UiError> {
    let data_dir = state.data_dir.clone();
    rrss_app::execution::blocking(move || queries::pi_skill(&data_dir, &name))
        .await
        .map_err(UiError::from)?
        .map_err(UiError::from)
}

#[tauri::command]
pub async fn save_skill(state: State<'_, AppState>, draft: SkillDraft) -> Result<(), UiError> {
    let data_dir = state.data_dir.clone();
    with_db("save_skill", state.db_path.clone(), move |conn| {
        rrss_app::actions::save_skill(conn, &data_dir, &draft).map_err(UiError::from_anyhow_pretty)
    })
    .await
}

#[tauri::command]
pub async fn delete_skill(state: State<'_, AppState>, name: String) -> Result<(), UiError> {
    let data_dir = state.data_dir.clone();
    with_db("delete_skill", state.db_path.clone(), move |conn| {
        rrss_app::actions::delete_skill(conn, &data_dir, &name).map_err(UiError::from_anyhow_pretty)
    })
    .await
}

/// 用户显式操作：在后台下载或升级托管 Pi（不受「自动下载」开关限制），立即返回当前进度；
/// 进度与结论经 `pi:install` 事件推送，离开设置页不中断。
#[tauri::command]
pub fn install_pi(state: State<AppState>) -> PiInstallView {
    use rrss_app::pi::install;
    if install::begin_install(&state.data_dir) {
        let (db_path, data_dir) = (state.db_path.clone(), state.data_dir.clone());
        tauri::async_runtime::spawn(async move {
            if let Err(error) = install::ensure_pi_binary(&db_path, &data_dir, true).await {
                tracing::warn!(error = %format!("{error:#}"), "install managed pi");
            }
        });
    }
    install::install_status(&state.data_dir)
}

/// 当前（或最近一次）对话引擎下载进度；重新进入设置页时用它接上进度。
#[tauri::command]
pub fn pi_install_status(state: State<AppState>) -> PiInstallView {
    rrss_app::pi::install::install_status(&state.data_dir)
}

#[tauri::command]
pub async fn probe_mcp(
    state: State<'_, AppState>,
    input: rrss_app::McpInput,
) -> Result<rrss_app::McpProbeView, UiError> {
    let db_path = state.db_path.clone();
    let data_dir = state.data_dir.clone();
    Ok(rrss_app::mcp::probe(&db_path, &data_dir, input).await)
}

#[tauri::command]
pub async fn login_mcp(
    state: State<'_, AppState>,
    id: i64,
) -> Result<rrss_app::McpLoginResult, UiError> {
    let db_path = state.db_path.clone();
    let data_dir = state.data_dir.clone();
    Ok(rrss_app::mcp::login(&db_path, &data_dir, id).await)
}

#[tauri::command]
pub fn cancel_mcp_login(state: State<'_, AppState>, id: i64) {
    rrss_app::mcp::cancel_login(&state.data_dir, id);
}

command_handler![
    pi_page,
    pi_config,
    network_proxy,
    save_network_proxy,
    test_network_proxy,
    save_pi_config,
    pi_generated_config,
    pi_available_models,
    acknowledge_pi_version_notice,
    pi_probe,
    save_pi_settings,
    toggle_pi_tool,
    upsert_mcp,
    preview_mcp_import,
    import_mcp,
    delete_mcp_server,
    toggle_mcp_server,
    toggle_skill,
    set_pi_tool_description,
    pi_skill,
    save_skill,
    delete_skill,
    install_pi,
    pi_install_status,
    probe_mcp,
    login_mcp,
    cancel_mcp_login,
];
