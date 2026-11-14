use super::{UiError, UiErrorKind};
use rrss_core::{Error, error::WebFailure};

error_codes!(WebError {
    SnapshotUnimplemented => "web_snapshot_unimplemented",
    SnapshotTimeout => "web_snapshot_timeout",
    TitleReadFailed => "web_title_read_failed",
    Webview2SnapshotFailed => "web_webview2_snapshot_failed",
    Webview2SnapshotInvalid => "web_webview2_snapshot_invalid",
    WebkitSnapshotFailed => "web_webkit_snapshot_failed",
    WebkitSnapshotReadFailed => "web_webkit_snapshot_read_failed",
    SnapshotCancelled => "web_snapshot_cancelled",
    SnapshotTooLarge => "web_snapshot_too_large",
    SnapshotMimeLimit => "web_snapshot_mime_limit",
    SnapshotHeadersInvalid => "web_snapshot_headers_invalid",
    SnapshotContentTypeMissing => "web_snapshot_content_type_missing",
    SnapshotBoundaryMissing => "web_snapshot_boundary_missing",
    SnapshotBoundaryInvalid => "web_snapshot_boundary_invalid",
    SnapshotIncomplete => "web_snapshot_incomplete",
    SnapshotBase64Invalid => "web_snapshot_base64_invalid",
    SnapshotQuotedPrintableInvalid => "web_snapshot_quoted_printable_invalid",
    SnapshotEncodingUnsupported => "web_snapshot_encoding_unsupported",
    SnapshotHtmlMissing => "web_snapshot_html_missing",
    SnapshotUrlInvalid => "web_snapshot_url_invalid",
    SnapshotHttpOnly => "web_snapshot_http_only",
    ExtractionUnavailable => "web_extraction_unavailable",
    Closed => "web_closed",
    ExtractionCancelled => "web_extraction_cancelled",
    ExtractionUrlChanged => "web_extraction_url_changed",
    ExtractionPageChanged => "web_extraction_page_changed",
    SnapshotProcessingFailed => "web_snapshot_processing_failed",
    UrlReadFailed => "web_url_read_failed",

    FulltextProxyAuth => "fulltext_proxy_auth",
    FulltextTooLarge => "fulltext_too_large",
    OperationFailed => "web_operation_failed",
    BoundsInvalid => "web_bounds_invalid",
    DataClearFailed => "web_data_clear_failed",
    CommandForbidden => "web_command_forbidden",
    SessionUnavailable => "web_session_unavailable",
    ProxyRouteChanged => "web_proxy_route_changed",
    HttpOnly => "web_http_only",
    NativeFailed => "web_native_failed",
    ActionUnknown => "web_action_unknown",
    WebkitContextMissing => "web_webkit_context_missing",
    DataManagerMissing => "web_data_manager_missing",
    WebkitSettingsMissing => "web_webkit_settings_missing",
    ParentMissing => "web_parent_missing",
    ParentInvalid => "web_parent_invalid",
    MainViewMissing => "web_main_view_missing",
    Webview2Unavailable => "web_webview2_unavailable",
});

impl From<WebError> for UiError {
    fn from(error: WebError) -> Self {
        let detail = error.code();
        Self::new(UiErrorKind::Web(error), detail)
    }
}

pub(super) fn classify(error: &Error) -> Option<UiErrorKind> {
    let Error::Web(reason) = error else { return None };
    Some(UiErrorKind::Web(match reason {
        WebFailure::OperationFailed => WebError::OperationFailed,
        WebFailure::BoundsInvalid => WebError::BoundsInvalid,
        WebFailure::DataClearFailed => WebError::DataClearFailed,
        WebFailure::VerifyUnavailable => WebError::ExtractionUnavailable,
        WebFailure::Closed => WebError::Closed,
        WebFailure::NavigationCancelled => WebError::ExtractionCancelled,
        WebFailure::UrlChanged => WebError::ExtractionUrlChanged,
        WebFailure::PageChanged => WebError::ExtractionPageChanged,
        WebFailure::SnapshotProcessing => WebError::SnapshotProcessingFailed,
        WebFailure::UrlUnavailable => WebError::UrlReadFailed,
        WebFailure::NativeFailed => WebError::NativeFailed,
        WebFailure::ActionUnknown => WebError::ActionUnknown,
        WebFailure::SnapshotUnsupported => WebError::SnapshotUnimplemented,
        WebFailure::SnapshotTimeout => WebError::SnapshotTimeout,
        WebFailure::SnapshotCancelled => WebError::SnapshotCancelled,
        WebFailure::SnapshotTooLarge => WebError::SnapshotTooLarge,
        WebFailure::WebkitContextMissing => WebError::WebkitContextMissing,
        WebFailure::WebkitDataMissing => WebError::DataManagerMissing,
        WebFailure::WebkitSettingsMissing => WebError::WebkitSettingsMissing,
        WebFailure::ParentMissing => WebError::ParentMissing,
        WebFailure::ParentInvalid => WebError::ParentInvalid,
        WebFailure::MainMissing => WebError::MainViewMissing,
        WebFailure::WebkitSnapshotFailed => WebError::WebkitSnapshotFailed,
        WebFailure::WebkitSnapshotRead => WebError::WebkitSnapshotReadFailed,
        WebFailure::Webview2Unavailable => WebError::Webview2Unavailable,
        WebFailure::Webview2Title => WebError::TitleReadFailed,
        WebFailure::Webview2Snapshot => WebError::Webview2SnapshotFailed,
        WebFailure::Webview2SnapshotInvalid => WebError::Webview2SnapshotInvalid,
        WebFailure::MimeLimit => WebError::SnapshotMimeLimit,
        WebFailure::MimeHeadersInvalid => WebError::SnapshotHeadersInvalid,
        WebFailure::MimeTypeMissing => WebError::SnapshotContentTypeMissing,
        WebFailure::MimeBoundaryMissing => WebError::SnapshotBoundaryMissing,
        WebFailure::MimeBoundaryInvalid => WebError::SnapshotBoundaryInvalid,
        WebFailure::MimeTruncated => WebError::SnapshotIncomplete,
        WebFailure::Base64Invalid => WebError::SnapshotBase64Invalid,
        WebFailure::QuotedPrintableInvalid => WebError::SnapshotQuotedPrintableInvalid,
        WebFailure::EncodingUnsupported => WebError::SnapshotEncodingUnsupported,
        WebFailure::HtmlMissing => WebError::SnapshotHtmlMissing,
        WebFailure::SnapshotUrlInvalid => WebError::SnapshotUrlInvalid,
        WebFailure::SnapshotSchemeUnsupported => WebError::SnapshotHttpOnly,
        WebFailure::FulltextWaf => return Some(UiErrorKind::Reader(super::reader::ReaderError::FulltextWaf)),
        WebFailure::FulltextNetwork => return Some(UiErrorKind::Reader(super::reader::ReaderError::FulltextNetwork)),
        WebFailure::FulltextEmpty => return Some(UiErrorKind::Reader(super::reader::ReaderError::FulltextEmpty)),
        WebFailure::FulltextNotArticle => return Some(UiErrorKind::Reader(super::reader::ReaderError::FulltextNotArticle)),
        WebFailure::FulltextProxyAuth => WebError::FulltextProxyAuth,
        WebFailure::FulltextTooLarge => WebError::FulltextTooLarge,
        WebFailure::ContentBusy => return Some(UiErrorKind::Reader(super::reader::ReaderError::ContentBusy)),
        WebFailure::DirectTooLarge => return Some(UiErrorKind::Reader(super::reader::ReaderError::DirectTooLarge)),
        WebFailure::DirectNotSupported => return Some(UiErrorKind::Reader(super::reader::ReaderError::DirectNotDirect)),
        WebFailure::DirectHttp { status } => return Some(UiErrorKind::Reader(super::reader::ReaderError::DirectHttp { status: *status })),
        WebFailure::DirectStore => return Some(UiErrorKind::Reader(super::reader::ReaderError::DirectStore)),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn web_errors_keep_codes_without_legacy_text() {
        for error in [
            WebError::SnapshotUnimplemented,
            WebError::SnapshotTimeout,
            WebError::TitleReadFailed,
            WebError::Webview2SnapshotFailed,
            WebError::Webview2SnapshotInvalid,
            WebError::WebkitSnapshotFailed,
            WebError::WebkitSnapshotReadFailed,
            WebError::SnapshotCancelled,
            WebError::SnapshotTooLarge,
            WebError::SnapshotMimeLimit,
            WebError::SnapshotHeadersInvalid,
            WebError::SnapshotContentTypeMissing,
            WebError::SnapshotBoundaryMissing,
            WebError::SnapshotBoundaryInvalid,
            WebError::SnapshotIncomplete,
            WebError::SnapshotBase64Invalid,
            WebError::SnapshotQuotedPrintableInvalid,
            WebError::SnapshotEncodingUnsupported,
            WebError::SnapshotHtmlMissing,
            WebError::SnapshotUrlInvalid,
            WebError::SnapshotHttpOnly,
            WebError::ExtractionUnavailable,
            WebError::Closed,
            WebError::ExtractionCancelled,
            WebError::ExtractionUrlChanged,
            WebError::ExtractionPageChanged,
            WebError::SnapshotProcessingFailed,
            WebError::UrlReadFailed,
            WebError::OperationFailed,
            WebError::BoundsInvalid,
            WebError::DataClearFailed,
            WebError::CommandForbidden,
            WebError::SessionUnavailable,
            WebError::ProxyRouteChanged,
            WebError::HttpOnly,
            WebError::NativeFailed,
            WebError::ActionUnknown,
            WebError::WebkitContextMissing,
            WebError::DataManagerMissing,
            WebError::WebkitSettingsMissing,
            WebError::ParentMissing,
            WebError::ParentInvalid,
            WebError::MainViewMissing,
            WebError::Webview2Unavailable,
        ] {
            let code = error.code();
            let ui = UiError::from(error);
            assert_eq!(ui.code(), code);
            assert!(ui.detail.is_ascii());
            assert_eq!(serde_json::to_value(ui).unwrap()["code"], code);
        }
    }
}

// The archive parser retains its existing internal string contract. Convert only
// its fixed vocabulary here; never treat an arbitrary diagnostic as a parameter.
pub fn snapshot_error(message: String) -> UiError {
    match message.as_str() {
        crate::web_snapshot::WEB_SNAPSHOT_TOO_LARGE => WebError::SnapshotTooLarge.into(),
        crate::web_snapshot::WEB_SNAPSHOT_MIME_LIMIT => WebError::SnapshotMimeLimit.into(),
        crate::web_snapshot::WEB_SNAPSHOT_HEADERS_INVALID => {
            WebError::SnapshotHeadersInvalid.into()
        }
        crate::web_snapshot::WEB_SNAPSHOT_CONTENT_TYPE_MISSING => {
            WebError::SnapshotContentTypeMissing.into()
        }
        crate::web_snapshot::WEB_SNAPSHOT_BOUNDARY_MISSING => {
            WebError::SnapshotBoundaryMissing.into()
        }
        crate::web_snapshot::WEB_SNAPSHOT_BOUNDARY_INVALID => {
            WebError::SnapshotBoundaryInvalid.into()
        }
        crate::web_snapshot::WEB_SNAPSHOT_INCOMPLETE => WebError::SnapshotIncomplete.into(),
        crate::web_snapshot::WEB_SNAPSHOT_BASE64_INVALID => WebError::SnapshotBase64Invalid.into(),
        crate::web_snapshot::WEB_SNAPSHOT_QUOTED_PRINTABLE_INVALID => {
            WebError::SnapshotQuotedPrintableInvalid.into()
        }
        crate::web_snapshot::WEB_SNAPSHOT_ENCODING_UNSUPPORTED => {
            WebError::SnapshotEncodingUnsupported.into()
        }
        crate::web_snapshot::WEB_SNAPSHOT_HTML_MISSING => WebError::SnapshotHtmlMissing.into(),
        _ => UiError::from(message),
    }
}

#[cfg(test)]
#[test]
fn snapshot_messages_have_codes() {
    assert_eq!(
        snapshot_error(crate::web_snapshot::WEB_SNAPSHOT_TOO_LARGE.into()).code(),
        "web_snapshot_too_large"
    );
    assert_eq!(
        snapshot_error(crate::web_snapshot::WEB_SNAPSHOT_MIME_LIMIT.into()).code(),
        "web_snapshot_mime_limit"
    );
    assert_eq!(
        snapshot_error(crate::web_snapshot::WEB_SNAPSHOT_HEADERS_INVALID.into()).code(),
        "web_snapshot_headers_invalid"
    );
    assert_eq!(
        snapshot_error(crate::web_snapshot::WEB_SNAPSHOT_CONTENT_TYPE_MISSING.into()).code(),
        "web_snapshot_content_type_missing"
    );
    assert_eq!(
        snapshot_error(crate::web_snapshot::WEB_SNAPSHOT_BOUNDARY_MISSING.into()).code(),
        "web_snapshot_boundary_missing"
    );
    assert_eq!(
        snapshot_error(crate::web_snapshot::WEB_SNAPSHOT_BOUNDARY_INVALID.into()).code(),
        "web_snapshot_boundary_invalid"
    );
    assert_eq!(
        snapshot_error(crate::web_snapshot::WEB_SNAPSHOT_INCOMPLETE.into()).code(),
        "web_snapshot_incomplete"
    );
    assert_eq!(
        snapshot_error(crate::web_snapshot::WEB_SNAPSHOT_BASE64_INVALID.into()).code(),
        "web_snapshot_base64_invalid"
    );
    assert_eq!(
        snapshot_error(crate::web_snapshot::WEB_SNAPSHOT_QUOTED_PRINTABLE_INVALID.into()).code(),
        "web_snapshot_quoted_printable_invalid"
    );
    assert_eq!(
        snapshot_error(crate::web_snapshot::WEB_SNAPSHOT_ENCODING_UNSUPPORTED.into()).code(),
        "web_snapshot_encoding_unsupported"
    );
    assert_eq!(
        snapshot_error(crate::web_snapshot::WEB_SNAPSHOT_HTML_MISSING.into()).code(),
        "web_snapshot_html_missing"
    );
}
