use super::UiErrorKind;
use rrss_core::Error;

error_codes!(ReaderError {
    TranslateTimeout => "translate_timeout",
    TranslateNetwork => "translate_network",
    TranslateInvalidOutput => "translate_invalid_output",
    TranslateAuthentication => "translate_authentication",
    TranslateQuota => "translate_quota",
    TranslateRateLimited => "translate_rate_limited",
    TranslateConfiguration => "translate_configuration",
    TranslateInvalidated => "translate_invalidated",
    TranslateStorage => "translate_storage",
    TranslateRetryUnavailable => "translate_retry_unavailable",
    TranslateCapacity => "translate_capacity",
    TranslateUrlInvalid => "translate_url_invalid",
    TranslateSchemeUnsupported => "translate_scheme_unsupported",
    TranslateInputChanged => "translate_input_changed",
    TranslateResultIncomplete => "translate_result_incomplete",
    TranslateCacheEncode => "translate_cache_encode",
    TranslateDisabled => "translate_disabled",
    TranslatePruned => "translate_pruned",
    TranslateEmpty => "translate_empty",
    TranslateTargetSame => "translate_target_same",
    TranslateKeyMissing => "translate_key_missing",
    TranslateEndpointMissing => "translate_endpoint_missing",
    TranslateUnitsInvalid => "translate_units_invalid",
    TranslateUnitsMissing => "translate_units_missing",
    TranslateOutputEmpty => "translate_output_empty",
    TranslateDataMissing => "translate_data_missing",
    TranslateTextMissing => "translate_text_missing",
    TranslateTextInvalid => "translate_text_invalid",
    TranslateArrayMissing => "translate_array_missing",
    TranslateSelectionTooLong { max: usize } => "translate_selection_too_long",
    TranslateDeeplCount { actual: usize, expected: usize } => "translate_deepl_count",
    TranslateHttpCount { actual: usize, expected: usize } => "translate_http_count",
    TranslateRequest => "translate_request",
    TranslateRead => "translate_read",
    TranslateJson { reason: String } => "translate_json",
    TranslateDeepLxRequest => "translate_deeplx_request",
    TranslateDeepLxRead => "translate_deeplx_read",
    TranslateDeepLxJson { reason: String } => "translate_deeplx_json",
    TranslateDeepLRequest => "translate_deepl_request",
    TranslateDeepLRead => "translate_deepl_read",
    TranslateDeepLJson { reason: String } => "translate_deepl_json",
    TranslateHttpRejected { status: u16, reason: String } => "translate_http_rejected",
    TranslateDeepLxRejected { status: u16, reason: String } => "translate_deeplx_rejected",
    TranslateDeepLRejected { status: u16, reason: String } => "translate_deepl_rejected",
    TranslateDeepLxError { status: i64, reason: String } => "translate_deeplx_error",
    TranslateDeepLxFailed { status: i64 } => "translate_deeplx_failed",
    TranslateRemoteReason { reason: String } => "translate_remote_reason",
    TranslateServiceHttp { status: u16 } => "translate_service_http",
    FulltextVerifyWaiting => "reader_fulltext_verify_waiting",
    FulltextSuspicious { reasons: String } => "reader_fulltext_suspicious",
    FulltextWaf => "reader_fulltext_waf",
    FulltextNetwork => "reader_fulltext_network",
    FulltextEmpty => "reader_fulltext_empty",
    FulltextNotArticle => "reader_fulltext_not_article",
    DirectTooLarge => "reader_direct_too_large",
    DirectNotDirect => "reader_direct_not_direct",
    DirectStore => "reader_direct_store",
    ContentBusy => "reader_content_busy",
    DirectHttp { status: u16 } => "reader_direct_http",

    ArticleNotFound => "article_not_found",
    DigestNotFound => "digest_not_found",
    ImageHttp403 => "image_http_403",
    ImageHttp404 => "image_http_404",
    ImageHttp5xx => "image_http_5xx",
    ImageDownloadFailed => "image_download_failed",
    ImageTooLarge => "image_too_large",
    ImageNetworkFailed => "image_network_failed",
    ImageDecodeFailed => "image_decode_failed",
    ImageStoreFailed => "image_store_failed",
    ImageUnsupported => "image_unsupported",
});

pub(super) fn classify(error: &Error) -> Option<UiErrorKind> {
    Some(UiErrorKind::Reader(match error {
        Error::Translation(reason) => match reason {
            rrss_core::error::TranslationFailure::UrlInvalid => ReaderError::TranslateUrlInvalid,
            rrss_core::error::TranslationFailure::SchemeUnsupported => {
                ReaderError::TranslateSchemeUnsupported
            }
            rrss_core::error::TranslationFailure::InputChanged => {
                ReaderError::TranslateInputChanged
            }
            rrss_core::error::TranslationFailure::ResultIncomplete => {
                ReaderError::TranslateResultIncomplete
            }
            rrss_core::error::TranslationFailure::CacheEncode => ReaderError::TranslateCacheEncode,
        },
        Error::ArticleNotFound => ReaderError::ArticleNotFound,
        Error::DigestNotFound => ReaderError::DigestNotFound,
        Error::UnsupportedImage => ReaderError::ImageUnsupported,
        _ => return None,
    }))
}

impl From<ReaderError> for crate::UiError {
    fn from(error: ReaderError) -> Self {
        let detail = error.code();
        Self::new(UiErrorKind::Reader(error), detail)
    }
}

impl From<crate::fulltext::FulltextError> for crate::UiError {
    fn from(error: crate::fulltext::FulltextError) -> Self {
        error.error
    }
}

impl From<crate::embed_fetch::DirectFail> for crate::UiError {
    fn from(error: crate::embed_fetch::DirectFail) -> Self {
        use crate::embed_fetch::DirectFail::*;
        match error {
            TooLarge => ReaderError::DirectTooLarge.into(),
            NotDirect => ReaderError::DirectNotDirect.into(),
            Store => ReaderError::DirectStore.into(),
            Http(status) => ReaderError::DirectHttp { status }.into(),
            Network(message) => message,
        }
    }
}

#[cfg(test)]
#[test]
fn reader_failures_retain_codes() {
    use crate::{
        UiError,
        embed_fetch::DirectFail,
        fulltext::{FulltextError, FulltextErrorKind},
    };
    for (kind, code) in [
        (FulltextErrorKind::Waf, "reader_fulltext_waf"),
        (FulltextErrorKind::Network, "reader_fulltext_network"),
        (FulltextErrorKind::Empty, "reader_fulltext_empty"),
        (FulltextErrorKind::NotArticle, "reader_fulltext_not_article"),
    ] {
        assert_eq!(UiError::from(FulltextError::coded(kind, None)).code(), code);
    }
    for (error, code) in [
        (DirectFail::TooLarge, "reader_direct_too_large"),
        (DirectFail::NotDirect, "reader_direct_not_direct"),
        (DirectFail::Store, "reader_direct_store"),
        (DirectFail::Http(403), "reader_direct_http"),
    ] {
        assert_eq!(UiError::from(error).code(), code);
    }
    assert_eq!(
        UiError::from(ReaderError::ContentBusy).code(),
        "reader_content_busy"
    );
}

impl ReaderError {
    pub(crate) fn remote(status: i64, reason: Option<&str>) -> crate::UiError {
        match reason {
            Some(reason) => Self::TranslateDeepLxError {
                status,
                reason: super::params::remote_reason(reason),
            },
            None => Self::TranslateDeepLxFailed { status },
        }
        .into()
    }
    pub(crate) fn remote_reason(reason: &str) -> crate::UiError {
        Self::TranslateRemoteReason {
            reason: super::params::remote_reason(reason),
        }
        .into()
    }
    pub(crate) fn rejected(service: &str, status: u16, reason: &str) -> crate::UiError {
        let reason = super::params::remote_reason(reason);
        match service {
            "deepl" => Self::TranslateDeepLRejected { status, reason },
            "deeplx" => Self::TranslateDeepLxRejected { status, reason },
            _ => Self::TranslateHttpRejected { status, reason },
        }
        .caused_by(Self::TranslateServiceHttp { status }.into_ui())
    }
}

impl ReaderError {
    pub(crate) fn into_ui(self) -> crate::UiError {
        self.into()
    }
}

impl ReaderError {
    pub(crate) fn suspicious(reasons: &str) -> crate::UiError {
        Self::FulltextSuspicious {
            reasons: super::params::name(reasons),
        }
        .into()
    }
}

impl ReaderError {
    pub(crate) fn caused_by(self, cause: crate::UiError) -> crate::UiError {
        let mut ui = self.into_ui();
        ui.cause = Some(Box::new(cause));
        ui
    }
    pub(crate) fn translation_json(service: &str, reason: &str) -> crate::UiError {
        let reason = super::params::remote_reason(reason);
        match service {
            "deepl" => Self::TranslateDeepLJson { reason },
            "deeplx" => Self::TranslateDeepLxJson { reason },
            _ => Self::TranslateJson { reason },
        }
        .into()
    }
}
