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
    assert_eq!(
        UiError::from(busy(rusqlite::ffi::SQLITE_BUSY)).code(),
        "database_busy"
    );
    assert_eq!(
        UiError::from(busy(rusqlite::ffi::SQLITE_LOCKED)).code(),
        "database_busy"
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
    let error = UiError::from(Error::Http {
        status: 407,
        message: rrss_core::ingest::feed_http::PROXY_AUTH_MESSAGE.into(),
    });
    let wire = serde_json::to_value(error).unwrap();
    assert_eq!(wire["code"], "network_proxy_auth");
    assert_eq!(
        wire["params"],
        json!({ "status": 407, "message": "proxy_auth" })
    );
}

#[test]
fn specific_http_messages_keep_the_original_display_and_safe_diagnostic() {
    for status in [0, 200, 407, 503] {
        let error = Error::Http {
            status,
            message: "image too large: response body canary".into(),
        };
        let original = error.to_string();
        let diagnostic = logsafe::error(&error);
        let ui = UiError::from(error);
        assert_eq!(ui.code(), "legacy_text");
        assert_eq!(ui.detail, original);
        assert_eq!(ui.log_detail(), diagnostic);
        assert!(!ui.log_detail().contains("canary"));
        let wire = serde_json::to_value(&ui).unwrap();
        assert!(wire.get("diagnostic").is_none());
        let restored: UiError = serde_json::from_value(wire).unwrap();
        assert_eq!(restored.log_detail(), "legacy command error");
    }
}

#[test]
fn sentence_errors_remain_legacy_and_proxy_context_is_typed() {
    for error in [
        Error::InvalidProvider("fixture".into()),
        Error::InvalidSetting("fixture".into()),
        Error::InvalidRule("fixture".into()),
        Error::InvalidNetworkProxy("fixture".into()),
        Error::InvalidPiOverride("fixture".into()),
        Error::InvalidModelChoice("fixture".into()),
        Error::InvalidTreeOrder("fixture".into()),
        Error::Backup("fixture".into()),
        Error::VectorSpaceRebuildRequired {
            model: "fixture".into(),
        },
        Error::VectorSpaceRebuildInProgress,
        Error::TaskModelUnset("fixture".into()),
        Error::SchemaTooOld { found: 1, min: 2 },
    ] {
        let original = error.to_string();
        let ui = UiError::from(error);
        assert_eq!(ui.code(), "legacy_text");
        assert_eq!(ui.detail, original);
    }
    let wire =
        serde_json::to_value(UiError::from(Error::InvalidNetworkProxy("fixture".into()))).unwrap();
    assert_eq!(wire["params"]["source"], "network_proxy");
}

#[test]
fn string_and_anyhow_errors_are_scrubbed_at_the_boundary() {
    let original =
        "failed https://user:canary-pass@service.example/path?token=canary-token sk-canary-key";
    let legacy = UiError::from(original.to_string());
    assert_eq!(legacy.code(), "legacy_text");
    assert!(!legacy.detail.contains("canary"));
    assert_eq!(legacy.log_detail(), "legacy command error");
    let error = UiError::from(anyhow::anyhow!(original));
    assert_eq!(error.code(), "legacy_text");
    assert_eq!(error.detail, legacy.detail);
    assert!(!error.log_detail().contains("canary"));
}

#[test]
fn anyhow_uses_core_codes_only_when_unwrapping_keeps_the_display() {
    let coded = UiError::from(anyhow::Error::new(Error::FeedNotFound));
    assert_eq!(coded.code(), "feed_not_found");

    let error = anyhow::Error::new(Error::TaskModelUnset("fixture task".into()));
    let original = error.to_string();
    let diagnostic = crate::log::error(error.as_ref());
    let legacy = UiError::from(error);
    assert_eq!(legacy.code(), "legacy_text");
    assert_eq!(legacy.detail, original);
    assert_eq!(legacy.log_detail(), diagnostic);
    assert!(!legacy.log_detail().contains("fixture task"));

    let error = anyhow::Error::new(Error::FeedNotFound).context("select another feed");
    let original = error.to_string();
    let diagnostic = crate::log::error(error.as_ref());
    let context = UiError::from(error);
    assert_eq!(context.code(), "legacy_text");
    assert_eq!(context.detail, original);
    assert_eq!(context.log_detail(), diagnostic);
}

#[test]
fn pretty_anyhow_calls_keep_the_full_chain_and_its_safe_log_projection() {
    let error = anyhow::Error::new(Error::Http {
        status: 413,
        message: "response body canary".into(),
    })
    .context("read model catalog");
    let original = format!("{error:#}");
    let diagnostic = crate::log::error(error.as_ref());
    let ui = UiError::from_anyhow_pretty(error);
    assert_eq!(ui.code(), "legacy_text");
    assert_eq!(ui.detail, original);
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
}
