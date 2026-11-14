use super::UiErrorKind;
use rrss_core::Error;

error_codes!(ReaderError {
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
