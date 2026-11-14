use super::*;
use std::path::{Path, PathBuf};

fn frontend_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../apps/desktop/src/lib/uiCodes.json")
}

fn frontend_source() -> String {
    let mut errors = [
        common::CommonError::CODES,
        feeds::FeedError::CODES,
        folders::FolderError::CODES,
        rules::RuleError::CODES,
        models::ModelError::CODES,
        chat::ChatUiError::CODES,
        reader::ReaderError::CODES,
        web::WebError::CODES,
        host::HostError::CODES,
        network::NetworkError::CODES,
        settings::SettingError::CODES,
        backup::BackupError::CODES,
        pi::PiUiError::CODES,
        mcp::McpUiError::CODES,
        jobs::JobsUiError::CODES,
    ]
    .concat();
    errors.sort_unstable();
    assert!(
        !errors.windows(2).any(|pair| pair[0] == pair[1]),
        "duplicate UI error code"
    );
    let mut reasons = [crate::reasons::ReasonMessage::CODES].concat();
    reasons.sort_unstable();
    assert!(
        !reasons.windows(2).any(|pair| pair[0] == pair[1]),
        "duplicate UI reason code"
    );
    format!(
        "{}\n",
        serde_json::to_string_pretty(&serde_json::json!({
            "errors": errors, "reasons": reasons,
            "feed_health": feeds::FeedHealthNotice::CODES,
        }))
        .unwrap()
    )
}

#[test]
#[ignore = "explicit export: cargo test -p rrss-app export_ui_codes -- --ignored"]
fn export_ui_codes() {
    std::fs::write(frontend_path(), frontend_source()).unwrap();
}

#[test]
fn frontend_codes_are_current() {
    assert_eq!(
        std::fs::read_to_string(frontend_path())
            .unwrap()
            .replace("\r\n", "\n"),
        frontend_source(),
        "run cargo test -p rrss-app export_ui_codes -- --ignored"
    );
}
