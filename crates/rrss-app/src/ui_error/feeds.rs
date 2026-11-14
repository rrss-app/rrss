use super::{UiErrorKind, params};
use rrss_core::{
    Error,
    ingest::{FetchDeferredReason, feed_http::FeedRequestError},
    opml::OpmlError,
    parse::FeedParseError,
    url_norm::InvalidUrlReason,
};

error_codes!(FeedError {
    UrlMalformed => "url_malformed",
    UrlSchemeUnsupported => "url_scheme_unsupported",
    UrlHostMissing => "url_host_missing",
    WebPageUrlInvalid => "web_page_url_invalid",
    MediaUrlInvalid => "media_url_invalid",
    FeedIconInvalid => "feed_icon_invalid",
    FeedBodyTooLarge { #[ts(type = "number")] max_bytes: u64 } => "feed_body_too_large",
    FeedRedirectLimit => "feed_redirect_limit",
    FeedRedirectMissing => "feed_redirect_missing",
    FeedRedirectInvalid => "feed_redirect_invalid",
    FeedRedirectScheme => "feed_redirect_scheme",
    FeedAddressInvalid => "feed_address_invalid",
    FeedTimeout => "feed_timeout",
    FeedRuntimeStopped => "feed_runtime_stopped",
    FeedCooldownSaving => "feed_cooldown_saving",
    FeedQueueBusy => "feed_queue_busy",
    FeedCooldownSaveFailed => "feed_cooldown_save_failed",
    FeedCooldownLoadFailed => "feed_cooldown_load_failed",
    FeedHostLimited { #[ts(type = "number | null")] until: Option<i64> } => "feed_host_limited",
    FeedCooldownRecovery => "feed_cooldown_recovery",
    FeedParseFailed => "feed_parse_failed",
    FeedKindUnknown => "feed_kind_unknown",
    OpmlInvalid => "opml_invalid",
    OpmlSerialize => "opml_serialize",
    FeedNetworkUnavailable => "feed_network_unavailable",
    MediaTooLarge { #[ts(type = "number")] bytes: u64, #[ts(type = "number")] max_bytes: u64 } => "media_too_large",
    FeedRefreshHttp { status: u16 } => "feed_refresh_http",
    FeedRefreshFailed => "feed_refresh_failed",
    FeedHeld => "feed_held",
    FeedNotFound => "feed_not_found",
    FeedAlreadySubscribed { host: String } => "feed_already_subscribed",
});

pub(super) fn classify(error: &Error) -> Option<UiErrorKind> {
    Some(UiErrorKind::Feeds(match error {
        Error::FeedNotFound => FeedError::FeedNotFound,
        Error::DuplicateFeed(url) => FeedError::FeedAlreadySubscribed {
            host: params::host(url),
        },
        Error::InvalidUrl(reason) => match reason {
            InvalidUrlReason::Malformed => FeedError::UrlMalformed,
            InvalidUrlReason::UnsupportedScheme => FeedError::UrlSchemeUnsupported,
            InvalidUrlReason::MissingHost => FeedError::UrlHostMissing,
            InvalidUrlReason::WebPage => FeedError::WebPageUrlInvalid,
            InvalidUrlReason::Media => FeedError::MediaUrlInvalid,
            InvalidUrlReason::FeedIcon => FeedError::FeedIconInvalid,
        },
        Error::FeedRequest(reason) => match reason {
            FeedRequestError::BodyTooLarge { max_bytes } => FeedError::FeedBodyTooLarge {
                max_bytes: *max_bytes,
            },
            FeedRequestError::RedirectLimit => FeedError::FeedRedirectLimit,
            FeedRequestError::RedirectMissing => FeedError::FeedRedirectMissing,
            FeedRequestError::RedirectInvalid => FeedError::FeedRedirectInvalid,
            FeedRequestError::RedirectScheme => FeedError::FeedRedirectScheme,
            FeedRequestError::InvalidAddress => FeedError::FeedAddressInvalid,
            FeedRequestError::Timeout => FeedError::FeedTimeout,
            FeedRequestError::RuntimeStopped => FeedError::FeedRuntimeStopped,
            FeedRequestError::ProxyAuth => return None,
        },
        Error::FetchDeferred { until, reason } => match reason {
            FetchDeferredReason::CooldownSaving => FeedError::FeedCooldownSaving,
            FetchDeferredReason::QueueBusy => FeedError::FeedQueueBusy,
            FetchDeferredReason::CooldownSaveFailed => FeedError::FeedCooldownSaveFailed,
            FetchDeferredReason::CooldownLoadFailed => FeedError::FeedCooldownLoadFailed,
            FetchDeferredReason::HostLimited => FeedError::FeedHostLimited { until: *until },
            FetchDeferredReason::Recovery => FeedError::FeedCooldownRecovery,
        },
        Error::FeedParse(FeedParseError::InvalidDocument) => FeedError::FeedParseFailed,
        Error::FeedParse(FeedParseError::UnknownKind) => FeedError::FeedKindUnknown,
        Error::Opml(OpmlError::InvalidDocument) => FeedError::OpmlInvalid,
        Error::Opml(OpmlError::Serialize) => FeedError::OpmlSerialize,
        Error::FetchPaused => FeedError::FeedNetworkUnavailable,
        Error::MediaTooLarge { bytes, max_bytes } => FeedError::MediaTooLarge {
            bytes: *bytes,
            max_bytes: *max_bytes,
        },
        _ => return None,
    }))
}

// Health sentences carry raw data and no diagnostic text.
error_codes!(FeedHealthNotice {
    Failing {
        #[ts(type = "number")] fail_count: i64,
        #[ts(type = "number")] failing_days: i64,
        #[ts(type = "number | null")] last_success_at: Option<i64>,
    } => "feed_health_failing",
    Stale {
        #[ts(type = "number")] quiet_days: i64,
        avg_interval_days: Option<f64>,
        failed: bool,
        #[ts(type = "number | null")] last_article_at: Option<i64>,
        #[ts(type = "number | null")] last_success_at: Option<i64>,
    } => "feed_health_stale",
    Noisy { #[ts(type = "number")] window_days: i64 } => "feed_health_noisy",
});

pub(crate) fn stored_error(raw: &str) -> crate::views::UiError {
    use crate::views::UiError;
    if raw == "held" {
        return UiError::new(UiErrorKind::Feeds(FeedError::FeedHeld), "feed fetch held");
    }
    if !raw.starts_with('{') {
        return UiError::from(raw.to_string());
    }
    let kind = serde_json::from_str(raw).or_else(|_| {
        let mut value: serde_json::Value = serde_json::from_str(raw)?;
        // Adjacent-tagged unit variants omit params; storage always includes {}.
        if value
            .get("params")
            .is_some_and(|p| p == &serde_json::json!({}))
        {
            value.as_object_mut().unwrap().remove("params");
        }
        serde_json::from_value(value)
    });
    UiError::new(
        kind.unwrap_or(UiErrorKind::Feeds(FeedError::FeedRefreshFailed)),
        "stored feed error",
    )
}

/// Same stored envelope for failures produced by the application write path.
pub(crate) fn error_record(error: crate::views::UiError) -> String {
    let kind = if error.code() == "legacy_text" {
        UiErrorKind::Feeds(FeedError::FeedRefreshFailed)
    } else {
        error.kind
    };
    let mut value = serde_json::to_value(kind).unwrap();
    value
        .as_object_mut()
        .unwrap()
        .entry("params")
        .or_insert_with(|| serde_json::json!({}));
    value.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::views::UiError;
    use serde_json::json;

    #[test]
    fn feed_validation_and_policy_reasons_have_codes_and_parameters() {
        let cases = [
            (
                Error::InvalidUrl(InvalidUrlReason::Malformed),
                "url_malformed",
                json!({}),
            ),
            (
                Error::InvalidUrl(InvalidUrlReason::UnsupportedScheme),
                "url_scheme_unsupported",
                json!({}),
            ),
            (
                Error::InvalidUrl(InvalidUrlReason::MissingHost),
                "url_host_missing",
                json!({}),
            ),
            (
                Error::InvalidUrl(InvalidUrlReason::WebPage),
                "web_page_url_invalid",
                json!({}),
            ),
            (
                Error::InvalidUrl(InvalidUrlReason::Media),
                "media_url_invalid",
                json!({}),
            ),
            (
                Error::InvalidUrl(InvalidUrlReason::FeedIcon),
                "feed_icon_invalid",
                json!({}),
            ),
            (
                Error::FeedRequest(FeedRequestError::BodyTooLarge { max_bytes: 64 }),
                "feed_body_too_large",
                json!({ "max_bytes": 64 }),
            ),
            (
                Error::FeedRequest(FeedRequestError::RedirectLimit),
                "feed_redirect_limit",
                json!({}),
            ),
            (
                Error::FeedRequest(FeedRequestError::RedirectMissing),
                "feed_redirect_missing",
                json!({}),
            ),
            (
                Error::FeedRequest(FeedRequestError::RedirectInvalid),
                "feed_redirect_invalid",
                json!({}),
            ),
            (
                Error::FeedRequest(FeedRequestError::RedirectScheme),
                "feed_redirect_scheme",
                json!({}),
            ),
            (
                Error::FeedRequest(FeedRequestError::InvalidAddress),
                "feed_address_invalid",
                json!({}),
            ),
            (
                Error::FeedRequest(FeedRequestError::Timeout),
                "feed_timeout",
                json!({}),
            ),
            (
                Error::FeedRequest(FeedRequestError::RuntimeStopped),
                "feed_runtime_stopped",
                json!({}),
            ),
            (
                Error::FeedParse(FeedParseError::InvalidDocument),
                "feed_parse_failed",
                json!({}),
            ),
            (
                Error::FeedParse(FeedParseError::UnknownKind),
                "feed_kind_unknown",
                json!({}),
            ),
            (
                Error::Opml(OpmlError::InvalidDocument),
                "opml_invalid",
                json!({}),
            ),
            (
                Error::Opml(OpmlError::Serialize),
                "opml_serialize",
                json!({}),
            ),
            (Error::FetchPaused, "feed_network_unavailable", json!({})),
            (
                Error::MediaTooLarge {
                    bytes: 65,
                    max_bytes: 64,
                },
                "media_too_large",
                json!({ "bytes": 65, "max_bytes": 64 }),
            ),
        ];
        for (error, code, params) in cases {
            let wire = serde_json::to_value(UiError::from(error)).unwrap();
            assert_eq!(wire["code"], code);
            assert_eq!(wire.get("params").cloned().unwrap_or(json!({})), params);
        }
        for (reason, code, params) in [
            (
                FetchDeferredReason::CooldownSaving,
                "feed_cooldown_saving",
                json!({}),
            ),
            (FetchDeferredReason::QueueBusy, "feed_queue_busy", json!({})),
            (
                FetchDeferredReason::CooldownSaveFailed,
                "feed_cooldown_save_failed",
                json!({}),
            ),
            (
                FetchDeferredReason::CooldownLoadFailed,
                "feed_cooldown_load_failed",
                json!({}),
            ),
            (
                FetchDeferredReason::HostLimited,
                "feed_host_limited",
                json!({ "until": 1000 }),
            ),
            (
                FetchDeferredReason::Recovery,
                "feed_cooldown_recovery",
                json!({}),
            ),
        ] {
            let error = Error::FetchDeferred {
                until: Some(1000),
                reason,
            };
            let record = rrss_core::feeds::refresh_error_json(&error);
            let wire = serde_json::to_value(UiError::from(error)).unwrap();
            assert_eq!(wire["code"], code);
            assert_eq!(wire.get("params").cloned().unwrap_or(json!({})), params);
            assert_eq!(stored_error(&record).code(), code);
        }
    }

    #[test]
    fn stored_errors_keep_legacy_text_and_decode_new_json_without_body_data() {
        let legacy = "fixture failure\nsecond line";
        let old = stored_error(legacy);
        assert_eq!(old.code(), "legacy_text");
        assert_eq!(old.detail, legacy);
        let record = rrss_core::feeds::refresh_error_json(&Error::Http {
            status: 503,
            message: "response body canary https://fixture.example/private".into(),
        });
        let new = stored_error(&record);
        assert_eq!(new.code(), "feed_refresh_http");
        assert_eq!(
            serde_json::to_value(new).unwrap()["params"],
            json!({ "status": 503 })
        );
        assert!(!record.contains("canary"));
        assert!(!record.contains("fixture.example"));
        assert_eq!(stored_error("held").code(), "feed_held");
        assert_eq!(
            stored_error(r#"{"code":"future_feed_error","params":{}}"#).code(),
            "feed_refresh_failed"
        );
        assert_eq!(stored_error("{broken").code(), "feed_refresh_failed");
        assert_eq!(
            stored_error(&error_record(UiError::from(
                "old command fixture".to_string()
            )))
            .code(),
            "feed_refresh_failed"
        );
    }
}
