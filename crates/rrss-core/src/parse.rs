//! feed-rs → 领域条目。日志 target：`rrss_core::parse`。

use chrono::{DateTime, Utc};
use feed_rs::model::{Entry, Feed, FeedType, Link, Person};
use feed_rs::parser;
use sha2::{Digest, Sha256};

use crate::error::{Error, Result};
use crate::kind::FeedKind;
use crate::url_norm::canonical_article_url;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum FeedParseError {
    #[error("invalid feed document")]
    InvalidDocument,
    #[error("unknown feed kind")]
    UnknownKind,
}

#[derive(Debug, Clone)]
pub struct ParsedFeed {
    pub title: Option<String>,
    pub updated_at: Option<i64>,
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
        .map_err(|_| Error::FeedParse(FeedParseError::InvalidDocument))?;
    Ok(from_feed(feed, source_uri))
}

fn from_feed(feed: Feed, source_uri: Option<&str>) -> ParsedFeed {
    let updated_at = feed
        .updated
        .into_iter()
        .chain(feed.published)
        .chain(
            feed.entries
                .iter()
                .filter_map(|entry| entry.updated.or(entry.published)),
        )
        .max()
        .map(datetime_ms);
    let site_url = homepage_link(&feed.links).or_else(|| source_uri.map(str::to_string));
    let title = feed.title.as_ref().map(|t| t.content.clone());
    let wire_kind = FeedKind::from_wire(&feed.feed_type);
    let entries = feed
        .entries
        .iter()
        .map(|entry| map_entry(entry, site_url.as_deref(), feed.feed_type == FeedType::JSON))
        .collect();
    ParsedFeed {
        title,
        updated_at,
        site_url,
        wire_kind,
        entries,
    }
}

fn map_entry(entry: &Entry, base: Option<&str>, json_feed: bool) -> ParsedEntry {
    let title = entry
        .title
        .as_ref()
        .map(|t| t.content.clone())
        .unwrap_or_default();
    let canonical_url = entry_url(entry, base);
    let author = entry.authors.iter().find_map(person_label);
    let published = entry.published.or(entry.updated).map(datetime_ms);
    let content_html = entry_html(entry, canonical_url.as_deref().or(base), json_feed);
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

fn entry_html(entry: &Entry, base: Option<&str>, json_feed: bool) -> Option<String> {
    use rrss_render::{Block, EmbedKind, RenderOpts};
    let mut html = entry
        .content
        .as_ref()
        .and_then(|content| content.body.clone())
        .filter(|body| !body.trim().is_empty())
        .or_else(|| {
            entry
                .summary
                .as_ref()
                .map(|summary| summary.content.clone())
        });
    let base_url = base.and_then(|base| url::Url::parse(base).ok());
    let media = entry
        .media
        .iter()
        .flat_map(|group| &group.content)
        .filter_map(|media| {
            Some((
                media.url.as_ref()?.as_str(),
                media.content_type.as_ref().map(|mime| mime.as_str()),
            ))
        });
    let links = entry
        .links
        .iter()
        .filter(|link| {
            link.rel.as_deref() == Some("enclosure") || (json_feed && link.media_type.is_some())
        })
        .map(|link| (link.href.as_str(), link.media_type.as_deref()));
    let opts = RenderOpts { base_url };
    let mut attachments = media
        .chain(links)
        .filter_map(|(url, mime)| rrss_render::attachment_embed(url, opts.base_url.as_ref(), mime))
        .peekable();
    if attachments.peek().is_none() {
        return html;
    }
    let mut seen: std::collections::HashSet<_> =
        rrss_render::html_to_document(html.as_deref().unwrap_or_default(), &opts)
            .blocks
            .into_iter()
            .filter_map(|block| match block {
                Block::Embed { url, .. } => Some(url),
                _ => None,
            })
            .collect();
    for attachment in attachments {
        if !seen.insert(attachment.url.clone()) {
            continue;
        }
        let tag = match attachment.kind {
            EmbedKind::Audio => "audio",
            EmbedKind::Video => "video",
            _ => "iframe",
        };
        let src = attachment.url.replace('&', "&amp;").replace('"', "&quot;");
        html.get_or_insert_with(String::new)
            .push_str(&format!("\n<{tag} src=\"{src}\"></{tag}>"));
    }
    html
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
    fn podcast_and_video_attachments_reach_the_reader_once() {
        use rrss_render::{Block, EmbedKind, RenderOpts};
        let rss = r#"<rss version="2.0" xmlns:media="http://search.yahoo.com/mrss/" xmlns:content="http://purl.org/rss/1.0/modules/content/"><channel><title>Media</title><item><title>Clip</title><link>https://media.example/post/</link><content:encoded><![CDATA[<figure><video src="clip.mp4"></video></figure>]]></content:encoded><enclosure url="https://media.example/post/clip.mp4" type="video/mp4"/><media:content url="https://cdn.example/listen?a=1&amp;b=2" type="audio/mpeg"/><media:content url="https://cdn.example/cover.jpg" type="image/jpeg"/></item></channel></rss>"#;
        let atom = r#"<feed xmlns="http://www.w3.org/2005/Atom"><id>urn:example:feed</id><title>Media</title><entry><id>urn:example:item</id><title>Episode</title><link rel="enclosure" type="audio/mpeg" href="https://cdn.example/listen?a=1&amp;b=2"/></entry></feed>"#;
        let json = r#"{"version":"https://jsonfeed.org/version/1.1","title":"Show","items":[{"id":"1","content_text":"Episode","attachments":[{"url":"https://cdn.example/listen?a=1&b=2","mime_type":"audio/mpeg"}]}]}"#;
        for (wire, video) in [(rss, true), (atom, false), (json, false)] {
            let entry = parse_feed_bytes(wire.as_bytes(), Some("https://media.example/feed"))
                .unwrap()
                .entries
                .remove(0);
            let opts = RenderOpts {
                base_url: entry
                    .canonical_url
                    .as_deref()
                    .and_then(|url| url.parse().ok()),
            };
            let doc = rrss_render::html_to_document(entry.content_html.as_deref().unwrap(), &opts);
            let blocks: Vec<_> = doc
                .blocks
                .iter()
                .filter_map(|block| match block {
                    Block::Embed { kind, url, .. } => Some((*kind, url.as_str())),
                    _ => None,
                })
                .collect();
            let mut expected = Vec::new();
            if video {
                expected.push((EmbedKind::Video, "https://media.example/post/clip.mp4"));
            }
            expected.push((EmbedKind::Audio, "https://cdn.example/listen?a=1&b=2"));
            assert_eq!(blocks, expected);
        }
    }

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
