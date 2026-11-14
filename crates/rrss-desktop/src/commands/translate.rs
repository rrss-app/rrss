use rrss_core::network::NetworkScope;
use tauri::State;

use rrss_app::queries;
use rrss_app::translate::ipc;
use rrss_app::views::*;

use crate::state::AppState;

db_task!(translate_page() -> TranslatePage => queries::translate_page);

#[tauri::command]
pub async fn translate_selection(
    state: State<'_, AppState>,
    text: String,
    config_identity: String,
) -> Result<SelectionTranslation, TranslationCommandError> {
    rrss_app::translate::selection::translate_selection(
        &state.db_path,
        &state
            .network
            .client(NetworkScope::Api)
            .await
            .map_err(|e| TranslationCommandError {
                kind: TranslationFailure::Configuration,
                error: Some(UiError::from(e)),
            })?,
        text,
        config_identity,
    )
    .await
}

#[tauri::command]
pub async fn translate_article(
    state: State<'_, AppState>,
    id: i64,
    mode: TranslationStartMode,
    initial: Option<TranslationInitialViewport>,
) -> Result<TranslationReply, TranslationCommandError> {
    ipc::start(&state.translation, &state.db_path, id, mode, initial)
        .await
        .map_err(TranslationCommandError::from)
}

#[tauri::command]
pub async fn translate_snapshot(
    state: State<'_, AppState>,
    id: i64,
    generation: Option<u64>,
) -> Result<TranslationReply, TranslationCommandError> {
    ipc::recover(&state.translation, &state.db_path, id, generation)
        .await
        .map_err(TranslationCommandError::from)
}

#[tauri::command]
pub fn translate_attach(state: State<'_, AppState>, generation: u64, ticket: u64) {
    state.translation.attach(generation, ticket);
}

#[tauri::command]
pub fn translate_detach(state: State<'_, AppState>, generation: u64, ticket: u64) {
    state.translation.detach(generation, ticket);
}

#[tauri::command]
pub fn translate_hint(state: State<'_, AppState>, hint: TranslationHint) {
    state.translation.hint(hint);
}

#[tauri::command]
pub async fn translate_retry_save(
    state: State<'_, AppState>,
    id: i64,
    generation: u64,
) -> Result<TranslationReply, TranslationCommandError> {
    ipc::retry_save(&state.translation, &state.db_path, id, generation)
        .await
        .map_err(TranslationCommandError::from)
}

db_task!(save_translate_settings(args: rrss_app::actions::SaveTranslateArgs) -> () =>
    |conn| rrss_app::actions::save_translate_settings(conn, &args));

#[tauri::command]
pub async fn test_translate(state: State<'_, AppState>) -> Result<String, UiError> {
    rrss_app::translate::test_translate(
        &state.db_path,
        &state
            .network
            .client(NetworkScope::Api)
            .await
            .map_err(UiError::from)?,
    )
    .await
    .map_err(UiError::from)
}

command_handler![
    translate_page,
    translate_article,
    translate_snapshot,
    translate_attach,
    translate_detach,
    translate_hint,
    translate_retry_save,
    save_translate_settings,
    test_translate,
    translate_selection,
];
