use super::*;
use crate::WebVerifyState::*;

#[test]
fn verification_transition_table_and_retry() {
    let page = OpenWebLinkView {
        article_id: 7,
        web_extracted_at: Some(1),
        temporary: true,
        layer: crate::WebLinkLayer::General,
        suspicious: vec![],
        text_chars: 1000,
        image_count: 2,
        actual_host: "example.test".into(),
        actual_url: "https://example.test".into(),
    };
    let good = Ok(page.clone());
    let bad = Err("没有正文".into());
    let mut suspicious = page;
    suspicious.suspicious.push("TooShort".into());
    assert!(
        serde_json::to_value(&suspicious)
            .unwrap()
            .get("suspicious")
            .is_none()
    );
    let suspicious = Ok(suspicious);
    for (url, title, loaded, last, expected) in [
        (
            "https://example.test",
            "Article",
            false,
            None,
            WaitingVerification,
        ),
        (
            "https://example.test",
            "JUST A MOMENT...",
            true,
            None,
            WaitingVerification,
        ),
        (
            "https://example.test/cf-browser-verification",
            "",
            true,
            None,
            WaitingVerification,
        ),
        (
            "https://example.test",
            "Attention Required!",
            true,
            Some(&good),
            WaitingVerification,
        ),
        (
            "https://example.test",
            "Cloudflare Ray ID",
            true,
            None,
            WaitingVerification,
        ),
        ("https://example.test", "Article", true, None, PreExtract),
        (
            "https://example.test",
            "Article",
            true,
            Some(&good),
            Extracted,
        ),
        (
            "https://example.test",
            "Article",
            true,
            Some(&bad),
            NotExtracted,
        ),
        (
            "https://example.test",
            "Article",
            true,
            Some(&suspicious),
            NotExtracted,
        ),
        // A new finished load resets the previous failed attempt, allowing retry.
        (
            "https://example.test/next",
            "Next article",
            true,
            None,
            PreExtract,
        ),
    ] {
        assert_eq!(
            verification_state(url, title, loaded, last),
            expected,
            "{url}: {title}"
        );
    }
    let success = event(
        "web-1",
        "https://example.test",
        Some("Article".into()),
        Some(&good),
    );
    assert_eq!(success.article_id, Some(7));
    assert_eq!(success.text_chars, Some(1000));
    assert_eq!(success.image_count, Some(2));
    assert_eq!(success.reason, None);
    let failure = event("web-1", "https://example.test", None, Some(&bad));
    assert_eq!(failure.reason.as_deref(), Some("没有正文"));
    assert_eq!(failure.article_id, None);
}
