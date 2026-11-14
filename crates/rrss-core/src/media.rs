//! 文章配图缓存。日志 target：`rrss_core::media`。

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{Error, Result};
use crate::feeds::now_ms;

pub const MAX_IMAGE_BYTES: usize = 8 * 1024 * 1024;
pub const MEDIA_HEAD_BYTES: usize = 1024;
pub const MEDIA_SECURITY_HEADERS: [(&str, &str); 2] = [
    ("X-Content-Type-Options", "nosniff"),
    ("Content-Security-Policy", "sandbox; default-src 'none'"),
];
const MEDIA_WINDOW_LABELS: &[&str] = &["main"];

pub fn media_window_allowed(label: &str) -> bool {
    MEDIA_WINDOW_LABELS.contains(&label)
}

/// Only inspect a bounded prefix; never search past another document's root element.
pub fn is_svg(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(MEDIA_HEAD_BYTES)];
    let text = String::from_utf8_lossy(head.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(head));
    let mut rest = text.trim_start();
    if rest.starts_with("<?xml") {
        let Some((_, tail)) = rest.split_once("?>") else {
            return false;
        };
        rest = tail.trim_start();
        loop {
            if let Some(comment) = rest.strip_prefix("<!--") {
                let Some((_, tail)) = comment.split_once("-->") else {
                    return false;
                };
                rest = tail.trim_start();
            } else if rest.starts_with("<!DOCTYPE svg") {
                // A DOCTYPE may contain quoted '>' or an internal subset.
                let mut quote = None;
                let mut depth = 0u32;
                let end = rest.char_indices().find_map(|(i, c)| {
                    if let Some(q) = quote {
                        if c == q {
                            quote = None;
                        }
                    } else {
                        match c {
                            '\'' | '"' => quote = Some(c),
                            '[' => depth += 1,
                            ']' => depth = depth.saturating_sub(1),
                            '>' if depth == 0 => return Some(i),
                            _ => {}
                        }
                    }
                    None
                });
                let Some(end) = end else { return false };
                rest = rest[end + 1..].trim_start();
            } else {
                break;
            }
        }
    }
    rest.strip_prefix("<svg")
        .is_some_and(|tail| tail.is_empty() || tail.starts_with([' ', '\t', '\r', '\n', '/', '>']))
}

pub fn served_mime(kind: &str, stored_mime: Option<&str>, head: &[u8]) -> String {
    if kind == "video" {
        stored_mime
            .and_then(canonical_av_mime)
            .or_else(|| sniff_av_mime(head).map(str::to_string))
            .unwrap_or_else(|| "application/octet-stream".into())
    } else {
        sniff_image_mime(head)
            .or_else(|| is_svg(head).then_some("image/svg+xml"))
            .unwrap_or("application/octet-stream")
            .to_string()
    }
}
/// LRU 配图缓存总容量上限。超出后按 `accessed_at` 淘汰最久未读的图片。不含直链音视频。
pub const MAX_CACHE_BYTES: usize = 256 * 1024 * 1024;
/// 单个直链音视频上限。超过则不下载。
pub const MAX_DIRECT_BYTES: usize = 64 * 1024 * 1024;
/// 直链音视频缓存总上限，与配图 LRU 分开计算。
pub const MAX_DIRECT_CACHE_BYTES: usize = 512 * 1024 * 1024;

pub fn direct_len_allowed(len: u64) -> bool {
    len <= MAX_DIRECT_BYTES as u64
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaRow {
    pub id: i64,
    pub article_id: i64,
    pub url: String,
    pub local_path: Option<String>,
    pub mime: Option<String>,
    pub kind: String,
    pub bytes: Option<i64>,
}

pub fn cache_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("cache").join("media")
}

pub fn cache_key(url: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(url.as_bytes());
    format!("{:x}", hasher.finalize())
}

pub fn cache_path(data_dir: &Path, url: &str) -> PathBuf {
    cache_dir(data_dir).join(cache_key(url))
}

/// 按文件头判断扩展名。缓存文件本身没有扩展名时，`image::open` / `Image::load_from_path` 会失败。
pub fn sniff_image_ext(bytes: &[u8]) -> Option<&'static str> {
    if bytes.len() >= 8 && bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        Some("png")
    } else if bytes.len() >= 3 && bytes[0] == 0xFF && bytes[1] == 0xD8 && bytes[2] == 0xFF {
        Some("jpg")
    } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        Some("webp")
    } else if bytes.len() >= 6 && (bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a")) {
        Some("gif")
    } else if bytes.starts_with(b"BM") {
        Some("bmp")
    } else if bytes.len() >= 4 && bytes.starts_with(&[0x00, 0x00, 0x01, 0x00]) {
        Some("ico")
    } else if is_avif(bytes) {
        Some("avif")
    } else {
        None
    }
}

/// ISOBMFF：偏移 4 起是 `ftyp`，主品牌或兼容品牌为 `avif` / `avis`。
/// 其它 `ftyp` 品牌（例如 HEIC）不在这里识别。
fn is_avif(bytes: &[u8]) -> bool {
    if bytes.len() < 12 || &bytes[4..8] != b"ftyp" {
        return false;
    }
    let declared = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
    let end = match declared {
        0 => bytes.len(),
        size if size < 12 => return false,
        size => size.min(bytes.len()),
    };
    if is_avif_brand(&bytes[8..12]) {
        return true;
    }
    let mut at = 16;
    while at + 4 <= end {
        if is_avif_brand(&bytes[at..at + 4]) {
            return true;
        }
        at += 4;
    }
    false
}

fn is_avif_brand(brand: &[u8]) -> bool {
    brand == b"avif" || brand == b"avis"
}

pub fn allowed_url(url: &str) -> bool {
    url.starts_with("https://") || url.starts_with("http://")
}

pub fn cached_path(conn: &Connection, article_id: i64, url: &str) -> Result<Option<PathBuf>> {
    let Some(row) = lookup(conn, article_id, url)? else {
        return Ok(None);
    };
    let Some(path) = row.local_path.filter(|p| !p.is_empty()) else {
        return Ok(None);
    };
    let path = PathBuf::from(path);
    if path.exists() {
        let _ = touch(conn, row.id);
        Ok(Some(path))
    } else {
        Ok(None)
    }
}

pub fn lookup(conn: &Connection, article_id: i64, url: &str) -> Result<Option<MediaRow>> {
    conn.query_row(
        "SELECT id, article_id, url, local_path, mime, kind, bytes
         FROM media WHERE article_id = ?1 AND url = ?2",
        params![article_id, url],
        media_from_row,
    )
    .optional()
    .map_err(Into::into)
}

pub fn store_image(
    conn: &Connection,
    data_dir: &Path,
    article_id: i64,
    url: &str,
    bytes: &[u8],
) -> Result<PathBuf> {
    if !allowed_url(url) {
        return Err(Error::InvalidUrl(crate::url_norm::InvalidUrlReason::Media));
    }
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err(Error::MediaTooLarge {
            bytes: bytes.len() as u64,
            max_bytes: MAX_IMAGE_BYTES as u64,
        });
    }
    let ext = sniff_image_ext(bytes)
        .or_else(|| is_svg(bytes).then_some("svg"))
        .ok_or(Error::UnsupportedImage)?;
    let mime = mime_for_ext(ext);
    let dir = cache_dir(data_dir);
    std::fs::create_dir_all(&dir)?;
    let mut path = cache_path(data_dir, url);
    path.set_extension(ext);
    std::fs::write(&path, bytes)?;
    let local = path.to_string_lossy().into_owned();
    let now = now_ms();
    if let Some(existing) = lookup(conn, article_id, url)? {
        conn.execute(
            "UPDATE media SET local_path = ?1, mime = ?2, bytes = ?3, accessed_at = ?4 WHERE id = ?5",
            params![local, mime, bytes.len() as i64, now, existing.id],
        )?;
    } else {
        conn.execute(
            "INSERT INTO media (article_id, url, local_path, mime, kind, bytes, created_at, accessed_at)
             VALUES (?1, ?2, ?3, ?4, 'image', ?5, ?6, ?6)",
            params![article_id, url, local, mime, bytes.len() as i64, now],
        )?;
    }
    let evicted = evict_lru(conn, data_dir)?;
    if evicted > 0 {
        tracing::debug!(evicted, "evicted lru media cache entries");
    }
    tracing::debug!(article_id, host = %crate::logsafe::host_str(url), bytes = bytes.len(), "stored image");
    Ok(path)
}

/// 登记已经写到磁盘的直链文件。不读入内存，也不计入配图 LRU。
pub fn store_direct(
    conn: &Connection,
    data_dir: &Path,
    article_id: i64,
    url: &str,
    path: &Path,
    mime: Option<&str>,
    bytes: u64,
) -> Result<PathBuf> {
    if !allowed_url(url) {
        return Err(Error::InvalidUrl(crate::url_norm::InvalidUrlReason::Media));
    }
    if !direct_len_allowed(bytes) {
        return Err(Error::MediaTooLarge {
            bytes,
            max_bytes: MAX_DIRECT_BYTES as u64,
        });
    }
    record_cached(conn, data_dir, article_id, url, path, mime, "video", bytes)
}

/// 封面元数据等小文件。不参与配图或直链淘汰。
pub fn store_sidecar(
    conn: &Connection,
    _data_dir: &Path,
    article_id: i64,
    url: &str,
    path: &Path,
    mime: Option<&str>,
    bytes: u64,
) -> Result<PathBuf> {
    if !allowed_url(url) {
        return Err(Error::InvalidUrl(crate::url_norm::InvalidUrlReason::Media));
    }
    record_row(conn, article_id, url, path, mime, "other", bytes)?;
    Ok(path.to_path_buf())
}

fn record_cached(
    conn: &Connection,
    data_dir: &Path,
    article_id: i64,
    url: &str,
    path: &Path,
    mime: Option<&str>,
    kind: &str,
    bytes: u64,
) -> Result<PathBuf> {
    record_row(conn, article_id, url, path, mime, kind, bytes)?;
    let evicted = if kind == "video" {
        evict_direct_lru(conn, data_dir)?
    } else {
        0
    };
    if evicted > 0 {
        tracing::debug!(evicted, kind, "evicted lru media cache entries");
    }
    Ok(path.to_path_buf())
}

fn record_row(
    conn: &Connection,
    article_id: i64,
    url: &str,
    path: &Path,
    mime: Option<&str>,
    kind: &str,
    bytes: u64,
) -> Result<()> {
    let local = path.to_string_lossy().into_owned();
    let now = now_ms();
    let nbytes = i64::try_from(bytes).unwrap_or(i64::MAX);
    if let Some(existing) = lookup(conn, article_id, url)? {
        conn.execute(
            "UPDATE media SET local_path = ?1, mime = ?2, kind = ?3, bytes = ?4, accessed_at = ?5 WHERE id = ?6",
            params![local, mime, kind, nbytes, now, existing.id],
        )?;
    } else {
        conn.execute(
            "INSERT INTO media (article_id, url, local_path, mime, kind, bytes, created_at, accessed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
            params![article_id, url, local, mime, kind, nbytes, now],
        )?;
    }
    Ok(())
}

pub fn get(conn: &Connection, id: i64) -> Result<Option<MediaRow>> {
    conn.query_row(
        "SELECT id, article_id, url, local_path, mime, kind, bytes
         FROM media WHERE id = ?1",
        [id],
        media_from_row,
    )
    .optional()
    .map_err(Into::into)
}

/// 协议读图或缓存命中时刷新 LRU 时间戳。
pub fn touch(conn: &Connection, id: i64) -> Result<()> {
    conn.execute(
        "UPDATE media SET accessed_at = ?1 WHERE id = ?2",
        params![now_ms(), id],
    )?;
    Ok(())
}

/// 缓存总量超过 [`MAX_CACHE_BYTES`] 时，淘汰最久未访问的配图。直链不计入、也不被这次淘汰删掉。
pub fn evict_lru(conn: &Connection, data_dir: &Path) -> Result<usize> {
    evict_lru_until(conn, data_dir, MAX_CACHE_BYTES as i64, "image")
}

/// 直链缓存超过 [`MAX_DIRECT_CACHE_BYTES`] 时，只淘汰 kind = video 的条目。
pub fn evict_direct_lru(conn: &Connection, data_dir: &Path) -> Result<usize> {
    evict_lru_until(conn, data_dir, MAX_DIRECT_CACHE_BYTES as i64, "video")
}

fn evict_lru_until(
    conn: &Connection,
    _data_dir: &Path,
    max_bytes: i64,
    kind: &str,
) -> Result<usize> {
    let mut evicted = 0usize;
    loop {
        let total = cached_bytes(conn, kind)?;
        if total <= max_bytes {
            break;
        }
        let Some((id, local_path)) = oldest_cached(conn, kind)? else {
            break;
        };
        remove_cached(conn, id, &local_path)?;
        evicted += 1;
    }
    Ok(evicted)
}

fn cached_bytes(conn: &Connection, kind: &str) -> Result<i64> {
    conn.query_row(
        "SELECT COALESCE(SUM(bytes), 0) FROM media
         WHERE kind = ?1 AND local_path IS NOT NULL AND local_path != ''",
        [kind],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

fn oldest_cached(conn: &Connection, kind: &str) -> Result<Option<(i64, String)>> {
    conn.query_row(
        "SELECT id, local_path FROM media
         WHERE kind = ?1 AND local_path IS NOT NULL AND local_path != ''
         ORDER BY accessed_at ASC, id ASC
         LIMIT 1",
        [kind],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .optional()
    .map_err(Into::into)
}

pub fn path_refcount(conn: &Connection, local_path: &str, exclude_id: i64) -> Result<i64> {
    conn.query_row(
        "SELECT COUNT(*) FROM media WHERE local_path = ?1 AND id != ?2",
        params![local_path, exclude_id],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

pub fn remove_cached(conn: &Connection, id: i64, local_path: &str) -> Result<()> {
    let refs = path_refcount(conn, local_path, id)?;
    conn.execute("DELETE FROM media WHERE id = ?1", [id])?;
    let path = PathBuf::from(local_path);
    if refs == 0 && in_cache_dir(&path) && path.is_file() {
        let _ = std::fs::remove_file(path);
    }
    Ok(())
}

/// 库里的路径不一定可信（例如恢复了别处来的备份），只删直接放在 `cache/media/` 下的文件。
fn in_cache_dir(path: &Path) -> bool {
    use std::path::Component;
    let mut parts = path.components().rev();
    matches!(parts.next(), Some(Component::Normal(_)))
        && parts.next() == Some(Component::Normal("media".as_ref()))
        && parts.next() == Some(Component::Normal("cache".as_ref()))
        && path.components().all(|c| c != Component::ParentDir)
}

/// 退订源时清理其关联文章的本地图片物理文件及 media 表记录。
pub fn cleanup_feed_media(conn: &Connection, feed_id: i64) -> Result<()> {
    let mut stmt = conn.prepare(
        "SELECT m.id, m.local_path
         FROM media m
         LEFT JOIN articles a ON m.article_id = a.id
         WHERE (a.feed_id = ?1 OR m.feed_id = ?1) AND m.local_path IS NOT NULL AND m.local_path != ''",
    )?;
    let rows: Vec<(i64, String)> = stmt
        .query_map([feed_id], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    for (id, local_path) in rows {
        let _ = remove_cached(conn, id, &local_path);
    }
    Ok(())
}

/// 删除文章时清理其关联的本地图片物理文件及 media 表记录。
pub fn cleanup_article_media(conn: &Connection, article_id: i64) -> Result<()> {
    let mut stmt = conn.prepare(
        "SELECT id, local_path
         FROM media
         WHERE article_id = ?1 AND local_path IS NOT NULL AND local_path != ''",
    )?;
    let rows: Vec<(i64, String)> = stmt
        .query_map([article_id], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    for (id, local_path) in rows {
        let _ = remove_cached(conn, id, &local_path);
    }
    Ok(())
}

pub fn sniff_image_mime(bytes: &[u8]) -> Option<&'static str> {
    sniff_image_ext(bytes).and_then(mime_for_ext)
}

/// 播放器认得的音视频类型。`audio/vnd.wave` 这类厂商类型改成 `audio/wav`。
pub fn canonical_av_mime(raw: &str) -> Option<String> {
    let mime = raw.split(';').next()?.trim().to_ascii_lowercase();
    if mime.is_empty() {
        return None;
    }
    let mime = match mime.as_str() {
        "audio/vnd.wave" | "audio/wave" | "audio/x-wav" => "audio/wav",
        other => other,
    };
    if mime.starts_with("audio/") || mime.starts_with("video/") {
        Some(mime.to_string())
    } else {
        None
    }
}

/// 文件头能确定时，覆盖错误的 Content-Type。
pub fn sniff_av_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WAVE" {
        Some("audio/wav")
    } else if bytes.starts_with(b"ID3")
        || (bytes.len() >= 2 && bytes[0] == 0xFF && bytes[1] & 0xE0 == 0xE0)
    {
        Some("audio/mpeg")
    } else if bytes.len() >= 8 && &bytes[4..8] == b"ftyp" {
        Some("video/mp4")
    } else if bytes.starts_with(b"OggS") {
        Some("audio/ogg")
    } else {
        None
    }
}

/// `Range: bytes=start-end`。end 含在内。无法满足时返回 `Err(())`，调用方回 416。
pub fn parse_byte_range(header: &str, len: u64) -> std::result::Result<(u64, u64), ()> {
    if len == 0 {
        return Err(());
    }
    let rest = header.trim().strip_prefix("bytes=").ok_or(())?;
    let rest = rest.split(',').next().unwrap_or(rest).trim();
    if let Some(suffix) = rest.strip_prefix('-') {
        if suffix.is_empty() {
            return Err(());
        }
        let n: u64 = suffix.parse().map_err(|_| ())?;
        if n == 0 {
            return Err(());
        }
        let n = n.min(len);
        return Ok((len - n, len - 1));
    }
    let (start_s, end_s) = rest.split_once('-').ok_or(())?;
    if start_s.is_empty() {
        return Err(());
    }
    let start: u64 = start_s.parse().map_err(|_| ())?;
    if start >= len {
        return Err(());
    }
    let end = if end_s.is_empty() {
        len - 1
    } else {
        let end: u64 = end_s.parse().map_err(|_| ())?;
        if end < start {
            return Err(());
        }
        end.min(len - 1)
    };
    Ok((start, end))
}

/// 只读 `[start, end]` 这一段，不把整个文件读进内存。
pub fn read_file_span(path: &Path, start: u64, end: u64) -> std::io::Result<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};
    if end < start {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "empty range",
        ));
    }
    let len = usize::try_from(end - start + 1)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidInput, err))?;
    let mut file = std::fs::File::open(path)?;
    file.seek(SeekFrom::Start(start))?;
    let mut buf = vec![0u8; len];
    file.read_exact(&mut buf)?;
    Ok(buf)
}

pub(crate) fn mime_for_ext(ext: &str) -> Option<&'static str> {
    Some(match ext {
        "png" => "image/png",
        "jpg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        "svg" => "image/svg+xml",
        "avif" => "image/avif",
        _ => return None,
    })
}

fn media_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<MediaRow> {
    Ok(MediaRow {
        id: row.get(0)?,
        article_id: row.get::<_, Option<i64>>(1)?.unwrap_or(0),
        url: row.get(2)?,
        local_path: row.get(3)?,
        mime: row.get(4)?,
        kind: row.get(5)?,
        bytes: row.get(6)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_at;
    use crate::feeds::{SubscribeRequest, subscribe};
    use crate::parse::ParsedEntry;
    use tempfile::tempdir;

    #[test]
    fn html_cannot_use_stored_image_mime() {
        for head in [b"<!doctype html><html>".as_slice(), b"<html>"] {
            assert_eq!(
                served_mime("image", Some("image/png"), head),
                "application/octet-stream"
            );
        }
    }

    #[test]
    fn png_mime_comes_from_bytes() {
        assert_eq!(
            served_mime("image", Some("text/html"), &sample_png(8)),
            "image/png"
        );
    }

    #[test]
    fn svg_prefixes_are_recognized() {
        for head in [
            b" \n<svg xmlns='http://www.w3.org/2000/svg'/>".as_slice(),
            b"<?xml version='1.0'?> <!--logo--> <!DOCTYPE svg PUBLIC 'a>b' 'svg.dtd' [<!ENTITY x 'logo'>]> <!--root--> <svg/>",
            b"\xEF\xBB\xBF \t<svg/>",
        ] {
            assert_eq!(served_mime("image", None, head), "image/svg+xml");
        }
    }

    #[test]
    fn svg_detection_stays_in_the_document_prefix() {
        for head in [
            b"".as_slice(),
            b"<",
            b"<svgscript/>",
            b"<?xml",
            b"<?xml?><!DOCTYPE svg",
            b"<?xml?><!--unfinished",
            b"<?xml?><!DOCTYPE html><html><svg/>",
            b"<html><svg/>",
            b"<!doctype html><svg/>",
        ] {
            assert!(!is_svg(head), "{head:?}");
        }
        let mut late = vec![b' '; MEDIA_HEAD_BYTES];
        late.extend_from_slice(b"<svg/>");
        assert!(!is_svg(&late));
    }

    #[test]
    fn av_mime_uses_only_canonical_or_sniffed_types() {
        for (stored, head, expected) in [
            (Some("audio/vnd.wave"), b"".as_slice(), "audio/wav"),
            (None, b"\0\0\0\x18ftyp", "video/mp4"),
            (Some("text/html"), b"<html>", "application/octet-stream"),
        ] {
            assert_eq!(served_mime("video", stored, head), expected);
        }
    }

    #[test]
    fn protocol_headers_restrict_content_without_cors() {
        assert!(MEDIA_SECURITY_HEADERS.contains(&("X-Content-Type-Options", "nosniff")));
        assert!(
            MEDIA_SECURITY_HEADERS
                .contains(&("Content-Security-Policy", "sandbox; default-src 'none'"))
        );
        assert!(
            !MEDIA_SECURITY_HEADERS
                .iter()
                .any(|(name, _)| name.eq_ignore_ascii_case("Access-Control-Allow-Origin"))
        );
    }

    #[test]
    fn protocol_windows_default_to_denied() {
        assert!(media_window_allowed("main"));
        for label in ["web-1", "web-42", "", "Main", "main-1"] {
            assert!(!media_window_allowed(label));
        }
    }

    #[test]
    fn unsupported_image_leaves_no_cache_and_preserves_existing_entry() {
        let (data, conn, id) = seeded();
        let url = "https://cdn.example.com/rejected.png";
        let reject = || store_image(&conn, data.path(), id, url, b"<html>");
        assert!(matches!(reject(), Err(Error::UnsupportedImage)));
        assert!(!cache_dir(data.path()).exists());
        assert!(lookup(&conn, id, url).unwrap().is_none());
        let png = sample_png(8);
        let path = store_image(&conn, data.path(), id, url, &png).unwrap();
        let original = lookup(&conn, id, url).unwrap().unwrap();
        assert!(matches!(reject(), Err(Error::UnsupportedImage)));
        let after = lookup(&conn, id, url).unwrap().unwrap();
        assert_eq!(
            (after.id, after.local_path, after.mime, after.bytes),
            (
                original.id,
                original.local_path,
                original.mime,
                original.bytes
            )
        );
        assert_eq!(std::fs::read(path).unwrap(), png);
    }

    #[test]
    fn svg_is_stored_with_recognized_mime_and_extension() {
        let (data, conn, id) = seeded();
        let url = "https://cdn.example.com/logo";
        let path = store_image(&conn, data.path(), id, url, b"<svg/>").unwrap();
        assert_eq!(path.extension().unwrap(), "svg");
        assert_eq!(std::fs::read(path).unwrap(), b"<svg/>");
        assert_eq!(
            lookup(&conn, id, url).unwrap().unwrap().mime.as_deref(),
            Some("image/svg+xml")
        );
    }

    #[test]
    fn stores_and_reuses_cache_file() {
        let dir = tempdir().unwrap();
        let data = dir.path();
        let conn = open_at(&data.join("rrss.db")).unwrap();
        let feed = subscribe(
            &conn,
            SubscribeRequest {
                url: "https://news.example.com/rss.xml".into(),
                folder_id: None,
                as_rsshub: false,
                rsshub_instance_id: None,
            },
        )
        .unwrap();
        let url = "https://news.example.com/1";
        crate::ingest::upsert_entries(
            &conn,
            feed,
            &[ParsedEntry {
                guid: url.into(),
                canonical_url: Some(url.into()),
                title: "pic".into(),
                author: None,
                published_at: Some(1),
                content_html: Some(
                    "<p><img src=\"https://cdn.example.com/a.png\" alt=\"a\"></p>".into(),
                ),
                hash: crate::parse::entry_hash("pic", Some(url), Some("<p>x</p>")),
                tags: vec![],
            }],
            1,
        )
        .unwrap();
        let article_id: i64 = conn
            .query_row("SELECT id FROM articles", [], |row| row.get(0))
            .unwrap();
        let png = sample_png(8);
        let path = store_image(
            &conn,
            data,
            article_id,
            "https://cdn.example.com/a.png",
            &png,
        )
        .unwrap();
        assert!(path.exists());
        let again = store_image(
            &conn,
            data,
            article_id,
            "https://cdn.example.com/a.png",
            &sample_png(16),
        )
        .unwrap();
        assert_eq!(path, again);
        assert_eq!(std::fs::read(&path).unwrap(), sample_png(16));
        assert_eq!(path.extension().unwrap(), "png");
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM media", [], |row| row.get(0))
            .unwrap();
        assert_eq!(n, 1);
        assert!(matches!(
            store_image(&conn, data, article_id, "ftp://x", &png),
            Err(Error::InvalidUrl(_))
        ));
        let big = vec![0u8; MAX_IMAGE_BYTES + 1];
        assert!(
            store_image(
                &conn,
                data,
                article_id,
                "https://cdn.example.com/big.png",
                &big,
            )
            .is_err()
        );
        let cached = cached_path(&conn, article_id, "https://cdn.example.com/a.png").unwrap();
        assert_eq!(cached.as_deref(), Some(path.as_path()));
        assert_eq!(
            cached_path(&conn, article_id, "https://cdn.example.com/missing.png").unwrap(),
            None
        );
    }

    #[test]
    fn sniffs_png_jpeg_webp_magic() {
        let png = include_bytes!("../../../testdata/render/cover.png");
        assert_eq!(sniff_image_ext(png), Some("png"));
        assert_eq!(sniff_image_ext(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("jpg"));
        let mut webp = b"RIFF".to_vec();
        webp.extend_from_slice(&[0, 0, 0, 0]);
        webp.extend_from_slice(b"WEBPXXXX");
        assert_eq!(sniff_image_ext(&webp), Some("webp"));
        assert_eq!(sniff_image_ext(b"not an image"), None);
        assert_eq!(sniff_image_ext(&[0x89, b'P', b'N', b'G']), None);
    }

    fn ftyp(major: &[u8; 4], compatible: &[[u8; 4]]) -> Vec<u8> {
        let mut body = major.to_vec();
        body.extend_from_slice(&[0, 0, 0, 0]);
        for brand in compatible {
            body.extend_from_slice(brand);
        }
        let mut out = ((8 + body.len()) as u32).to_be_bytes().to_vec();
        out.extend_from_slice(b"ftyp");
        out.extend(body);
        out
    }

    #[test]
    fn avif_brand_uses_image_avif_content_type() {
        let avif = ftyp(b"avif", &[]);
        assert_eq!(sniff_image_ext(&avif), Some("avif"));
        assert_eq!(sniff_image_mime(&avif), Some("image/avif"));
        assert_eq!(
            served_mime("image", Some("application/octet-stream"), &avif),
            "image/avif"
        );
        assert_eq!(sniff_image_ext(&ftyp(b"mif1", &[*b"avis"])), Some("avif"));
        assert_eq!(sniff_image_ext(&ftyp(b"heic", &[*b"mif1"])), None);
        assert_eq!(sniff_image_ext(&avif[..10]), None);
        assert_eq!(sniff_image_ext(b"ftypavifavis"), None);
        let mut short_box = avif.clone();
        short_box[..4].copy_from_slice(&3u32.to_be_bytes());
        assert_eq!(sniff_image_ext(&short_box), None);
    }

    #[test]
    fn stores_real_png_with_extension_even_if_url_is_awebp() {
        let dir = tempdir().unwrap();
        let data = dir.path();
        let conn = open_at(&data.join("rrss.db")).unwrap();
        let feed = subscribe(
            &conn,
            SubscribeRequest {
                url: "https://news.example.com/rss.xml".into(),
                folder_id: None,
                as_rsshub: false,
                rsshub_instance_id: None,
            },
        )
        .unwrap();
        crate::ingest::upsert_entries(
            &conn,
            feed,
            &[ParsedEntry {
                guid: "https://news.example.com/1".into(),
                canonical_url: Some("https://news.example.com/1".into()),
                title: "pic".into(),
                author: None,
                published_at: Some(1),
                content_html: Some("<p>x</p>".into()),
                hash: crate::parse::entry_hash(
                    "pic",
                    Some("https://news.example.com/1"),
                    Some("<p>x</p>"),
                ),
                tags: vec![],
            }],
            1,
        )
        .unwrap();
        let article_id: i64 = conn
            .query_row("SELECT id FROM articles", [], |row| row.get(0))
            .unwrap();
        let png = include_bytes!("../../../testdata/render/cover.png");
        let path = store_image(
            &conn,
            data,
            article_id,
            "https://cdn.example.com/cover.awebp",
            png,
        )
        .unwrap();
        assert_eq!(path.extension().and_then(|e| e.to_str()), Some("png"));
        let row = lookup(&conn, article_id, "https://cdn.example.com/cover.awebp")
            .unwrap()
            .unwrap();
        assert_eq!(row.mime.as_deref(), Some("image/png"));
    }

    fn sample_png(len: usize) -> Vec<u8> {
        let mut bytes = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        bytes.resize(len.max(8), 0);
        bytes
    }

    fn article_id(conn: &Connection) -> i64 {
        conn.query_row("SELECT id FROM articles LIMIT 1", [], |row| row.get(0))
            .unwrap()
    }

    #[test]
    fn evicts_least_recently_used_when_over_limit() {
        let dir = tempdir().unwrap();
        let data = dir.path();
        let conn = open_at(&data.join("rrss.db")).unwrap();
        let feed = subscribe(
            &conn,
            SubscribeRequest {
                url: "https://news.example.com/rss.xml".into(),
                folder_id: None,
                as_rsshub: false,
                rsshub_instance_id: None,
            },
        )
        .unwrap();
        crate::ingest::upsert_entries(
            &conn,
            feed,
            &[ParsedEntry {
                guid: "https://news.example.com/1".into(),
                canonical_url: Some("https://news.example.com/1".into()),
                title: "pic".into(),
                author: None,
                published_at: Some(1),
                content_html: Some("<p>x</p>".into()),
                hash: crate::parse::entry_hash(
                    "pic",
                    Some("https://news.example.com/1"),
                    Some("<p>x</p>"),
                ),
                tags: vec![],
            }],
            1,
        )
        .unwrap();
        let article_id = article_id(&conn);
        let payload = sample_png(40);
        store_image(
            &conn,
            data,
            article_id,
            "https://cdn.example.com/1.png",
            &payload,
        )
        .unwrap();
        store_image(
            &conn,
            data,
            article_id,
            "https://cdn.example.com/2.png",
            &payload,
        )
        .unwrap();
        store_image(
            &conn,
            data,
            article_id,
            "https://cdn.example.com/3.png",
            &payload,
        )
        .unwrap();
        conn.execute(
            "UPDATE media SET accessed_at = 100 WHERE url = 'https://cdn.example.com/1.png'",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE media SET accessed_at = 200 WHERE url = 'https://cdn.example.com/2.png'",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE media SET accessed_at = 300 WHERE url = 'https://cdn.example.com/3.png'",
            [],
        )
        .unwrap();

        let evicted = evict_lru_until(&conn, data, 80, "image").unwrap();
        assert!(evicted >= 1);
        assert!(
            cached_path(&conn, article_id, "https://cdn.example.com/1.png")
                .unwrap()
                .is_none()
        );
        assert!(
            cached_path(&conn, article_id, "https://cdn.example.com/3.png")
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn touch_updates_accessed_at() {
        let dir = tempdir().unwrap();
        let data = dir.path();
        let conn = open_at(&data.join("rrss.db")).unwrap();
        let feed = subscribe(
            &conn,
            SubscribeRequest {
                url: "https://news.example.com/rss.xml".into(),
                folder_id: None,
                as_rsshub: false,
                rsshub_instance_id: None,
            },
        )
        .unwrap();
        crate::ingest::upsert_entries(
            &conn,
            feed,
            &[ParsedEntry {
                guid: "https://news.example.com/1".into(),
                canonical_url: Some("https://news.example.com/1".into()),
                title: "pic".into(),
                author: None,
                published_at: Some(1),
                content_html: Some("<p>x</p>".into()),
                hash: crate::parse::entry_hash(
                    "pic",
                    Some("https://news.example.com/1"),
                    Some("<p>x</p>"),
                ),
                tags: vec![],
            }],
            1,
        )
        .unwrap();
        let article_id = article_id(&conn);
        store_image(
            &conn,
            data,
            article_id,
            "https://cdn.example.com/a.png",
            &sample_png(16),
        )
        .unwrap();
        conn.execute("UPDATE media SET accessed_at = 1", [])
            .unwrap();
        let id: i64 = conn
            .query_row("SELECT id FROM media LIMIT 1", [], |row| row.get(0))
            .unwrap();
        touch(&conn, id).unwrap();
        let accessed: i64 = conn
            .query_row("SELECT accessed_at FROM media WHERE id = ?1", [id], |row| {
                row.get(0)
            })
            .unwrap();
        assert!(accessed > 1);
    }

    #[test]
    fn remove_cached_only_deletes_files_inside_media_cache() {
        let (data, conn, article_id) = seeded();
        let outside = data.path().join("notes.txt");
        std::fs::write(&outside, b"keep").unwrap();
        let inside = cache_path(data.path(), "https://cdn.example.com/a.png");
        std::fs::create_dir_all(inside.parent().unwrap()).unwrap();
        std::fs::write(&inside, b"png").unwrap();
        let sneaky = cache_dir(data.path())
            .join("..")
            .join("..")
            .join("notes.txt");
        for (url, path) in [("u1", &outside), ("u2", &sneaky), ("u3", &inside)] {
            conn.execute(
                "INSERT INTO media (article_id, url, local_path, kind, created_at) VALUES (?1, ?2, ?3, 'image', 1)",
                rusqlite::params![article_id, url, path.to_string_lossy()],
            )
            .unwrap();
            let id = conn.last_insert_rowid();
            remove_cached(&conn, id, &path.to_string_lossy()).unwrap();
        }
        assert!(outside.is_file());
        assert!(!inside.exists());
        let left: i64 = conn
            .query_row("SELECT COUNT(*) FROM media WHERE url LIKE 'u%'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(left, 0);
    }

    #[test]
    fn direct_file_over_limit_is_rejected_without_counting_as_image() {
        assert!(direct_len_allowed(MAX_DIRECT_BYTES as u64));
        assert!(!direct_len_allowed(MAX_DIRECT_BYTES as u64 + 1));
        let (data, conn, article_id) = seeded();
        let path = cache_path(data.path(), "https://cdn.example.com/clip.mp4");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"mp4").unwrap();
        let err = store_direct(
            &conn,
            data.path(),
            article_id,
            "https://cdn.example.com/clip.mp4",
            &path,
            Some("video/mp4"),
            MAX_DIRECT_BYTES as u64 + 1,
        );
        assert!(err.is_err());
        assert!(
            lookup(&conn, article_id, "https://cdn.example.com/clip.mp4")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn direct_cache_does_not_evict_images_and_image_lru_ignores_video() {
        let (data, conn, article_id) = seeded();
        let png = sample_png(32);
        store_image(
            &conn,
            data.path(),
            article_id,
            "https://cdn.example.com/keep.png",
            &png,
        )
        .unwrap();
        let video = cache_path(data.path(), "https://cdn.example.com/big.mp4");
        std::fs::write(&video, b"v").unwrap();
        store_direct(
            &conn,
            data.path(),
            article_id,
            "https://cdn.example.com/big.mp4",
            &video,
            Some("video/mp4"),
            4,
        )
        .unwrap();
        conn.execute(
            "UPDATE media SET bytes = ?1 WHERE url = 'https://cdn.example.com/big.mp4'",
            [MAX_CACHE_BYTES as i64 + 1],
        )
        .unwrap();
        let evicted = evict_lru(&conn, data.path()).unwrap();
        assert_eq!(
            evicted, 0,
            "a direct file must not push images out of the image budget"
        );
        assert!(
            cached_path(&conn, article_id, "https://cdn.example.com/keep.png")
                .unwrap()
                .is_some()
        );
        let older = cache_path(data.path(), "https://cdn.example.com/old.mp4");
        std::fs::write(&older, b"o").unwrap();
        store_direct(
            &conn,
            data.path(),
            article_id,
            "https://cdn.example.com/old.mp4",
            &older,
            Some("video/mp4"),
            4,
        )
        .unwrap();
        conn.execute(
            "UPDATE media SET bytes = ?1, accessed_at = 1 WHERE url = 'https://cdn.example.com/old.mp4'",
            [MAX_DIRECT_CACHE_BYTES as i64],
        )
        .unwrap();
        conn.execute(
            "UPDATE media SET bytes = ?1, accessed_at = 2 WHERE url = 'https://cdn.example.com/big.mp4'",
            [64],
        )
        .unwrap();
        let evicted = evict_direct_lru(&conn, data.path()).unwrap();
        assert!(evicted >= 1);
        assert!(
            lookup(&conn, article_id, "https://cdn.example.com/old.mp4")
                .unwrap()
                .is_none()
        );
        assert!(
            lookup(&conn, article_id, "https://cdn.example.com/big.mp4")
                .unwrap()
                .is_some()
        );
        assert!(
            cached_path(&conn, article_id, "https://cdn.example.com/keep.png")
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn byte_range_reads_only_the_requested_span() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("clip.mp4");
        let body: Vec<u8> = (0..100).collect();
        std::fs::write(&path, &body).unwrap();
        let (start, end) = parse_byte_range("bytes=10-13", body.len() as u64).unwrap();
        assert_eq!((start, end), (10, 13));
        let span = read_file_span(&path, start, end).unwrap();
        assert_eq!(span, &body[10..=13]);
        assert_ne!(span.len(), body.len());
        let (start, end) = parse_byte_range("bytes=90-", body.len() as u64).unwrap();
        assert_eq!((start, end), (90, 99));
        assert!(parse_byte_range("bytes=100-110", body.len() as u64).is_err());
        let (start, end) = parse_byte_range("bytes=-4", body.len() as u64).unwrap();
        assert_eq!((start, end), (96, 99));
    }

    fn seeded() -> (tempfile::TempDir, crate::db::PooledConnection, i64) {
        let dir = tempdir().unwrap();
        let data = dir.path();
        let conn = open_at(&data.join("rrss.db")).unwrap();
        let feed = subscribe(
            &conn,
            SubscribeRequest {
                url: "https://news.example.com/rss.xml".into(),
                folder_id: None,
                as_rsshub: false,
                rsshub_instance_id: None,
            },
        )
        .unwrap();
        crate::ingest::upsert_entries(
            &conn,
            feed,
            &[ParsedEntry {
                guid: "https://news.example.com/1".into(),
                canonical_url: Some("https://news.example.com/1".into()),
                title: "pic".into(),
                author: None,
                published_at: Some(1),
                content_html: Some("<p>x</p>".into()),
                hash: crate::parse::entry_hash(
                    "pic",
                    Some("https://news.example.com/1"),
                    Some("<p>x</p>"),
                ),
                tags: vec![],
            }],
            1,
        )
        .unwrap();
        let id = article_id(&conn);
        (dir, conn, id)
    }
}
