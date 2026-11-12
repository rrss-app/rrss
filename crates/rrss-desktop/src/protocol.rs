use std::path::Path;

use rrss_core::db;
use rrss_core::media::{self, parse_byte_range, read_file_span};

pub struct MediaPayload {
    pub status: u16,
    pub body: Vec<u8>,
    pub mime: String,
    pub content_range: Option<String>,
    pub accept_ranges: bool,
}

pub fn media_payload(
    id: i64,
    db_path: &Path,
    range_header: Option<&str>,
) -> Result<MediaPayload, String> {
    let conn = db::open_at(db_path).map_err(|e| e.to_string())?;
    let row = media::get(&conn, id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "not found".to_string())?;
    let _ = media::touch(&conn, id);
    let path = row.local_path.filter(|p| !p.is_empty()).ok_or("no file")?;
    let path = Path::new(&path);
    use std::io::Read;
    let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    (&mut file)
        .take(media::MEDIA_HEAD_BYTES as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    let mime = media::served_mime(&row.kind, row.mime.as_deref(), &bytes);
    let streamed = row.kind == "video" || mime.starts_with("audio/") || mime.starts_with("video/");
    if streamed {
        return ranged_file(path, &mime, range_header);
    }
    file.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    Ok(MediaPayload {
        status: 200,
        body: bytes,
        mime,
        content_range: None,
        accept_ranges: false,
    })
}

fn ranged_file(
    path: &Path,
    mime: &str,
    range_header: Option<&str>,
) -> Result<MediaPayload, String> {
    let len = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
    let Some(header) = range_header.map(str::trim).filter(|h| !h.is_empty()) else {
        let body = std::fs::read(path).map_err(|e| e.to_string())?;
        return Ok(MediaPayload {
            status: 200,
            body,
            mime: mime.to_string(),
            content_range: None,
            accept_ranges: true,
        });
    };
    match parse_byte_range(header, len) {
        Ok((start, end)) => {
            let body = read_file_span(path, start, end).map_err(|e| e.to_string())?;
            Ok(MediaPayload {
                status: 206,
                content_range: Some(format!("bytes {start}-{end}/{len}")),
                body,
                mime: mime.to_string(),
                accept_ranges: true,
            })
        }
        Err(()) => Ok(MediaPayload {
            status: 416,
            body: Vec::new(),
            mime: mime.to_string(),
            content_range: Some(format!("bytes */{len}")),
            accept_ranges: true,
        }),
    }
}
