//! Temporary web articles share the reader's storage and content network path.
use std::path::Path;

use rrss_core::{articles, db, feeds};
use rusqlite::{Connection, OptionalExtension, params};
use url::Url;

use crate::extract::{self, Layer};
use crate::fulltext::{FulltextError, FulltextErrorKind, fetch_page_with_url};
use crate::views::{LinkSafety, LinkWarning, OpenWebLinkView, WebLinkLayer};

const TEMP_LIFETIME_MS: i64 = 7 * 86_400_000;

impl WebLinkLayer {
    pub(crate) fn from_extraction(layer: Layer, user_rule: bool) -> Self {
        match layer {
            Layer::Rule if user_rule => Self::UserRule,
            Layer::Rule => Self::BuiltinRule,
            Layer::General => Self::General,
            Layer::Fallback => Self::Fallback,
            Layer::Container => Self::Container,
        }
    }
}

fn http_url(raw: &str) -> Result<Url, FulltextError> {
    let url = Url::parse(raw.trim()).map_err(FulltextError::network)?;
    if !rrss_core::url_norm::is_http_url(&url) {
        return Err(FulltextError::network("only HTTP(S) links are supported"));
    }
    Ok(url)
}

pub fn collect_article(conn: &Connection, id: i64) -> rrss_core::Result<()> {
    articles::collect_article(conn, id)
}

pub async fn open_web_link(
    network: &crate::network::NetworkClients,
    db_path: &Path,
    url: &str,
) -> Result<OpenWebLinkView, FulltextError> {
    let requested = http_url(url)?;
    let canonical = rrss_core::url_norm::canonical_article_url(requested.as_str(), None)
        .ok_or_else(|| FulltextError::network("invalid article URL"))?;
    let (html, actual_url) = fetch_page_with_url(network, requested.as_str()).await?;
    let path = db_path.to_path_buf();
    tokio::task::spawn_blocking(move || store_page(&path, &canonical, &html, &actual_url, None))
        .await
        .map_err(FulltextError::network)?
}

pub(crate) fn store_page(
    path: &Path,
    canonical: &str,
    html: &str,
    actual_url: &Url,
    snapshot: Option<(&Path, &[crate::web_snapshot::SnapshotImage])>,
) -> Result<OpenWebLinkView, FulltextError> {
    if snapshot.is_some() && crate::fulltext::detect_waf(html, 200) {
        return Err(FulltextError::new(
            FulltextErrorKind::Waf,
            "页面仍在等待验证",
        ));
    }
    let conn = db::open_at(path).map_err(FulltextError::network)?;
    let host = actual_url.host_str().unwrap_or_default();
    let rule = crate::site_rules::resolve_site_rule(&conn, host).map_err(FulltextError::network)?;
    let extracted = extract::extract_article(html, actual_url, rule.rule.as_ref())?;
    if snapshot.is_some() && !extracted.suspicious.is_empty() {
        return Err(FulltextError::new(
            FulltextErrorKind::NotArticle,
            format!("正文抽取结果可疑：{:?}", extracted.suspicious),
        ));
    }
    let layer = WebLinkLayer::from_extraction(extracted.layer, rule.user_rule);
    let final_canonical = rrss_core::url_norm::canonical_article_url(actual_url.as_str(), None)
        .unwrap_or_else(|| canonical.to_owned());
    let tx = db::write_transaction(&conn).map_err(FulltextError::network)?;
    let existing: Option<(i64, bool)> = tx
        .query_row(
            "SELECT id, feed_id IN (SELECT id FROM feeds WHERE kind='manual')
             FROM articles WHERE canonical_url IN (?1,?2)
                OR (guid IN (?1,?2) AND feed_id IN (SELECT id FROM feeds WHERE kind='manual'))
             ORDER BY expires_at IS NOT NULL, id LIMIT 1",
            params![canonical, final_canonical],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(FulltextError::network)?;
    let now = feeds::now_ms();
    let mut web_extracted_at = None;
    let (id, manual) = if let Some(existing) = existing {
        existing
    } else {
        let feed = feeds::manual_feed(&tx).map_err(FulltextError::network)?;
        // Keep the requested permalink stable if redirects change; the manual GUID
        // also remembers the resolved URL so opening that address reuses this row.
        tx.execute(
            "INSERT INTO articles(feed_id,canonical_url,guid,title,created_at,expires_at)
             VALUES(?1,?2,?3,'',?4,?5)",
            params![
                feed,
                canonical,
                final_canonical,
                now,
                now + TEMP_LIFETIME_MS
            ],
        )
        .map_err(FulltextError::network)?;
        (tx.last_insert_rowid(), true)
    };
    if manual {
        web_extracted_at = snapshot.map(|_| now);
        let current = articles::get_article(&tx, id).map_err(FulltextError::network)?;
        let hash = crate::document::source_hash(&extracted.title, &extracted.document);
        if hash
            != crate::document::source_hash(
                &current.title,
                &crate::document::load_document(&current),
            )
        {
            let plain = rrss_render::document_to_plain(&extracted.document);
            let json = rrss_render::document_to_json(&extracted.document)
                .map_err(FulltextError::network)?;
            articles::set_fulltext(&tx, id, &plain, &json).map_err(FulltextError::network)?;
            tx.execute(
            "UPDATE articles SET hash=?2,summary=NULL,summary_json=NULL,summary_hash=NULL,embed_tier=0 WHERE id=?1",
            params![id, hash],
        ).map_err(FulltextError::network)?;
        }
        let published = extracted.published_time.as_deref().and_then(|s| {
            chrono::DateTime::parse_from_rfc3339(s)
                .or_else(|_| chrono::DateTime::parse_from_rfc2822(s))
                .ok()
                .map(|d| d.timestamp_millis())
                .or_else(|| {
                    chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
                        .ok()
                        .and_then(|d| d.and_hms_opt(0, 0, 0))
                        .map(|d| d.and_utc().timestamp_millis())
                })
        });
        tx.execute(
        "UPDATE articles SET title=?2,author=COALESCE(?3,author),published_at=COALESCE(?4,published_at),
         expires_at=CASE WHEN expires_at IS NOT NULL THEN ?5 ELSE NULL END,
         web_extracted_at=?6,embed_tier=0 WHERE id=?1",
        params![id, extracted.title, extracted.byline, published, now + TEMP_LIFETIME_MS, web_extracted_at],
    ).map_err(FulltextError::network)?;
        if let Some((data_dir, images)) = snapshot {
            for image in images {
                rrss_core::media::store_image(&tx, data_dir, id, &image.url, &image.bytes)
                    .map_err(FulltextError::network)?;
            }
        }
    }
    let temporary = manual && articles::is_temporary(&tx, id).map_err(FulltextError::network)?;
    tx.commit().map_err(FulltextError::network)?;
    Ok(OpenWebLinkView {
        article_id: id,
        web_extracted_at,
        temporary,
        layer,
        suspicious: extracted
            .suspicious
            .iter()
            .map(|r| format!("{r:?}"))
            .collect(),
        text_chars: extracted.text_chars as i64,
        image_count: extracted.image_count as i64,
        actual_host: idna::domain_to_unicode(host).0,
        actual_url: actual_url.as_str().into(),
    })
}

pub fn link_safety(href: &str, link_text: &str) -> Result<LinkSafety, FulltextError> {
    use unicode_security::{MixedScript, RestrictionLevelDetection};
    let url = http_url(href)?;
    let punycode = url.host_str().unwrap_or_default();
    let actual_host = idna::domain_to_unicode(punycode).0;
    let mut warnings = Vec::new();
    // Evaluate each label: an ASCII TLD must not turn a Chinese-only IDN into mixed script.
    if actual_host.split('.').any(|label| {
        !label.is_ascii()
            && !label.is_single_script()
            && label.detect_restriction_level() > unicode_security::RestrictionLevel::SingleScript
    }) {
        warnings.push(LinkWarning::MixedScript {
            unicode: actual_host.clone(),
            punycode: punycode.into(),
        });
    }
    let text = link_text.trim();
    let text_url = if text.contains("://") {
        http_url(text).ok()
    } else if !text.chars().any(char::is_whitespace) && !text.contains('@') && text.contains('.') {
        http_url(&format!("https://{text}")).ok()
    } else {
        None
    };
    if let Some(text_url) = text_url {
        let text_host = text_url.host_str().unwrap_or_default();
        let bare = |s: &str| s.strip_prefix("www.").unwrap_or(s).to_string();
        if bare(text_host) != bare(punycode) {
            warnings.push(LinkWarning::HostMismatch {
                text_host: idna::domain_to_unicode(text_host).0,
            });
        }
    }
    if url.scheme() != "https" {
        warnings.push(LinkWarning::NonHttps);
    }
    Ok(LinkSafety {
        actual_host,
        warnings,
    })
}

#[cfg(test)]
mod tests;
