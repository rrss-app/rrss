//! ammonia 白名单：M1 标签，不含 iframe（先抽 Embed）。

use ammonia::Builder;
use std::collections::HashSet;

pub fn clean_html(html: &str, base: Option<&url::Url>) -> String {
    let mut b = Builder::default();
    b.tags(HashSet::from([
        "a",
        "b",
        "blockquote",
        "br",
        "code",
        "del",
        "div",
        "em",
        "figcaption",
        "figure",
        "h1",
        "h2",
        "h3",
        "h4",
        "h5",
        "h6",
        "hr",
        "i",
        "img",
        "li",
        "ol",
        "p",
        "pre",
        "s",
        "span",
        "strike",
        "strong",
        "table",
        "thead",
        "tbody",
        "tfoot",
        "tr",
        "th",
        "td",
        "u",
        "ul",
    ]));
    b.rm_tags(&["iframe", "video", "audio", "script", "style"]);
    b.url_schemes(HashSet::from(["http", "https", "mailto"]));
    b.link_rel(Some("noopener noreferrer"));
    if let Some(base) = base {
        b.url_relative(ammonia::UrlRelative::RewriteWithBase(base.clone()));
    } else {
        b.url_relative(ammonia::UrlRelative::Deny);
    }
    b.clean(html).to_string()
}
