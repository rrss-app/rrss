//! Shared article-body selection and bounded reuse of HTML parsing.
//! Provenance lives in an optional JSON field; the public block IR is unchanged.
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, OnceLock};

use rrss_render::{Document, HTML_RENDER_REVISION, IR_VERSION, RenderOpts};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const CACHE_BYTES: usize = 8 * 1024 * 1024;
const CACHE_ENTRIES: usize = 32;

#[derive(Serialize, Deserialize)]
struct Source {
    kind: String,
    hash: String,
    base_url: Option<String>,
    renderer: u32,
}
#[derive(Serialize, Deserialize)]
struct Stored {
    #[serde(flatten)]
    document: Document,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    rrss_source: Option<Source>,
}
fn hash(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

pub fn feed_json(html: &str, base_url: Option<&str>) -> serde_json::Result<String> {
    serde_json::to_string(&Stored {
        document: html_document(html, base_url),
        rrss_source: Some(Source {
            kind: "feed".into(),
            hash: hash(html),
            base_url: base_url.map(str::to_owned),
            renderer: HTML_RENDER_REVISION,
        }),
    })
}

pub fn fulltext_json(document: &Document, text: &str) -> serde_json::Result<String> {
    serde_json::to_string(&Stored {
        document: document.clone(),
        rrss_source: Some(Source {
            kind: "fulltext".into(),
            hash: hash(text),
            base_url: None,
            renderer: HTML_RENDER_REVISION,
        }),
    })
}

fn stored(json: Option<&str>) -> Option<Stored> {
    json.and_then(|json| serde_json::from_str::<Stored>(json).ok())
        .filter(|stored| stored.document.v == IR_VERSION)
}

/// Reader: fetched full text wins; feed IR is reused only with matching provenance.
pub fn article_document(article: &crate::ArticleRow) -> Document {
    crate::db::latency::measure("document_load", || article_document_inner(article))
}

fn article_document_inner(article: &crate::ArticleRow) -> Document {
    let stored = stored(article.blocks_json.as_deref());
    if let Some(text) = article
        .fulltext_md
        .as_deref()
        .filter(|s| !s.trim().is_empty())
    {
        if let Some(stored) = stored
            && !stored.document.blocks.is_empty()
            && stored
                .rrss_source
                .as_ref()
                .is_none_or(|s| s.kind == "fulltext" && s.hash == hash(text))
        {
            return stored.document;
        }
        return rrss_render::markdown_to_document(text);
    }
    let html = article.content_html.as_deref().unwrap_or("");
    if !html.trim().is_empty() {
        if let Some(stored) = stored
            && stored.rrss_source.as_ref().is_some_and(|s| {
                s.kind == "feed"
                    && s.renderer == HTML_RENDER_REVISION
                    && s.hash == hash(html)
                    && s.base_url.as_deref() == article.canonical_url.as_deref()
            })
        {
            return stored.document;
        }
        return html_document(html, article.canonical_url.as_deref());
    }
    stored
        .map(|s| s.document)
        .unwrap_or_else(|| Document::new(vec![]))
}

/// Generation/chunk consumers receive the stored full-text IR, not its lossy
/// plain-text projection. Untagged legacy IR keeps its existing priority.
pub fn generation_document(json: Option<&str>, html: Option<&str>) -> Option<Document> {
    let stored = stored(json);
    let html = html.unwrap_or("");
    let mut base = None;
    if let Some(stored) = stored {
        let reusable = stored.rrss_source.as_ref().is_none_or(|source| {
            source.kind == "fulltext"
                || (source.kind == "feed"
                    && source.renderer == HTML_RENDER_REVISION
                    && source.hash == hash(html))
        });
        if reusable && !stored.document.blocks.is_empty() {
            return Some(stored.document);
        }
        base = stored.rrss_source.and_then(|s| s.base_url);
    }
    (!html.trim().is_empty()).then(|| html_document(html, base.as_deref()))
}

/// Keep legacy summary signatures stable; an invalidated feed IR must include
/// the changed HTML rather than continuing to sign the obsolete stored blocks.
/// A renderer-only change keeps the signature: the summary still describes the
/// same source HTML, and stored blocks are not rewritten for existing articles.
pub fn signature(json: Option<&str>, html: Option<&str>) -> String {
    #[derive(Deserialize)]
    struct Metadata {
        rrss_source: Option<Source>,
    }
    let source = json
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| html.map(str::trim))
        .unwrap_or("");
    let mut digest = Sha256::new();
    digest.update(source.as_bytes());
    if json
        .and_then(|json| serde_json::from_str::<Metadata>(json).ok())
        .and_then(|s| s.rrss_source)
        .is_some_and(|s| s.kind == "feed" && s.hash != hash(html.unwrap_or("")))
    {
        digest.update(HTML_RENDER_REVISION.to_le_bytes());
        digest.update(html.unwrap_or("").as_bytes());
    }
    digest.finalize()[..8]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

struct Cached {
    key: [u8; 32],
    document: Arc<str>,
    bytes: usize,
}
#[derive(Default)]
struct Cache {
    entries: VecDeque<Cached>,
    bytes: usize,
}

fn html_document(html: &str, base: Option<&str>) -> Document {
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    html_document_cached(html, base, CACHE.get_or_init(Mutex::default))
}

fn html_document_cached(html: &str, base: Option<&str>, cache: &Mutex<Cache>) -> Document {
    let mut key = Sha256::new();
    key.update(HTML_RENDER_REVISION.to_le_bytes());
    key.update((html.len() as u64).to_le_bytes());
    key.update(html.as_bytes());
    key.update(base.unwrap_or("").as_bytes());
    let key: [u8; 32] = key.finalize().into();
    let hit = {
        let mut cache = cache.lock().unwrap_or_else(|e| e.into_inner());
        cache
            .entries
            .iter()
            .position(|entry| entry.key == key)
            .map(|index| {
                let entry = cache.entries.remove(index).unwrap();
                let doc = entry.document.clone();
                cache.entries.push_back(entry);
                doc
            })
    };
    if let Some(encoded) = hit {
        if let Ok(doc) = serde_json::from_str::<Document>(&encoded)
            && doc.v == IR_VERSION
        {
            return doc;
        }
        let mut cache = cache.lock().unwrap_or_else(|e| e.into_inner());
        // Remove only the failed entry; another reader may have replaced it.
        if let Some(index) = cache
            .entries
            .iter()
            .position(|entry| entry.key == key && Arc::ptr_eq(&entry.document, &encoded))
        {
            let old = cache.entries.remove(index).unwrap();
            cache.bytes -= old.bytes;
        }
    }
    let doc = crate::db::latency::measure("html_parse", || {
        rrss_render::html_to_document(
            html,
            &RenderOpts {
                base_url: base.and_then(|url| url.parse().ok()),
            },
        )
    });
    // Retain encoded IR, whose owned bytes are bounded exactly. Cache hits
    // deserialize IR without repeating HTML parsing and sanitization.
    let encoded = serde_json::to_string(&doc).expect("typed document");
    let bytes = encoded.len();
    if bytes <= CACHE_BYTES / 4 {
        let entry = Cached {
            key,
            document: Arc::from(encoded),
            bytes,
        };
        let mut cache = cache.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(index) = cache.entries.iter().position(|entry| entry.key == key) {
            let old = cache.entries.remove(index).unwrap();
            cache.bytes -= old.bytes;
        }
        while cache.entries.len() >= CACHE_ENTRIES || cache.bytes + bytes > CACHE_BYTES {
            if let Some(old) = cache.entries.pop_front() {
                cache.bytes -= old.bytes;
            } else {
                break;
            }
        }
        cache.bytes += bytes;
        cache.entries.push_back(entry);
    }
    doc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_cached_document_is_reparsed_and_replaced() {
        let cache = Mutex::new(Cache::default());
        let html = "<p><a href='../ref'>reference</a></p>";
        let base = Some("https://body.example/a/story");
        let expected = html_document_cached(html, base, &cache);
        for invalid in ["broken JSON", r#"{"v":0,"blocks":[]}"#] {
            {
                let mut cache = cache.lock().unwrap();
                let entry = cache.entries.front_mut().unwrap();
                entry.document = Arc::from(invalid);
                entry.bytes = invalid.len();
                cache.bytes = invalid.len();
            }
            assert_eq!(html_document_cached(html, base, &cache), expected);
            let cache = cache.lock().unwrap();
            assert_eq!(cache.entries.len(), 1);
            let entry = cache.entries.front().unwrap();
            assert_eq!(cache.bytes, entry.document.len());
            assert_eq!(
                serde_json::from_str::<Document>(&entry.document).unwrap(),
                expected
            );
        }
    }

    fn article() -> (
        tempfile::TempDir,
        crate::db::PooledConnection,
        crate::ArticleRow,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open_at(&dir.path().join("body.db")).unwrap();
        conn.execute_batch("INSERT INTO feeds(id,url,created_at) VALUES(1,'https://body.example/feed',0);
            INSERT INTO articles(id,feed_id,title,canonical_url,created_at) VALUES(1,1,'fixture','https://body.example/a/story',0);").unwrap();
        let row = crate::articles::get_article(&conn, 1).unwrap();
        (dir, conn, row)
    }

    #[test]
    fn provenance_preserves_blocks_and_invalidates_content_base_and_renderer_changes() {
        let (_dir, _conn, mut article) = article();
        let html = "<h2>Section</h2><p><a href='../ref'>reference<sup>[1]</sup></a></p><pre><code class='language-rust'>let a=1;</code></pre><img src='cover.png'><table><tr><td>cell</td></tr></table>";
        article.content_html = Some(html.into());
        article.blocks_json = Some(feed_json(html, article.canonical_url.as_deref()).unwrap());
        let expected = rrss_render::html_to_document(
            html,
            &RenderOpts {
                base_url: article
                    .canonical_url
                    .as_deref()
                    .and_then(|u| u.parse().ok()),
            },
        );
        assert_eq!(article_document(&article), expected);
        assert_eq!(
            generation_document(article.blocks_json.as_deref(), Some(html)).unwrap(),
            expected
        );
        let mut stale: serde_json::Value =
            serde_json::from_str(article.blocks_json.as_deref().unwrap()).unwrap();
        stale["rrss_source"]["renderer"] = (HTML_RENDER_REVISION - 1).into();
        stale["blocks"] =
            serde_json::to_value(rrss_render::markdown_to_document("stale").blocks).unwrap();
        article.blocks_json = Some(stale.to_string());
        assert_eq!(article_document(&article), expected);
        assert_eq!(
            generation_document(article.blocks_json.as_deref(), Some(html)).unwrap(),
            expected
        );
        let previous_signature = signature(
            article.blocks_json.as_deref(),
            article.content_html.as_deref(),
        );
        article.content_html = Some("<p>updated</p>".into());
        assert_ne!(
            signature(
                article.blocks_json.as_deref(),
                article.content_html.as_deref()
            ),
            previous_signature
        );
        assert_eq!(
            rrss_render::document_to_plain(&article_document(&article)),
            "updated"
        );
        article.content_html = Some(html.into());
        article.canonical_url = Some("https://changed.example/b/story".into());
        assert_ne!(article_document(&article), expected);
        article.blocks_json = Some("broken".into());
        assert!(!article_document(&article).blocks.is_empty());
    }

    #[test]
    fn renderer_revision_alone_keeps_the_summary_signature() {
        let (_dir, _conn, mut article) = article();
        let html = "<p>Model<sup>[1]</sup> text</p>";
        article.content_html = Some(html.into());
        let mut older: serde_json::Value =
            serde_json::from_str(&feed_json(html, article.canonical_url.as_deref()).unwrap())
                .unwrap();
        older["rrss_source"]["renderer"] = (HTML_RENDER_REVISION - 1).into();
        let older = older.to_string();
        let plain = Sha256::digest(older.as_bytes());
        let expected: String = plain[..8].iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(signature(Some(&older), Some(html)), expected);
        assert_ne!(
            signature(Some(&older), Some("<p>Model text, revised</p>")),
            expected
        );
    }

    #[test]
    fn fulltext_is_authoritative_and_legacy_reading_keeps_its_source_rules() {
        let (_dir, conn, mut article) = article();
        let doc =
            rrss_render::markdown_to_document("# Full text\n\n[link](https://body.example/ref)");
        let json = rrss_render::document_to_json(&doc).unwrap();
        article.content_html = Some("<p>feed excerpt</p>".into());
        article.blocks_json = Some(json.clone());
        assert_eq!(
            rrss_render::document_to_plain(&article_document(&article)),
            "feed excerpt"
        );
        article.fulltext_md = Some("Full text\n\nlink".into());
        assert_eq!(article_document(&article), doc);
        crate::articles::set_fulltext(
            &conn,
            article.id,
            article.fulltext_md.as_deref().unwrap(),
            &fulltext_json(&doc, article.fulltext_md.as_deref().unwrap()).unwrap(),
        )
        .unwrap();
        article.blocks_json = crate::articles::get_article(&conn, article.id)
            .unwrap()
            .blocks_json;
        assert_eq!(article_document(&article), doc);
        assert_eq!(
            generation_document(
                article.blocks_json.as_deref(),
                article.content_html.as_deref()
            )
            .unwrap(),
            doc
        );
        article.fulltext_md = Some("Updated full text".into());
        assert_eq!(
            rrss_render::document_to_plain(&article_document(&article)),
            "Updated full text"
        );
        article.blocks_json = Some(r#"{"v":999,"blocks":[]}"#.into());
        assert_eq!(
            rrss_render::document_to_plain(&article_document(&article)),
            "Updated full text"
        );
    }

    #[test]
    #[ignore = "explicit offline long-article rendering comparison"]
    fn document_reuse_latency() {
        let (_dir, _conn, mut article) = article();
        let html = "<h2>Local report</h2><p>A synthetic paragraph with <b>formatting</b> and a <a href='../ref'>reference</a>.</p>".repeat(300);
        article.content_html = Some(html.clone());
        article.blocks_json = Some(feed_json(&html, article.canonical_url.as_deref()).unwrap());
        let mut before = Vec::new();
        let mut after = Vec::new();
        let mut legacy_cached = Vec::new();
        let mut legacy = article.clone();
        let mut stored: serde_json::Value =
            serde_json::from_str(legacy.blocks_json.as_deref().unwrap()).unwrap();
        stored.as_object_mut().unwrap().remove("rrss_source");
        legacy.blocks_json = Some(stored.to_string());
        for _ in 0..30 {
            let started = std::time::Instant::now();
            let expected = rrss_render::html_to_document(
                &html,
                &RenderOpts {
                    base_url: article
                        .canonical_url
                        .as_deref()
                        .and_then(|u| u.parse().ok()),
                },
            );
            before.push(started.elapsed().as_secs_f64() * 1000.0);
            let started = std::time::Instant::now();
            let actual = article_document(&article);
            after.push(started.elapsed().as_secs_f64() * 1000.0);
            assert_eq!(actual, expected);
            let started = std::time::Instant::now();
            let actual = article_document(&legacy);
            legacy_cached.push(started.elapsed().as_secs_f64() * 1000.0);
            assert_eq!(actual, expected);
        }
        eprintln!(
            "document reuse: samples=30, legacy_parse_max_ms={:.2}, stored_ir_max_ms={:.2}, old_article_warm_cache_max_ms={:.2}",
            before.into_iter().fold(0.0, f64::max),
            after.into_iter().fold(0.0, f64::max),
            legacy_cached.into_iter().fold(0.0, f64::max),
        );
    }
}
