//! URL 规范化：去 tracking query 与 fragment。

use url::Url;

use crate::error::{Error, Result};

/// Fetching, snapshots and external link actions require an HTTP(S) host.
pub fn is_http_url(url: &Url) -> bool {
    matches!(url.scheme(), "http" | "https") && url.host_str().is_some()
}

/// An isolated web window also needs its local bootstrap page before navigation.
pub fn web_navigation_allowed(url: &Url) -> bool {
    is_http_url(url) || url.as_str() == "about:blank"
}

const TRACKING: &[&str] = &[
    "utm_source",
    "utm_medium",
    "utm_campaign",
    "utm_term",
    "utm_content",
    "utm_id",
    "fbclid",
    "gclid",
    "gclsrc",
    "dclid",
    "mc_cid",
    "mc_eid",
    "_ga",
];

/// 订阅 URL：必须是 http(s)，小写 host，去掉 fragment 与 tracking 参数。
pub fn normalize_feed_url(raw: &str) -> Result<String> {
    let url = parse_http(raw)?;
    Ok(strip_tracking(url).to_string())
}

/// 文章 permalink。相对地址用 `base` 解析；失败返回 `None`。
pub fn canonical_article_url(raw: &str, base: Option<&str>) -> Option<String> {
    let parsed = if let Ok(url) = Url::parse(raw) {
        url
    } else if let Some(base) = base.and_then(|b| Url::parse(b).ok()) {
        base.join(raw).ok()?
    } else {
        return None;
    };
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return None;
    }
    Some(strip_tracking(parsed).to_string())
}

fn parse_http(raw: &str) -> Result<Url> {
    let url = Url::parse(raw.trim()).map_err(|err| Error::InvalidUrl(err.to_string()))?;
    if url.scheme() != "http" && url.scheme() != "https" {
        return Err(Error::InvalidUrl(format!(
            "unsupported scheme {}",
            url.scheme()
        )));
    }
    if url.host_str().is_none() {
        return Err(Error::InvalidUrl("missing host".into()));
    }
    Ok(url)
}

fn strip_tracking(mut url: Url) -> Url {
    url.set_fragment(None);
    let filtered: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(k, _)| !TRACKING.iter().any(|t| k.eq_ignore_ascii_case(t)))
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    if filtered.is_empty() {
        url.set_query(None);
    } else {
        url.query_pairs_mut()
            .clear()
            .extend_pairs(filtered.iter().map(|(k, v)| (k.as_str(), v.as_str())));
    }
    url
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_url_policies_separate_navigation_from_fetching() {
        for (raw, allowed) in [
            ("http://example.com", true),
            ("https://example.com/path", true),
            ("about:blank", true),
            ("about:config", false),
            ("about:blank?remote", false),
            ("about:blank#remote", false),
            ("rrss://localhost/media/1", false),
            ("file:///tmp/page", false),
            ("tauri://localhost", false),
            ("javascript:alert(1)", false),
            ("data:text/html,hello", false),
            ("mailto:test@example.com", false),
            ("ftp://example.com", false),
            ("custom:payload", false),
        ] {
            assert_eq!(
                web_navigation_allowed(&Url::parse(raw).unwrap()),
                allowed,
                "{raw}"
            );
            assert_eq!(
                is_http_url(&Url::parse(raw).unwrap()),
                allowed && raw != "about:blank",
                "{raw}"
            );
        }
    }

    #[test]
    fn strips_tracking_and_fragment() {
        let got = canonical_article_url(
            "https://News.Example.com/a?utm_source=rss&keep=1&fbclid=x#section",
            None,
        )
        .unwrap();
        assert_eq!(got, "https://news.example.com/a?keep=1");
    }

    #[test]
    fn feed_url_rejects_ftp() {
        assert!(normalize_feed_url("ftp://example.com/rss").is_err());
    }

    #[test]
    fn relative_article_uses_base() {
        let got = canonical_article_url(
            "/posts/a?utm_medium=x",
            Some("https://lab.example.org/atom.xml"),
        )
        .unwrap();
        assert_eq!(got, "https://lab.example.org/posts/a");
    }
}
