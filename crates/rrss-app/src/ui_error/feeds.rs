use super::{UiErrorKind, params};
use rrss_core::Error;

error_codes!(FeedError {
    FeedNotFound => "feed_not_found",
    FeedAlreadySubscribed { host: String } => "feed_already_subscribed",
});

pub(super) fn classify(error: &Error) -> Option<UiErrorKind> {
    Some(UiErrorKind::Feeds(match error {
        Error::FeedNotFound => FeedError::FeedNotFound,
        Error::DuplicateFeed(url) => FeedError::FeedAlreadySubscribed {
            host: params::host(url),
        },
        _ => return None,
    }))
}
