use rrss_core::network::NetworkScope;
use tauri::State;

use rrss_app::queries;
use rrss_app::views::*;

use crate::state::AppState;

#[tauri::command]
pub fn provider_presets() -> Vec<ProviderPreset> {
    rrss_app::provider_presets::provider_presets()
}

db_task!(vector_space_status() -> VectorSpaceStatusView => queries::vector_space_status);

db_task!(model_bindings() -> ModelBindingsView => queries::model_bindings);

#[tauri::command]
pub async fn probe_pool_model(
    state: State<'_, AppState>,
    provider_id: i64,
    model: String,
) -> Result<ModelPoolView, UiError> {
    rrss_app::capabilities::probe_pool_model(
        &state.db_path,
        &state
            .network
            .client(NetworkScope::Api)
            .await
            .map_err(UiError::from)?,
        provider_id,
        &model,
    )
    .await
    .map_err(UiError::from)
}

db_task!(save_model_bindings(args: SaveModelBindingsArgs) -> ModelBindingsView =>
    |conn| rrss_app::actions::save_model_bindings(conn, &args));

db_task!(ai_page(selected_id: i64) -> AiPage => |conn| queries::ai_page(conn, selected_id));

db_task!(dismiss_auto_summary_notice() -> () => rrss_app::actions::dismiss_auto_summary_notice);

db_task!(model_pool() -> ModelPoolView => rrss_app::model_pool::model_pool);

db_task!(save_model_pool(args: SaveModelPoolArgs) -> ModelPoolView =>
    |conn| rrss_app::model_pool::save_model_pool(conn, &args));

db_task!(update_pool_model(args: UpdatePoolModelArgs) -> ModelPoolView =>
    |conn| rrss_app::model_pool::update_pool_model(conn, &args));

#[tauri::command]
pub async fn fetch_model_catalog(
    state: State<'_, AppState>,
    provider_id: i64,
) -> Result<ModelPoolView, UiError> {
    rrss_app::model_pool::fetch_model_catalog(
        &state.db_path,
        &state
            .network
            .client(NetworkScope::Api)
            .await
            .map_err(UiError::from)?,
        provider_id,
    )
    .await
    .map_err(UiError::from_anyhow_pretty)
}

db_task!(delete_provider(id: i64) -> () => |conn| rrss_app::actions::delete_provider(conn, id));

db_task!(save_ai_settings(args: SaveAiSettingsArgs) -> AiPage =>
    |conn| rrss_app::actions::save_ai_settings(conn, &args));

/// 草稿连接取模型列表；`provider_id` 是已保存的连接时，没填密钥就用库里的。
#[tauri::command]
pub async fn fetch_models(
    state: State<'_, AppState>,
    provider_id: i64,
    base_url: String,
    api_key: Option<String>,
) -> Result<Vec<String>, UiError> {
    rrss_app::capabilities::list_models(
        &state.db_path,
        &state
            .network
            .client(NetworkScope::Api)
            .await
            .map_err(UiError::from)?,
        provider_id,
        &base_url,
        api_key.as_deref(),
    )
    .await
    .map_err(UiError::from_anyhow_pretty)
}

#[tauri::command]
pub async fn test_connection(
    state: State<'_, AppState>,
    args: ConnectionTestArgs,
) -> Result<ConnectionTestView, UiError> {
    rrss_app::capabilities::test_connection(
        &state.db_path,
        &state
            .network
            .client(NetworkScope::Api)
            .await
            .map_err(UiError::from)?,
        args,
    )
    .await
    .map_err(UiError::from)
}

db_task!(reset_failed_embeds() -> () =>
    |conn| rrss_app::actions::reset_failed_embeds(conn).map(|_| ()));

#[tauri::command]
pub async fn embed_now(state: State<'_, AppState>) -> Result<(), UiError> {
    tracing::info!("manual embed_now triggered");
    state.scheduler.embed_now().await.map_err(|e| {
        let e = UiError::from(e);
        tracing::error!(code = e.code(), detail = %e.log_detail(), "manual embed_now failed");
        e
    })?;
    tracing::info!("manual embed_now completed successfully");
    Ok(())
}

db_task!(cancel_vector_rebuild() -> VectorSpaceStatusView => rrss_app::actions::cancel_vector_rebuild);
db_task!(set_vector_dims(value: String) -> VectorSpaceStatusView => |conn| rrss_app::actions::set_vector_dims(conn, &value));
#[tauri::command]
pub async fn check_vector_space(
    state: State<'_, AppState>,
) -> Result<VectorSpaceStatusView, UiError> {
    rrss_app::jobs::vector_space::check_now(&state.db_path, &state.network)
        .await
        .map_err(UiError::from)
}
#[tauri::command]
pub async fn restore_vector_space(
    state: State<'_, AppState>,
) -> Result<VectorSpaceStatusView, UiError> {
    rrss_app::jobs::vector_space::restore(&state.db_path, &state.network)
        .await
        .map_err(UiError::from)
}

command_handler![
    ai_page,
    dismiss_auto_summary_notice,
    provider_presets,
    model_bindings,
    vector_space_status,
    cancel_vector_rebuild,
    set_vector_dims,
    check_vector_space,
    restore_vector_space,
    save_model_bindings,
    model_pool,
    save_model_pool,
    update_pool_model,
    fetch_model_catalog,
    delete_provider,
    save_ai_settings,
    fetch_models,
    test_connection,
    probe_pool_model,
    reset_failed_embeds,
    embed_now,
];
