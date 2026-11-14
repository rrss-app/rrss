//! 官方站封面元数据与直链下载。封面图仍走配图存储；直链限额分开。
//! 不在这里解析 YouTube 地址，分类用 `rrss-render`。

use std::path::{Path, PathBuf};

use reqwest::header::{HeaderValue, REFERER};
use rrss_core::media::{self, cache_key};
use rrss_render::{Block, DirectMedia, Document, EmbedClass, EmbedKind, present_embed};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::image_fetch::{self, FetchFail, IMAGE_UA};
use crate::queries::{cached_media_ref, media_href_with_fingerprint};
use crate::views::{EmbedCoverView, MediaRef};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoverMeta {
    pub site: &'static str,
    pub title: String,
    pub author: String,
    pub duration_secs: Option<u32>,
    pub thumbnail_url: String,
    pub open_url: String,
}

pub(crate) const CONTENT_BUSY_MESSAGE: &str = "内容网络繁忙，请稍后重试";

#[derive(Debug, Clone)]
pub enum DirectFail {
    TooLarge,
    NotDirect,
    Http(u16),
    Network(crate::UiError),
    Store,
}

impl std::fmt::Display for DirectFail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooLarge => write!(f, "too_large"),
            Self::NotDirect => write!(f, "not_direct"),
            Self::Http(status) => write!(f, "http {status}"),
            Self::Network(msg) => write!(f, "{msg}"),
            Self::Store => write!(f, "store"),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct MetaFile {
    site: String,
    title: String,
    author: String,
    duration_secs: Option<u32>,
    open_url: String,
    thumbnail_url: String,
}

pub fn parse_youtube_oembed(body: &str, open_url: &str) -> Option<CoverMeta> {
    let value: Value = serde_json::from_str(body).ok()?;
    let title = value.get("title")?.as_str()?.trim();
    if title.is_empty() {
        return None;
    }
    let author = value
        .get("author_name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let thumbnail_url = value.get("thumbnail_url")?.as_str()?.trim().to_string();
    if !media::allowed_url(&thumbnail_url) {
        return None;
    }
    Some(CoverMeta {
        site: "youtube",
        title: title.to_string(),
        author,
        duration_secs: None,
        thumbnail_url,
        open_url: open_url.to_string(),
    })
}

pub fn parse_bilibili_view(body: &str, open_url: &str) -> Option<CoverMeta> {
    let value: Value = serde_json::from_str(body).ok()?;
    if value.get("code").and_then(|v| v.as_i64()) != Some(0) {
        return None;
    }
    let data = value.get("data")?;
    let title = data.get("title")?.as_str()?.trim();
    if title.is_empty() {
        return None;
    }
    let author = data
        .pointer("/owner/name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let thumbnail_url = data.get("pic")?.as_str()?.trim().to_string();
    if !media::allowed_url(&thumbnail_url) {
        return None;
    }
    let duration_secs = data.get("duration").and_then(json_u32);
    Some(CoverMeta {
        site: "bilibili",
        title: title.to_string(),
        author,
        duration_secs,
        thumbnail_url,
        open_url: open_url.to_string(),
    })
}

fn json_u32(value: &Value) -> Option<u32> {
    value
        .as_u64()
        .or_else(|| value.as_i64().filter(|n| *n >= 0).map(|n| n as u64))
        .and_then(|n| u32::try_from(n).ok())
}

pub fn present_document(doc: &mut Document) {
    for block in &mut doc.blocks {
        if let Block::Embed { kind, url, title } = block {
            let presented = present_embed(*kind, url, title);
            *kind = presented.kind;
            *url = presented.url;
            *title = presented.title;
        }
    }
}

pub fn official_open_urls(doc: &Document) -> Vec<String> {
    let mut urls = Vec::new();
    for block in &doc.blocks {
        if let Block::Embed { kind, url, .. } = block {
            if matches!(kind, EmbedKind::Youtube | EmbedKind::Bilibili) && !urls.contains(url) {
                urls.push(url.clone());
            }
        }
    }
    urls
}

pub fn cached_covers(
    conn: &rusqlite::Connection,
    article_id: i64,
    doc: &Document,
) -> Vec<EmbedCoverView> {
    official_open_urls(doc)
        .into_iter()
        .filter_map(|url| read_cached(conn, article_id, &url))
        .collect()
}

pub fn direct_in_document(doc: &Document, url: &str) -> Option<DirectMedia> {
    let wanted = url.trim();
    for block in &doc.blocks {
        let Block::Embed {
            kind,
            url: block_url,
            title,
        } = block
        else {
            continue;
        };
        let presented = present_embed(*kind, block_url, title);
        if presented.url != wanted && block_url != wanted {
            continue;
        }
        if let EmbedClass::Direct { media, .. } =
            rrss_render::classify_for_read(&presented.url, presented.kind)
        {
            return Some(media);
        }
    }
    None
}

pub async fn fetch_official_cover(
    client: &reqwest::Client,
    db_path: &Path,
    data_dir: &Path,
    article_id: i64,
    open_url: &str,
) -> Result<EmbedCoverView, FetchFail> {
    if let Some(view) = cached_on(db_path, article_id, open_url) {
        return Ok(view);
    }
    let class = rrss_render::classify_for_read(open_url, EmbedKind::Iframe);
    let (request_url, referer, open_url, bilibili) = match class {
        EmbedClass::Youtube { open_url, .. } => (
            youtube_oembed_url(&open_url),
            "https://www.youtube.com/",
            open_url,
            false,
        ),
        EmbedClass::Bilibili { id, open_url } => (
            format!("https://api.bilibili.com/x/web-interface/view?bvid={id}"),
            "https://www.bilibili.com/",
            open_url,
            true,
        ),
        _ => return Err(FetchFail::Http(0)),
    };
    let capacity = crate::scheduler::model_capacity::Capacity::for_database(db_path);
    let permit = capacity
        .content
        .clone()
        .acquire(crate::scheduler::model_capacity::Priority::Interactive)
        .await
        .map_err(|_| FetchFail::Network(CONTENT_BUSY_MESSAGE.into()))?;
    let body = get_text(client, &request_url, referer).await?;
    drop(permit);
    let meta = if bilibili {
        parse_bilibili_view(&body, &open_url)
    } else {
        parse_youtube_oembed(&body, &open_url)
    }
    .ok_or(FetchFail::Decode)?;
    write_meta(db_path, data_dir, article_id, &meta).map_err(|_| FetchFail::Store)?;
    let thumb_referer = if meta.site == "bilibili" {
        "https://www.bilibili.com/"
    } else {
        "https://www.youtube.com/"
    };
    image_fetch::fetch_and_store(
        client,
        db_path,
        data_dir,
        article_id,
        &meta.thumbnail_url,
        thumb_referer,
    )
    .await?;
    cached_on(db_path, article_id, &meta.open_url).ok_or(FetchFail::Store)
}

pub async fn download_direct(
    network: &crate::network::NetworkClients,
    db_path: &Path,
    data_dir: &Path,
    article_id: i64,
    url: &str,
    referer: &str,
) -> Result<MediaRef, DirectFail> {
    let doc = load_doc(db_path, article_id).map_err(|err| DirectFail::Network(err))?;
    let Some(media_kind) = direct_in_document(&doc, url) else {
        return Err(DirectFail::NotDirect);
    };
    let presented = present_embed(
        match media_kind {
            DirectMedia::Audio => EmbedKind::Audio,
            DirectMedia::Video => EmbedKind::Video,
        },
        url,
        "",
    );
    let url = presented.url;
    if let Some(href) = cached_direct(db_path, article_id, &url) {
        return Ok(href);
    }
    if !media::allowed_url(&url) {
        return Err(DirectFail::NotDirect);
    }
    let client = network
        .client(rrss_core::network::NetworkScope::Content)
        .await
        .map_err(|err| DirectFail::Network(crate::UiError::from(err)))?;
    let capacity = crate::scheduler::model_capacity::Capacity::for_database(db_path);
    let permit = capacity
        .content
        .clone()
        .acquire(crate::scheduler::model_capacity::Priority::Interactive)
        .await
        .map_err(|_| {
            DirectFail::Network(crate::UiError::from(
                crate::ui_error::reader::ReaderError::ContentBusy,
            ))
        })?;
    let mut resp = send_get(&client, &url, referer)
        .await
        .map_err(DirectFail::Network)?;
    let status = resp.status();
    if !status.is_success() {
        return Err(DirectFail::Http(status.as_u16()));
    }
    if resp
        .content_length()
        .is_some_and(|n| !media::direct_len_allowed(n))
    {
        return Err(DirectFail::TooLarge);
    }
    let mime = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.split(';').next().unwrap_or(s).trim().to_string());
    let dest = media::cache_path(data_dir, &url);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| DirectFail::Network(crate::UiError::from(err.to_string())))?;
    }
    let tmp = dest.with_extension("part");
    // 网络中断或取消时同样清理半成品，不把失败下载留在缓存目录。
    struct PartialFile(PathBuf);
    impl Drop for PartialFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    let _partial = PartialFile(tmp.clone());
    let mut file = std::fs::File::create(&tmp)
        .map_err(|err| DirectFail::Network(crate::UiError::from(err.to_string())))?;
    let mut got = 0u64;
    loop {
        let chunk = resp.chunk().await.map_err(|err| {
            DirectFail::Network(crate::UiError::from(rrss_core::network::request_error(
                &err,
            )))
        })?;
        let Some(chunk) = chunk else { break };
        got += chunk.len() as u64;
        if !media::direct_len_allowed(got) {
            drop(file);
            let _ = std::fs::remove_file(&tmp);
            return Err(DirectFail::TooLarge);
        }
        std::io::Write::write_all(&mut file, &chunk)
            .map_err(|err| DirectFail::Network(crate::UiError::from(err.to_string())))?;
    }
    drop(file);
    drop(permit);
    if std::fs::rename(&tmp, &dest).is_err() {
        let _ = std::fs::remove_file(&tmp);
        return Err(DirectFail::Store);
    }
    let mime = mime_for_direct(&url, mime.as_deref());
    let conn = rrss_core::db::open_at(db_path)
        .map_err(|err| DirectFail::Network(crate::UiError::from(err)))?;
    media::store_direct(&conn, data_dir, article_id, &url, &dest, Some(&mime), got)
        .map_err(|_| DirectFail::Store)?;
    cached_direct(db_path, article_id, &url).ok_or(DirectFail::Store)
}

fn mime_for_direct(url: &str, header: Option<&str>) -> String {
    if let Some(header) = header {
        if let Some(mime) = rrss_core::media::canonical_av_mime(header) {
            return mime;
        }
    }
    let ext = url
        .rsplit(['/', '?', '#'])
        .next()
        .and_then(|seg| seg.rsplit_once('.'))
        .map(|(_, ext)| ext.to_ascii_lowercase());
    match ext.as_deref() {
        Some("mp4") => "video/mp4",
        Some("webm") => "video/webm",
        Some("ogv") => "video/ogg",
        Some("ogg") => "audio/ogg",
        Some("mp3") => "audio/mpeg",
        Some("m4a") => "audio/mp4",
        Some("aac") => "audio/aac",
        Some("wav") => "audio/wav",
        Some("opus") => "audio/opus",
        _ => "application/octet-stream",
    }
    .to_string()
}

pub fn youtube_oembed_url(open_url: &str) -> String {
    format!(
        "https://www.youtube.com/oembed?url={}&format=json",
        encode_query(open_url)
    )
}

fn encode_query(value: &str) -> String {
    let mut out = String::new();
    for b in value.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn read_cached(
    conn: &rusqlite::Connection,
    article_id: i64,
    open_url: &str,
) -> Option<EmbedCoverView> {
    let row = media::lookup(conn, article_id, open_url).ok().flatten()?;
    let path = row.local_path.filter(|p| !p.is_empty())?;
    let text = std::fs::read_to_string(path).ok()?;
    let meta: MetaFile = serde_json::from_str(&text).ok()?;
    let image = media::lookup(conn, article_id, &meta.thumbnail_url)
        .ok()
        .flatten()?;
    let image_path = image.local_path.filter(|p| !p.is_empty())?;
    if !Path::new(&image_path).exists() {
        return None;
    }
    let key = cache_key(&image.url);
    let fp = if key.len() >= 16 { &key[..16] } else { &key };
    Some(EmbedCoverView {
        url: meta.open_url.clone(),
        open_url: meta.open_url,
        site: meta.site,
        title: meta.title,
        author: meta.author,
        duration_secs: meta.duration_secs,
        cover_href: media_href_with_fingerprint(image.id, fp),
        failed: false,
    })
}

fn cached_on(db_path: &Path, article_id: i64, open_url: &str) -> Option<EmbedCoverView> {
    let conn = rrss_core::db::open_at(db_path).ok()?;
    read_cached(&conn, article_id, open_url)
}

fn cached_direct(db_path: &Path, article_id: i64, url: &str) -> Option<MediaRef> {
    let conn = rrss_core::db::open_at(db_path).ok()?;
    let row = media::lookup(&conn, article_id, url).ok().flatten()?;
    if row.kind != "video" {
        return None;
    }
    let cached = row
        .local_path
        .as_deref()
        .is_some_and(|p| !p.is_empty() && Path::new(p).exists());
    cached.then(|| cached_media_ref(&row, url))
}

fn write_meta(
    db_path: &Path,
    data_dir: &Path,
    article_id: i64,
    meta: &CoverMeta,
) -> rrss_core::Result<PathBuf> {
    let dir = media::cache_dir(data_dir);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("meta-{}", cache_key(&meta.open_url)));
    let file = MetaFile {
        site: meta.site.to_string(),
        title: meta.title.clone(),
        author: meta.author.clone(),
        duration_secs: meta.duration_secs,
        open_url: meta.open_url.clone(),
        thumbnail_url: meta.thumbnail_url.clone(),
    };
    let bytes = serde_json::to_vec(&file).map_err(|err| rrss_core::Error::Http {
        status: 0,
        message: err.to_string(),
    })?;
    std::fs::write(&path, &bytes)?;
    let conn = rrss_core::db::open_at(db_path)?;
    media::store_sidecar(
        &conn,
        data_dir,
        article_id,
        &meta.open_url,
        &path,
        Some("application/json"),
        bytes.len() as u64,
    )
}

fn load_doc(db_path: &Path, article_id: i64) -> Result<Document, crate::UiError> {
    let conn = rrss_core::db::open_at(db_path).map_err(crate::UiError::from)?;
    let article =
        rrss_core::articles::get_article(&conn, article_id).map_err(crate::UiError::from)?;
    let mut doc = crate::document::load_document(&article);
    present_document(&mut doc);
    Ok(doc)
}

async fn get_text(client: &reqwest::Client, url: &str, referer: &str) -> Result<String, FetchFail> {
    let resp = send_get(client, url, referer)
        .await
        .map_err(|error| FetchFail::Network(error.to_string()))?;
    if !resp.status().is_success() {
        return Err(FetchFail::Http(resp.status().as_u16()));
    }
    resp.text()
        .await
        .map_err(|err| FetchFail::Network(rrss_core::network::request_error(&err).to_string()))
}

async fn send_get(
    client: &reqwest::Client,
    url: &str,
    referer: &str,
) -> Result<reqwest::Response, crate::UiError> {
    let mut builder = client
        .get(url)
        .header(
            reqwest::header::USER_AGENT,
            HeaderValue::from_static(IMAGE_UA),
        )
        .timeout(std::time::Duration::from_secs(20));
    if !referer.is_empty()
        && let Ok(value) = HeaderValue::from_str(referer)
    {
        builder = builder.header(REFERER, value);
    }
    builder
        .send()
        .await
        .map_err(|err| crate::UiError::from(rrss_core::network::request_error(&err)))
}

pub fn failed_cover(open_url: &str, site: &str) -> EmbedCoverView {
    EmbedCoverView {
        url: open_url.to_string(),
        open_url: open_url.to_string(),
        site: site.to_string(),
        title: String::new(),
        author: String::new(),
        duration_secs: None,
        cover_href: String::new(),
        failed: true,
    }
}

#[cfg(test)]
mod network_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn youtube_oembed_ignores_duration() {
        let body = r#"{
            "title": "示例视频",
            "author_name": "示例作者",
            "thumbnail_url": "https://i.ytimg.com/vi/abc123def45/hqdefault.jpg",
            "duration": 99
        }"#;
        let meta =
            parse_youtube_oembed(body, "https://www.youtube.com/watch?v=abc123def45").unwrap();
        assert_eq!(meta.title, "示例视频");
        assert_eq!(meta.author, "示例作者");
        assert_eq!(
            meta.thumbnail_url,
            "https://i.ytimg.com/vi/abc123def45/hqdefault.jpg"
        );
        assert_eq!(meta.duration_secs, None);
        assert_eq!(meta.site, "youtube");
        let url = youtube_oembed_url("https://www.youtube.com/watch?v=abc123def45");
        assert!(url.starts_with("https://www.youtube.com/oembed?url="));
        assert!(url.contains("format=json"));
        assert!(!url.contains("watch?v="));
    }

    #[test]
    fn bilibili_view_reads_owner_pic_and_duration() {
        let body = r#"{
            "code": 0,
            "data": {
                "title": "示例",
                "pic": "https://i0.hdslb.com/bfs/archive/abc.jpg",
                "duration": 482,
                "owner": { "name": "UP主" }
            }
        }"#;
        let meta =
            parse_bilibili_view(body, "https://www.bilibili.com/video/BV1xx411c7mD/").unwrap();
        assert_eq!(meta.title, "示例");
        assert_eq!(meta.author, "UP主");
        assert_eq!(
            meta.thumbnail_url,
            "https://i0.hdslb.com/bfs/archive/abc.jpg"
        );
        assert_eq!(meta.duration_secs, Some(482));
        assert_eq!(meta.site, "bilibili");
    }

    #[test]
    fn direct_quota_constant_rejects_oversize_length() {
        assert!(rrss_core::media::direct_len_allowed(64 * 1024 * 1024));
        assert!(!rrss_core::media::direct_len_allowed(64 * 1024 * 1024 + 1));
    }
}
