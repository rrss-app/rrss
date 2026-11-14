//! One privacy boundary for string parameters. Never pass URLs or full paths.
use rrss_core::logsafe;
use std::path::Path;

pub(crate) fn name(value: &str) -> String {
    logsafe::text(value, 200)
        .split_whitespace()
        .map(|part| {
            if part.contains("://") {
                logsafe::host_str(part)
            } else if part.starts_with(['/', '\\'])
                || part.starts_with("~/")
                || part.starts_with("~\\")
                || part.as_bytes().get(1) == Some(&b':')
            {
                part.rsplit(['/', '\\'])
                    .next()
                    .unwrap_or_default()
                    .to_string()
            } else {
                part.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}
pub(super) fn options(values: &[String]) -> String {
    values
        .iter()
        .map(|value| name(value))
        .collect::<Vec<_>>()
        .join(" / ")
}
pub(super) fn host(value: &str) -> String {
    logsafe::host_str(value)
}
pub(super) fn file_name(value: &Path) -> Option<String> {
    value.file_name().map(|name| {
        // Also handle a foreign-platform path in diagnostic input.
        let name = name.to_string_lossy();
        self::name(name.rsplit(['/', '\\']).next().unwrap_or_default())
    })
}

// Reason content may deliberately contain whitespace. Preserve it, while using
// the same privacy projection for URLs and credentials as other UI parameters.
pub(crate) fn reason_text(value: &str) -> String {
    rrss_core::rank::reasons::safe_text(value)
}

pub(crate) fn reason_legacy_text(value: &str) -> String {
    // Migration content is already displayed data, never diagnostic input.
    // Query stripping would change the old text; keep this out of error parameters.
    value.to_string()
}

// Services may echo a configured secret inside an otherwise legitimate reason.
pub(crate) fn scrub_error_params(error: &mut crate::UiError, secrets: &[&str]) {
    use crate::views::UiErrorKind;
    match &mut error.kind {
        UiErrorKind::Common(kind) => kind.scrub_parameters(secrets),
        UiErrorKind::Feeds(kind) => kind.scrub_parameters(secrets),
        UiErrorKind::Folders(kind) => kind.scrub_parameters(secrets),
        UiErrorKind::Rules(kind) => kind.scrub_parameters(secrets),
        UiErrorKind::Models(kind) => kind.scrub_parameters(secrets),
        UiErrorKind::Chat(kind) => kind.scrub_parameters(secrets),
        UiErrorKind::Reader(kind) => kind.scrub_parameters(secrets),
        UiErrorKind::Web(kind) => kind.scrub_parameters(secrets),
        UiErrorKind::Host(kind) => kind.scrub_parameters(secrets),
        UiErrorKind::Network(kind) => kind.scrub_parameters(secrets),
        UiErrorKind::Settings(kind) => kind.scrub_parameters(secrets),
        UiErrorKind::Backup(kind) => kind.scrub_parameters(secrets),
        UiErrorKind::Pi(kind) => kind.scrub_parameters(secrets),
        UiErrorKind::Mcp(kind) => kind.scrub_parameters(secrets),
        UiErrorKind::Jobs(kind) => kind.scrub_parameters(secrets),
    }
    if let Some(cause) = &mut error.cause {
        scrub_error_params(cause, secrets);
    }
}

// Remote error reasons are display data. Preserve safe whitespace while removing
// URLs, paths and secrets through the same parameter boundary.
pub(crate) fn remote_reason(value: &str) -> String {
    logsafe::scrub_secrets(value)
        .chars()
        .take(500)
        .collect::<String>()
        .split_inclusive(char::is_whitespace)
        .map(|part| {
            let token = part.trim_end_matches(char::is_whitespace);
            let projected = if token.contains("://") {
                name(token)
            } else if let Some((prefix, suffix)) = token.rsplit_once(['=', '(', '[', ':'])
                && (suffix.starts_with(['/', '\\']) || suffix.as_bytes().get(1) == Some(&b':'))
            {
                format!(
                    "{}{}{}",
                    prefix,
                    &token[prefix.len()..prefix.len() + 1],
                    name(suffix)
                )
            } else {
                name(token)
            };
            format!("{}{}", projected, &part[token.len()..])
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_error::{
        chat::ChatUiError,
        network::{NetworkError, NetworkMessage},
    };
    use crate::{UiError, views::UiErrorKind};

    #[test]
    fn credential_collisions_preserve_typed_params_and_scrub_free_text() {
        for message in [
            NetworkMessage::Http,
            NetworkMessage::Connect,
            NetworkMessage::Timeout,
        ] {
            let mut error = UiError::new(
                UiErrorKind::Network(NetworkError::NetworkHttp {
                    status: 503,
                    message,
                }),
                "network failure",
            );
            let original = serde_json::to_value(&error.kind).unwrap();
            error.cause = Some(Box::new(ChatUiError::model_switch_rejected(
                "http",
                Some("http connect timeout"),
            )));
            scrub_error_params(&mut error, &["http", "connect", "timeout"]);
            assert_eq!(serde_json::to_value(&error.kind).unwrap(), original);
            let cause = serde_json::to_value(error.cause.unwrap()).unwrap();
            for field in ["expected", "reason"] {
                let text = cause["params"][field].as_str().unwrap();
                assert!(!text.is_empty());
                for secret in ["http", "connect", "timeout"] {
                    assert!(!text.contains(secret));
                }
            }
        }
    }
}
