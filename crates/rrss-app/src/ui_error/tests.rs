use super::*;
use rrss_core::network::ErrorKind;
use serde_json::json;

#[test]
fn core_errors_keep_codes_and_typed_parameters() {
    for (error, code) in [
        (Error::FeedNotFound, "feed_not_found"),
        (Error::FolderNotFound, "folder_not_found"),
        (Error::ArticleNotFound, "article_not_found"),
        (Error::DigestNotFound, "digest_not_found"),
        (Error::RuleNotFound, "rule_not_found"),
        (Error::ProviderNotFound, "provider_not_found"),
        (Error::MemoryNotFound, "memory_not_found"),
        (Error::InterestNotFound, "interest_not_found"),
        (Error::ThreadNotFound, "thread_not_found"),
        (Error::McpServerNotFound, "mcp_server_not_found"),
        (Error::DailyBudgetExhausted, "ai_budget_exhausted"),
    ] {
        assert_eq!(UiError::from(error).code(), code);
    }
    let error = UiError::from(Error::UnsupportedSchema {
        found: 60,
        supported: 42,
    });
    let wire = serde_json::to_value(&error).unwrap();
    assert_eq!(wire["code"], "schema_unsupported");
    assert_eq!(wire["params"], json!({ "found": 60, "supported": 42 }));
    let restored: UiError = serde_json::from_value(wire).unwrap();
    assert_eq!(restored.code(), error.code());
}

#[test]
fn sqlite_busy_and_failure_are_distinct() {
    let busy = |code| {
        Error::Sqlite(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(code),
            None,
        ))
    };
    for code in [
        rusqlite::ffi::SQLITE_BUSY,
        rusqlite::ffi::SQLITE_BUSY_SNAPSHOT,
        rusqlite::ffi::SQLITE_LOCKED,
        rusqlite::ffi::SQLITE_LOCKED_SHAREDCACHE,
    ] {
        assert_eq!(UiError::from(busy(code)).code(), "database_busy");
    }
    assert_eq!(
        UiError::from(busy(rusqlite::ffi::SQLITE_IOERR_LOCK)).code(),
        "database"
    );
    assert_eq!(
        UiError::from(Error::Sqlite(rusqlite::Error::InvalidQuery)).code(),
        "database"
    );
}

#[test]
fn transport_categories_survive_without_unsafe_diagnostics() {
    for (kind, code) in [
        (ErrorKind::Timeout, "network_timeout"),
        (ErrorKind::Dns, "network_dns"),
        (ErrorKind::Refused, "network_refused"),
        (ErrorKind::Reset, "network_reset"),
        (ErrorKind::TlsCert, "network_tls_cert"),
        (ErrorKind::Tls, "network_tls"),
        (ErrorKind::ProxyConnect, "network_proxy_connect"),
        (ErrorKind::ProxyAuth, "network_proxy_auth"),
        (ErrorKind::Redirect, "network_redirect"),
        (ErrorKind::Body, "network_body"),
        (ErrorKind::Decode, "network_decode"),
        (ErrorKind::Http, "network_http"),
        (ErrorKind::Parse, "network_parse"),
        (ErrorKind::Other, "network_other"),
    ] {
        let error = UiError::from(Error::Network {
            kind,
            status: 0,
            message: rrss_core::network::RequestErrorMessage::Other,
        });
        assert_eq!(error.code(), code);
        assert!(!error.detail.contains("canary"));
    }
    let error = UiError::from(Error::FeedRequest(
        rrss_core::ingest::feed_http::FeedRequestError::ProxyAuth,
    ));
    let wire = serde_json::to_value(error).unwrap();
    assert_eq!(wire["code"], "network_proxy_auth");
    assert_eq!(
        wire["params"],
        json!({ "status": 407, "message": "proxy_auth" })
    );
}

#[test]
fn http_messages_never_reach_the_ui() {
    for (status, code) in [
        (0, "internal"),
        (200, "network_http"),
        (503, "network_http"),
    ] {
        let error = Error::Http {
            status,
            message: "image too large: response body canary".into(),
        };
        let diagnostic = logsafe::error(&error);
        let ui = UiError::from(error);
        assert_eq!(ui.code(), code);
        assert!(!ui.detail.contains("canary"));
        assert_eq!(ui.log_detail(), diagnostic);
        let wire = serde_json::to_value(&ui).unwrap();
        assert!(wire.get("diagnostic").is_none());
        assert!(!wire.to_string().contains("canary"));
    }
}

#[test]
fn stored_legacy_text_is_the_only_legacy_source() {
    let legacy = UiError::stored_legacy("old sentence https://user:canary@feed.example/");
    assert_eq!(legacy.code(), "legacy_text");
    assert!(!legacy.detail.contains("canary"));
    assert_eq!(legacy.log_detail(), "legacy stored error");
}

#[test]
fn migrated_validation_errors_keep_domain_codes() {
    for (error, code) in [
        (
            Error::InvalidNetworkProxy(rrss_core::network::ProxyError::UrlInvalid),
            "proxy_url_invalid",
        ),
        (
            Error::InvalidTreeOrder(rrss_core::tree_order::TreeOrderError::InvalidItem),
            "tree_item_invalid",
        ),
        (
            Error::InvalidModelChoice(
                rrss_core::error::models::ModelChoiceProblem::SelectionIncomplete,
            ),
            "model_selection_incomplete",
        ),
    ] {
        assert_code_params(error, code, json!({}));
    }
}

#[test]
fn anyhow_errors_are_internal_and_scrubbed_at_the_boundary() {
    let original =
        "failed https://user:canary-pass@service.example/path?token=canary-token sk-canary-key";
    let error = UiError::from(anyhow::anyhow!(original));
    assert_eq!(error.code(), "internal");
    assert!(!error.detail.contains("canary"));
    assert!(!error.log_detail().contains("canary"));
}

#[test]
fn anyhow_contexts_keep_the_wrapped_core_code() {
    let coded = UiError::from(anyhow::Error::new(Error::FeedNotFound));
    assert_eq!(coded.code(), "feed_not_found");

    let error = anyhow::Error::new(Error::TaskModelUnset("fixture task".into()));
    let diagnostic = crate::log::error(error.as_ref());
    let coded = UiError::from(error);
    assert_eq!(coded.code(), "task_model_unset");
    assert_eq!(coded.log_detail(), diagnostic);
    assert!(!coded.log_detail().contains("fixture task"));

    let error = anyhow::Error::new(Error::FeedNotFound).context("select another feed");
    let diagnostic = crate::log::error(error.as_ref());
    let context = UiError::from(error);
    assert_eq!(context.code(), "feed_not_found");
    assert_eq!(context.log_detail(), diagnostic);
}

pub(super) fn assert_code_params(error: Error, code: &str, params: serde_json::Value) {
    let error = UiError::from(error);
    assert_eq!(error.code(), code);
    let wire = serde_json::to_value(&error).unwrap();
    assert_eq!(
        wire.get("params").cloned().unwrap_or_else(|| json!({})),
        params,
        "{code}"
    );
    assert_eq!(
        serde_json::from_value::<UiError>(wire).unwrap().code(),
        code
    );
}

#[test]
fn pretty_anyhow_calls_code_the_core_failure_and_keep_a_safe_log_projection() {
    let error = anyhow::Error::new(Error::Http {
        status: 413,
        message: "response body canary".into(),
    })
    .context("read model catalog");
    let diagnostic = crate::log::error(error.as_ref());
    let ui = UiError::from_anyhow_pretty(error);
    assert_eq!(ui.code(), "network_http");
    assert_eq!(ui.log_detail(), diagnostic);
    assert!(!ui.log_detail().contains("canary"));
}

#[test]
fn parameter_helpers_only_project_allowed_data() {
    let error = UiError::from(Error::DuplicateFeed(
        "https://user:canary@feed.example/private?q=canary".into(),
    ));
    assert_eq!(
        serde_json::to_value(error).unwrap()["params"],
        json!({ "host": "feed.example" })
    );
    for path in [
        "/private/fixture/report.db",
        r"C:\private\fixture\report.db",
    ] {
        let error = UiError::from(Error::CreateDir {
            path: path.into(),
            source: std::io::ErrorKind::PermissionDenied.into(),
        });
        assert_eq!(
            serde_json::to_value(error).unwrap()["params"],
            json!({ "file": "report.db" })
        );
    }
    let error = UiError::from(Error::PresetNotFound("fixture sk-canary-key".into()));
    assert!(
        !serde_json::to_value(error).unwrap()["params"]
            .to_string()
            .contains("canary")
    );
    let error = UiError::from(Error::PresetNotFound(
        "https://user:canary@preset.example/private?q=canary".into(),
    ));
    assert_eq!(
        serde_json::to_value(error).unwrap()["params"],
        json!({ "name": "preset.example" })
    );
    for value in [
        "https://user:canary@model.example/private?q=canary",
        "/private/fixture/model.example",
        r"C:\private\fixture\model.example",
    ] {
        let error = UiError::from(Error::InvalidModelChoice(
            rrss_core::error::models::ModelChoiceProblem::NotFound {
                model: value.into(),
            },
        ));
        assert_eq!(
            serde_json::to_value(error).unwrap()["params"],
            json!({"model":"model.example"})
        );
    }
    let error =
        rrss_core::rules::validate_pattern(rrss_core::rules::Dimension::TitleRe, "[body-canary")
            .unwrap_err();
    let error = UiError::from(error);
    assert!(!serde_json::to_string(&error).unwrap().contains("canary"));
}
