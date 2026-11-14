use super::UiErrorKind;
use rrss_core::Error;

error_codes!(ReaderError {
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
    ImageUnsupported => "image_unsupported",
});

pub(super) fn classify(error: &Error) -> Option<UiErrorKind> {
    Some(UiErrorKind::Reader(match error {
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
        use crate::fulltext::FulltextErrorKind::*;
        match error.kind {
            Waf => ReaderError::FulltextWaf,
            Network => ReaderError::FulltextNetwork,
            Empty => ReaderError::FulltextEmpty,
            NotArticle => ReaderError::FulltextNotArticle,
        }
        .into()
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
            Network(message) if message.detail == crate::embed_fetch::CONTENT_BUSY_MESSAGE => {
                ReaderError::ContentBusy.into()
            }
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
        assert_eq!(
            UiError::from(FulltextError::new(kind, "fixture")).code(),
            code
        );
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
