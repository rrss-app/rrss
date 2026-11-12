//! feed-rs → 领域条目。日志 target：`rrss_core::parse`。

use chrono::{DateTime, Utc};
use feed_rs::model::{Content, Entry, Feed, FeedType, Link, Person};
use feed_rs::parser;
use sha2::{Digest, Sha256};

use crate::error::{Error, Result};
use crate::kind::FeedKind;
use crate::url_norm::canonical_article_url;

#[derive(Debug, Clone)]
pub struct ParsedFeed {
    pub title: Option<String>,
    pub site_url: Option<String>,
    pub wire_kind: FeedKind,
    pub entries: Vec<ParsedEntry>,
}

#[derive(Debug, Clone)]
pub struct ParsedEntry {
    pub guid: String,
    pub canonical_url: Option<String>,
    pub title: String,
    pub author: Option<String>,
    pub published_at: Option<i64>,
    pub content_html: Option<String>,
    pub hash: String,
    pub tags: Vec<String>,
}

pub fn parse_feed_bytes(bytes: &[u8], source_uri: Option<&str>) -> Result<ParsedFeed> {
    let mut builder = parser::Builder::new();
    if let Some(uri) = source_uri {
        builder = builder.base_uri(Some(uri));
    }
    let feed = builder
        .build()
        .parse(bytes)
        .map_err(|err| Error::Parse(err.to_string()))?;
    Ok(from_feed(feed, source_uri))
}

fn from_feed(feed: Feed, source_uri: Option<&str>) -> ParsedFeed {
    let site_url = homepage_link(&feed.links).or_else(|| source_uri.map(str::to_string));
    let title = feed.title.as_ref().map(|t| t.content.clone());
    let wire_kind = FeedKind::from_wire(&feed.feed_type);
    let entries = feed
        .entries
        .iter()
        .map(|entry| map_entry(entry, site_url.as_deref()))
        .collect();
    ParsedFeed {
        title,
        site_url,
        wire_kind,
        entries,
    }
}

fn map_entry(entry: &Entry, base: Option<&str>) -> ParsedEntry {
    let title = entry
        .title
        .as_ref()
        .map(|t| t.content.clone())
        .unwrap_or_default();
    let canonical_url = entry_url(entry, base);
    let author = entry.authors.iter().find_map(person_label);
    let published = entry.published.or(entry.updated).map(datetime_ms);
    let content_html = entry_html(entry);
    let hash = entry_hash(&title, canonical_url.as_deref(), content_html.as_deref());
    let tags = entry
        .categories
        .iter()
        .map(|c| {
            c.label
                .clone()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| c.term.clone())
        })
        .filter(|s| !s.trim().is_empty())
        .collect();
    ParsedEntry {
        guid: entry.id.clone(),
        canonical_url,
        title,
        author,
        published_at: published,
        content_html,
        hash,
        tags,
    }
}

pub fn entry_hash(title: &str, canonical_url: Option<&str>, html: Option<&str>) -> String {
    let mut hasher = Sha256::new();
    hasher.update(title.as_bytes());
    hasher.update(b"\n");
    hasher.update(canonical_url.unwrap_or_default().as_bytes());
    hasher.update(b"\n");
    hasher.update(html.unwrap_or_default().as_bytes());
    hasher
        .finalize()
        .iter()
        .fold(String::with_capacity(64), |mut s, b| {
            use std::fmt::Write;
            let _ = write!(s, "{b:02x}");
            s
        })
}

fn entry_url(entry: &Entry, base: Option<&str>) -> Option<String> {
    let link = entry
        .links
        .iter()
        .find(|l| l.rel.as_deref() == Some("alternate"))
        .or_else(|| entry.links.first())?;
    canonical_article_url(&link.href, base)
}

fn entry_html(entry: &Entry) -> Option<String> {
    if let Some(Content {
        body: Some(body), ..
    }) = &entry.content
        && !body.trim().is_empty()
    {
        return Some(body.clone());
    }
    entry.summary.as_ref().map(|t| t.content.clone())
}

fn homepage_link(links: &[Link]) -> Option<String> {
    links
        .iter()
        .find(|l| l.rel.as_deref() == Some("alternate") || l.rel.is_none())
        .or_else(|| links.first())
        .map(|l| l.href.clone())
}

fn person_label(person: &Person) -> Option<String> {
    let name = person.name.trim();
    let email = person.email.as_deref().unwrap_or("").trim();
    // RSS2 <author> is a contact: feed-rs stores the role in `name` ("author")
    // and the element text in `email`.
    if is_rss_contact_role(name) {
        return display_from_rss_contact(email);
    }
    if !name.is_empty() {
        return Some(name.to_string());
    }
    display_from_rss_contact(email)
}

fn is_rss_contact_role(name: &str) -> bool {
    matches!(name, "author" | "managingEditor" | "webMaster")
}

fn display_from_rss_contact(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    if let (Some(start), Some(end)) = (raw.find('('), raw.rfind(')'))
        && end > start + 1
    {
        let inner = raw[start + 1..end].trim();
        if !inner.is_empty() {
            return Some(inner.to_string());
        }
    }
    Some(raw.to_string())
}

fn datetime_ms(dt: DateTime<Utc>) -> i64 {
    dt.timestamp_millis()
}

#[allow(dead_code)]
pub fn wire_kind(feed_type: &FeedType) -> FeedKind {
    FeedKind::from_wire(feed_type)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RSS2: &str = include_str!("../../../testdata/feeds/rss2-sample.xml");
    const ATOM: &str = include_str!("../../../testdata/feeds/atom-sample.xml");
    const JSON: &str = include_str!("../../../testdata/feeds/jsonfeed-sample.json");

    #[test]
    fn parses_rss2_and_strips_tracking() {
        let feed =
            parse_feed_bytes(RSS2.as_bytes(), Some("https://news.example.com/rss.xml")).unwrap();
        assert_eq!(feed.wire_kind, FeedKind::Rss);
        assert_eq!(feed.title.as_deref(), Some("Example News"));
        assert_eq!(feed.entries.len(), 2);
        let first = &feed.entries[0];
        assert_eq!(first.guid, "urn:example:library-1");
        assert_eq!(
            first.canonical_url.as_deref(),
            Some("https://news.example.com/library")
        );
        assert!(first.content_html.as_ref().unwrap().contains("new library"));
        assert_eq!(first.author.as_deref(), Some("Ada Example"));
        assert_eq!(first.tags, vec!["Library".to_string(), "City".to_string()]);
    }

    #[test]
    fn parses_atom() {
        let feed =
            parse_feed_bytes(ATOM.as_bytes(), Some("https://lab.example.org/atom.xml")).unwrap();
        assert_eq!(feed.wire_kind, FeedKind::Atom);
        assert_eq!(feed.entries.len(), 2);
        assert_eq!(feed.entries[0].author.as_deref(), Some("Lin Researcher"));
        assert!(
            feed.entries[0]
                .content_html
                .as_ref()
                .unwrap()
                .contains("Cache hits")
        );
    }

    #[test]
    fn parses_json_feed_and_drops_fragment() {
        let feed =
            parse_feed_bytes(JSON.as_bytes(), Some("https://json.example.net/feed.json")).unwrap();
        assert_eq!(feed.wire_kind, FeedKind::Json);
        assert_eq!(feed.entries.len(), 1);
        assert_eq!(
            feed.entries[0].canonical_url.as_deref(),
            Some("https://json.example.net/2026/hello")
        );
        assert_eq!(
            feed.entries[0].tags,
            vec!["json".to_string(), "example".to_string()]
        );
    }
}
