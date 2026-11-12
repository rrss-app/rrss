use std::path::{Path, PathBuf};

use rrss_core::db;
use rrss_core::media::{MediaRow, cache_key};

use crate::views::MediaRef;

pub fn media_href(id: i64) -> String {
    media_href_with_fingerprint(id, "")
}

pub fn media_href_with_fingerprint(id: i64, fingerprint: &str) -> String {
    let query = if fingerprint.is_empty() {
        String::new()
    } else {
        format!("?v={fingerprint}")
    };
    // 协议来源随平台不同，见 `platform::Os::protocol_origin`。
    let origin = rrss_core::platform::Os::current().protocol_origin("rrss");
    format!("{origin}/media/{id}{query}")
}

/// 已缓存媒体给前端的引用：带内容指纹的地址；图片另附真实宽高。
pub fn cached_media_ref(row: &MediaRow, src: &str) -> MediaRef {
    let key = cache_key(&row.url);
    let fp = if key.len() >= 16 { &key[..16] } else { &key };
    let size = (row.kind == "image")
        .then(|| row.local_path.as_deref())
        .flatten()
        .and_then(|p| rrss_core::image_size::image_size(Path::new(p)));
    MediaRef {
        src: src.to_string(),
        media_id: Some(row.id),
        href: media_href_with_fingerprint(row.id, fp),
        width: size.map(|s| s.0),
        height: size.map(|s| s.1),
        error: String::new(),
    }
}

pub fn failed_media_ref(src: &str, error: &str) -> MediaRef {
    MediaRef {
        src: src.to_string(),
        media_id: None,
        href: String::new(),
        width: None,
        height: None,
        error: error.to_string(),
    }
}

pub fn media_id_from_path(path: &str) -> Option<i64> {
    let rest = path.trim_start_matches('/');
    let rest = rest.strip_prefix("localhost/").unwrap_or(rest);
    rest.strip_prefix("media/")
        .and_then(|s| s.split(['/', '?', '#']).next())
        .and_then(|s| s.parse().ok())
}

pub fn open_db(path: &Path) -> rrss_core::Result<rrss_core::db::PooledConnection> {
    db::open_at(path)
}

pub fn data_paths() -> anyhow::Result<(PathBuf, PathBuf)> {
    let data_dir = rrss_core::data_dir()?;
    std::fs::create_dir_all(&data_dir)?;
    let db_path = rrss_core::db_path()?;
    let _ = db::open_at(&db_path)?;
    Ok((data_dir, db_path))
}
