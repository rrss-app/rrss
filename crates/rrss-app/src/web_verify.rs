//! Current-document state policy, shared by automatic and manual extraction.
use crate::{OpenWebLinkView, WebVerifyState, WebVerifyStateEvent};

fn verification_state(
    url: &str,
    title: &str,
    loaded: bool,
    last: Option<&Result<OpenWebLinkView, crate::UiError>>,
) -> WebVerifyState {
    if !loaded || crate::fulltext::detect_waf(&format!("{url}\n{title}"), 200) {
        WebVerifyState::WaitingVerification
    } else {
        match last {
            None => WebVerifyState::PreExtract,
            Some(Ok(page)) if page.suspicious.is_empty() => WebVerifyState::Extracted,
            _ => WebVerifyState::NotExtracted,
        }
    }
}

pub fn event(
    label: &str,
    url: &str,
    title: Option<String>,
    last: Option<&Result<OpenWebLinkView, crate::UiError>>,
) -> WebVerifyStateEvent {
    let page = last.and_then(|result| result.as_ref().ok());
    WebVerifyStateEvent {
        label: label.into(),
        state: verification_state(url, title.as_deref().unwrap_or_default(), true, last),
        url: url.into(),
        title,
        article_id: page.map(|p| p.article_id),
        text_chars: page.map(|p| p.text_chars),
        image_count: page.map(|p| p.image_count),
        error: last
            .and_then(|result| result.as_ref().err())
            .filter(|error| error.code() != "legacy_text")
            .cloned(),
        reason: last.and_then(|result| match result {
            Err(reason) if reason.code() == "legacy_text" => Some(reason.detail.clone()),
            Ok(page) if !page.suspicious.is_empty() => Some(page.suspicious.join(", ")),
            _ => None,
        }),
    }
}

#[cfg(test)]
mod tests;
