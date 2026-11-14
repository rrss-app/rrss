//! Browser archives are untrusted bytes. Parsing and storage never fetch URLs.
use std::path::Path;

use base64::Engine;
use mailparse::{MailHeader, MailHeaderMap};
use url::Url;

pub const MAX_SNAPSHOT_BYTES: usize = 30 * 1024 * 1024;
pub(crate) const WEB_SNAPSHOT_TOO_LARGE: &str = "快照超过 30 MiB 上限";
pub(crate) const WEB_SNAPSHOT_MIME_LIMIT: &str = "快照 MIME 层数或部分数量超限";
pub(crate) const WEB_SNAPSHOT_HEADERS_INVALID: &str = "快照 MIME 头损坏";
pub(crate) const WEB_SNAPSHOT_CONTENT_TYPE_MISSING: &str = "快照缺少 Content-Type";
pub(crate) const WEB_SNAPSHOT_BOUNDARY_MISSING: &str = "快照缺少 MIME boundary";
pub(crate) const WEB_SNAPSHOT_BOUNDARY_INVALID: &str = "快照 MIME boundary 损坏";
pub(crate) const WEB_SNAPSHOT_INCOMPLETE: &str = "快照 MIME 未完整结束";
pub(crate) const WEB_SNAPSHOT_BASE64_INVALID: &str = "快照 base64 损坏";
pub(crate) const WEB_SNAPSHOT_QUOTED_PRINTABLE_INVALID: &str = "快照 quoted-printable 损坏";
pub(crate) const WEB_SNAPSHOT_ENCODING_UNSUPPORTED: &str = "快照使用了不支持的传输编码";
pub(crate) const WEB_SNAPSHOT_HTML_MISSING: &str = "快照中没有 HTML 正文";

const MAX_IMAGES: usize = 200;

#[derive(Debug)]
pub struct SnapshotImage {
    pub url: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug)]
pub struct Snapshot {
    pub html: String,
    pub images: Vec<SnapshotImage>,
    // Image parts omitted because of the per-image or image-count limit.
    pub skipped_images: usize,
}

struct Part<'a> {
    headers: Vec<MailHeader<'a>>,
    body: &'a [u8],
}

// Parse headers with mailparse, but bound our own MIME traversal: parse_mail's
// recursive tree has no depth limit and accepts truncated multipart bodies.
fn parts<'a>(raw: &'a [u8], depth: usize, out: &mut Vec<Part<'a>>) -> Result<(), String> {
    if depth > 8 || out.len() >= 1024 {
        return Err(WEB_SNAPSHOT_MIME_LIMIT.into());
    }
    let (headers, offset) =
        mailparse::parse_headers(raw).map_err(|_| WEB_SNAPSHOT_HEADERS_INVALID)?;
    let content_type = headers
        .get_first_value("Content-Type")
        .ok_or(WEB_SNAPSHOT_CONTENT_TYPE_MISSING)?;
    let content_type = mailparse::parse_content_type(&content_type);
    if !content_type.mimetype.starts_with("multipart/") {
        out.push(Part {
            headers,
            body: &raw[offset..],
        });
        return Ok(());
    }
    let boundary = content_type
        .params
        .get("boundary")
        .ok_or(WEB_SNAPSHOT_BOUNDARY_MISSING)?;
    if boundary.is_empty() || boundary.len() > 200 || boundary.contains(['\r', '\n']) {
        return Err(WEB_SNAPSHOT_BOUNDARY_INVALID.into());
    }
    let marker = format!("--{boundary}");
    let end = format!("{marker}--");
    let mut start = None;
    let mut cursor = offset;
    for line in raw[offset..].split_inclusive(|b| *b == b'\n') {
        let clean = line.trim_ascii_end();
        if clean == marker.as_bytes() || clean == end.as_bytes() {
            if let Some(begin) = start.take() {
                let body = &raw[begin..cursor];
                let body = body
                    .strip_suffix(b"\r\n")
                    .or_else(|| body.strip_suffix(b"\n"))
                    .unwrap_or(body);
                parts(body, depth + 1, out)?;
            }
            if clean == end.as_bytes() {
                return Ok(());
            }
            start = Some(cursor + line.len());
        }
        cursor += line.len();
    }
    Err(WEB_SNAPSHOT_INCOMPLETE.into())
}

fn decode(part: &Part<'_>) -> Result<Vec<u8>, String> {
    match part
        .headers
        .get_first_value("Content-Transfer-Encoding")
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "base64" => base64::engine::general_purpose::STANDARD
            .decode(
                part.body
                    .iter()
                    .copied()
                    .filter(|b| !b.is_ascii_whitespace())
                    .collect::<Vec<_>>(),
            )
            .map_err(|_| WEB_SNAPSHOT_BASE64_INVALID.into()),
        "quoted-printable" => {
            quoted_printable::decode(part.body, quoted_printable::ParseMode::Robust)
                .map_err(|_| WEB_SNAPSHOT_QUOTED_PRINTABLE_INVALID.into())
        }
        "" | "7bit" | "8bit" | "binary" => Ok(part.body.to_vec()),
        _ => Err(WEB_SNAPSHOT_ENCODING_UNSUPPORTED.into()),
    }
}

pub fn parse_mhtml(raw: &[u8], page: &Url) -> Result<Snapshot, String> {
    if raw.len() > MAX_SNAPSHOT_BYTES {
        return Err(WEB_SNAPSHOT_TOO_LARGE.into());
    }
    let mut all = Vec::new();
    parts(raw, 0, &mut all)?;
    let mut main = None;
    let mut images = Vec::new();
    let mut image_count = 0;
    let mut skipped_images = 0;
    for part in &all {
        let content_type = part
            .headers
            .get_first_value("Content-Type")
            .unwrap_or_default();
        let mime = mailparse::parse_content_type(&content_type).mimetype;
        let location = part
            .headers
            .get_first_value("Content-Location")
            .and_then(|s| page.join(&s).ok());
        if mime == "text/html" && (main.is_none() || location.as_ref() == Some(page)) {
            main = Some((part, content_type));
        } else if mime.starts_with("image/") {
            image_count += 1;
            if image_count > MAX_IMAGES {
                skipped_images += 1;
                continue;
            }
            let bytes = decode(part)?;
            if bytes.len() > rrss_core::media::MAX_IMAGE_BYTES {
                skipped_images += 1;
                continue;
            }
            // Missing locations and cid/data URLs have no reusable HTTP cache key.
            if let Some(url) = location.filter(rrss_core::url_norm::is_http_url) {
                images.push(SnapshotImage {
                    url: url.into(),
                    bytes,
                });
            }
        }
    }
    let (main, content_type) = main.ok_or(WEB_SNAPSHOT_HTML_MISSING)?;
    if skipped_images > 0 {
        tracing::debug!(skipped_images, "snapshot images skipped due to limits");
    }
    Ok(Snapshot {
        html: crate::fulltext::decode_page(&decode(main)?, &content_type),
        images,
        skipped_images,
    })
}

pub fn store_snapshot(
    db_path: &Path,
    data_dir: &Path,
    url: &str,
    raw: &[u8],
) -> Result<crate::OpenWebLinkView, crate::UiError> {
    let page = Url::parse(url)
        .map_err(|_| crate::UiError::from(crate::ui_error::web::WebError::SnapshotUrlInvalid))?;
    if !rrss_core::url_norm::is_http_url(&page) {
        return Err(crate::UiError::from(
            crate::ui_error::web::WebError::SnapshotHttpOnly,
        ));
    }
    let snapshot = parse_mhtml(raw, &page).map_err(crate::ui_error::web::snapshot_error)?;
    let canonical = rrss_core::url_norm::canonical_article_url(page.as_str(), None).ok_or(
        crate::UiError::from(crate::ui_error::web::WebError::SnapshotUrlInvalid),
    )?;
    crate::web::store_page(
        db_path,
        &canonical,
        &snapshot.html,
        &page,
        Some((data_dir, &snapshot.images)),
    )
    .map_err(|e| crate::UiError::from(e.message))
}

#[cfg(test)]
mod tests;
