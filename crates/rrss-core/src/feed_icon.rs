//! 网站图标选择、重试间隔与媒体缓存登记。
use std::{cell::RefCell, path::Path};

use html5ever::{
    tendril::StrTendril,
    tokenizer::{BufferQueue, TagKind, Token, TokenSink, TokenSinkResult, Tokenizer},
};
use rusqlite::{Connection, params};
use sha2::{Digest, Sha256};
use url::Url;

use crate::{
    db,
    error::{Error, Result},
    media,
};

pub const SETTING_KEY: &str = "feed_icons_enabled";
pub const RETRY_MS: i64 = 7 * 86_400_000;
pub const MAX_BYTES: usize = 64 * 1024;

pub fn enabled(conn: &Connection) -> Result<bool> {
    Ok(db::get_setting(conn, SETTING_KEY)?.as_deref() != Some("0"))
}

pub fn due(checked: Option<i64>, now: i64) -> bool {
    checked.is_none_or(|t| now.saturating_sub(t) >= RETRY_MS)
}

struct Links(RefCell<Vec<(String, u32)>>);
impl TokenSink for Links {
    type Handle = ();
    fn process_token(&self, token: Token, _: u64) -> TokenSinkResult<()> {
        if let Token::TagToken(ref tag) = token
            && tag.kind == TagKind::StartTag
        {
            use html5ever::tokenizer::states::RawKind;
            match tag.name.as_ref() {
                "script" => return TokenSinkResult::RawData(RawKind::ScriptData),
                "style" => return TokenSinkResult::RawData(RawKind::Rawtext),
                "title" | "textarea" => return TokenSinkResult::RawData(RawKind::Rcdata),
                _ => {}
            }
        }
        if let Token::TagToken(tag) = token
            && tag.kind == TagKind::StartTag
            && tag.name.as_ref() == "link"
        {
            let attr = |name: &str| {
                tag.attrs
                    .iter()
                    .find(|a| a.name.local.as_ref() == name)
                    .map(|a| a.value.to_string())
                    .unwrap_or_default()
            };
            if attr("rel").split_ascii_whitespace().any(|r| {
                r.eq_ignore_ascii_case("icon") || r.eq_ignore_ascii_case("apple-touch-icon")
            }) {
                let mime = attr("type").to_ascii_lowercase();
                if mime.is_empty()
                    || matches!(
                        mime.as_str(),
                        "image/png"
                            | "image/jpeg"
                            | "image/webp"
                            | "image/x-icon"
                            | "image/vnd.microsoft.icon"
                    )
                {
                    let distance = attr("sizes")
                        .split_ascii_whitespace()
                        .filter_map(|s| {
                            let (w, h) = s.split_once('x')?;
                            let (w, h) = (w.parse::<u32>().ok()?, h.parse::<u32>().ok()?);
                            Some(w.abs_diff(64).saturating_add(h.abs_diff(64)))
                        })
                        .min()
                        .unwrap_or(128);
                    self.0.borrow_mut().push((attr("href"), distance));
                }
            }
        }
        TokenSinkResult::Continue
    }
}

pub fn select_url(home: &Url, html: &str) -> Option<Url> {
    let tokenizer = Tokenizer::new(Links(RefCell::new(Vec::new())), Default::default());
    let input = BufferQueue::default();
    input.push_back(StrTendril::from(html));
    let _ = tokenizer.feed(&input);
    tokenizer.end();
    let mut links = tokenizer.sink.0.into_inner();
    links.sort_by_key(|(_, distance)| *distance);
    links
        .into_iter()
        .filter(|(href, _)| !href.trim().is_empty())
        .filter_map(|(href, _)| home.join(&href).ok())
        .find(|url| {
            media::allowed_url(url.as_str()) && !url.path().to_ascii_lowercase().ends_with(".svg")
        })
        .or_else(|| home.join("/favicon.ico").ok())
}

pub fn image_type(bytes: &[u8], mime: Option<&str>) -> Option<&'static str> {
    if bytes.len() > MAX_BYTES {
        return None;
    }
    let actual = match media::sniff_image_ext(bytes)? {
        "png" => "image/png",
        "jpg" => "image/jpeg",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        _ => return None,
    };
    if let Some(mime) = mime {
        let mime = mime.split(';').next()?.trim().to_ascii_lowercase();
        if mime != actual
            && mime != "application/octet-stream"
            && !(actual == "image/x-icon" && mime == "image/vnd.microsoft.icon")
        {
            return None;
        }
    }
    Some(actual)
}

pub fn store(
    conn: &Connection,
    data_dir: &Path,
    feed_id: i64,
    url: &str,
    bytes: &[u8],
    mime: Option<&str>,
) -> Result<()> {
    let mime =
        image_type(bytes, mime).ok_or_else(|| Error::InvalidUrl("invalid feed icon".into()))?;
    if !media::allowed_url(url) {
        return Err(Error::InvalidUrl(url.into()));
    }
    let tx = db::write_transaction(conn)?;
    // 关闭设置或退订期间到达的下载结果不再落盘。
    let exists: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM feeds WHERE id=?1)",
        [feed_id],
        |r| r.get(0),
    )?;
    if !exists || !enabled(&tx)? {
        return Ok(());
    }
    let old: Vec<(i64, String)> = tx
        .prepare("SELECT id, local_path FROM media WHERE feed_id=?1")?
        .query_map([feed_id], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let path =
        media::cache_dir(data_dir).join(format!("feed-icon-{feed_id}-{:x}", Sha256::digest(bytes)));
    std::fs::create_dir_all(media::cache_dir(data_dir))?;
    std::fs::write(&path, bytes)?;
    tx.execute("DELETE FROM media WHERE feed_id=?1", [feed_id])?;
    let now = crate::feeds::now_ms();
    tx.execute("INSERT INTO media(feed_id, url, local_path, mime, kind, bytes, created_at, accessed_at) VALUES (?1, ?2, ?3, ?4, 'image', ?5, ?6, ?6)", params![feed_id, url, path.to_string_lossy(), mime, bytes.len() as i64, now])?;
    tx.commit()?;
    for (id, old_path) in old {
        media::remove_cached(conn, id, &old_path)?;
    }
    media::evict_lru(conn, data_dir)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selects_nearest_resolves_links_and_falls_back() {
        let home = Url::parse("https://example.test/news/").unwrap();
        assert_eq!(
            select_url(&home, include_str!("../../../testdata/feeds/icons.html"))
                .unwrap()
                .as_str(),
            "https://example.test/icon64.png"
        );
        assert_eq!(
            select_url(
                &home,
                "<link rel=icon href='data:image/png,x'><link rel=icon href='/icon.svg'>"
            )
            .unwrap()
            .as_str(),
            "https://example.test/favicon.ico"
        );
        assert_eq!(
            select_url(
                &home,
                "<script>const html = '<link rel=icon href=/fake.png>';</script>"
            )
            .unwrap()
            .as_str(),
            "https://example.test/favicon.ico"
        );
    }
    #[test]
    fn restricts_type_size_and_retry_time() {
        let png = include_bytes!("../../../testdata/render/cover.png");
        assert_eq!(image_type(png, Some("image/png")), Some("image/png"));
        assert_eq!(image_type(png, Some("image/svg+xml")), None);
        assert_eq!(image_type(b"<svg/>", Some("image/png")), None);
        assert_eq!(image_type(b"GIF89a", None), None);
        let mut large = png.to_vec();
        large.resize(MAX_BYTES + 1, 0);
        assert_eq!(image_type(&large, None), None);
        assert!(due(None, 0));
        assert!(!due(Some(100), 100 + RETRY_MS - 1));
        assert!(due(Some(100), 100 + RETRY_MS));
    }
}
