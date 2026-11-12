//! 截断源抓取全文：浏览器 UA、WAF 检测、ammonia + Block IR。

use std::path::Path;
use std::time::Duration;

use reqwest::header::REFERER;

use rrss_core::articles;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, ts_rs::TS)]
pub enum FulltextErrorKind {
    Waf,
    Network,
    Empty,
    NotArticle,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, ts_rs::TS)]
pub struct FulltextError {
    pub kind: FulltextErrorKind,
    pub message: String,
}

impl FulltextError {
    pub(crate) fn new(kind: FulltextErrorKind, message: impl std::fmt::Display) -> Self {
        Self {
            kind,
            message: message.to_string(),
        }
    }

    pub(crate) fn network(error: impl std::fmt::Display) -> Self {
        Self::new(FulltextErrorKind::Network, error)
    }

    pub fn user_message(&self) -> &'static str {
        match self.kind {
            FulltextErrorKind::Waf => "站点做了访问防护，无法抓取全文",
            FulltextErrorKind::Network => "打不开原文站点",
            FulltextErrorKind::Empty => "没提取到正文",
            FulltextErrorKind::NotArticle => "这个页面不是文章",
        }
    }
}

impl std::fmt::Display for FulltextError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.user_message())
    }
}

impl std::error::Error for FulltextError {}

impl From<crate::extract::ExtractError> for FulltextError {
    fn from(error: crate::extract::ExtractError) -> Self {
        use crate::extract::ExtractError;
        let kind = match error {
            ExtractError::Empty => FulltextErrorKind::Empty,
            ExtractError::NotArticle => FulltextErrorKind::NotArticle,
        };
        Self::new(kind, error)
    }
}

pub async fn fetch_and_store(
    network: &crate::network::NetworkClients,
    db_path: &Path,
    id: i64,
) -> Result<(), FulltextError> {
    let db_path_buf = db_path.to_path_buf();
    let path = db_path_buf.clone();
    let url = tokio::task::spawn_blocking(move || {
        let conn = rrss_core::db::open_at(&path).map_err(FulltextError::network)?;
        let article = articles::get_article(&conn, id).map_err(FulltextError::network)?;
        article
            .canonical_url
            .filter(|u| u.starts_with("http"))
            .ok_or_else(|| FulltextError::new(FulltextErrorKind::Empty, "no url"))
    })
    .await
    .map_err(FulltextError::network)??;

    let capacity = crate::scheduler::model_capacity::Capacity::for_database(db_path);
    let permit = capacity
        .content
        .clone()
        .acquire(crate::scheduler::model_capacity::Priority::Interactive)
        .await
        .map_err(FulltextError::network)?;
    let html = fetch_page(network, &url).await?;
    drop(permit);

    let page_url: url::Url = url
        .parse()
        .map_err(|e: url::ParseError| FulltextError::new(FulltextErrorKind::Empty, e))?;

    // Keep extraction, serialization and SQLite work off the async worker thread.
    tokio::task::spawn_blocking(move || {
        let conn = rrss_core::db::open_at(&db_path_buf).map_err(FulltextError::network)?;
        let rule = crate::site_rules::resolve_site_rule(
            &conn,
            page_url.host_str().unwrap_or_default(),
        )
        .map_err(FulltextError::network)?;
        let extracted = crate::extract::extract_article(&html, &page_url, rule.rule.as_ref())?;
        tracing::debug!(layer = ?extracted.layer, suspicious = ?extracted.suspicious, "fulltext extraction");
        let doc = extracted.document;
        let plain = rrss_render::document_to_plain(&doc);
        let blocks_json = rrss_render::document_to_json(&doc)
            .map_err(|e| FulltextError::new(FulltextErrorKind::Empty, e))?;
        articles::set_fulltext(&conn, id, &plain, &blocks_json).map_err(FulltextError::network)
    })
    .await
    .map_err(FulltextError::network)?
}

// Limit decompressed bytes too: Content-Length alone is insufficient for gzip/chunked pages.
const MAX_HTML_BYTES: usize = 5 * 1024 * 1024;
const CHARSET_SCAN_BYTES: usize = 1024;

async fn fetch_page(
    network: &crate::network::NetworkClients,
    url: &str,
) -> Result<String, FulltextError> {
    Ok(fetch_page_with_url(network, url).await?.0)
}

pub(crate) async fn fetch_page_with_url(
    network: &crate::network::NetworkClients,
    url: &str,
) -> Result<(String, url::Url), FulltextError> {
    let client = network
        .client(rrss_core::network::NetworkScope::Content)
        .await
        .map_err(FulltextError::network)?;
    let mut resp = client
        .get(url)
        .header(reqwest::header::USER_AGENT, crate::image_fetch::IMAGE_UA)
        .header(REFERER, url)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| FulltextError::network(rrss_core::network::request_error(&e)))?;

    let final_url = resp.url().clone();
    let status = resp.status().as_u16();
    // 不让代理认证响应作为文章正文存入数据库。
    if status == 407 {
        return Err(FulltextError::network("HTTP 407：代理认证失败"));
    }
    if status == 403 || status == 429 || status == 503 {
        return Err(FulltextError::new(
            FulltextErrorKind::Waf,
            format!("http {status}"),
        ));
    }

    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_owned();
    if resp
        .content_length()
        .is_some_and(|len| len > MAX_HTML_BYTES as u64)
    {
        return Err(page_too_large());
    }
    let mut body = Vec::new();
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| FulltextError::network(rrss_core::network::request_error(&e)))?
    {
        if chunk.len() > MAX_HTML_BYTES - body.len() {
            return Err(page_too_large());
        }
        body.extend_from_slice(&chunk);
    }
    let html = decode_page(&body, &content_type);
    if detect_waf(&html, status) {
        return Err(FulltextError::new(FulltextErrorKind::Waf, "challenge page"));
    }

    Ok((html, final_url))
}

fn page_too_large() -> FulltextError {
    FulltextError::network("页面过大，超过 5 MiB 下载上限")
}

fn charset_parameter(value: &str) -> Option<&'static encoding_rs::Encoding> {
    value.split(';').find_map(|parameter| {
        let (name, label) = parameter.trim().split_once('=')?;
        name.trim()
            .eq_ignore_ascii_case("charset")
            .then(|| {
                encoding_rs::Encoding::for_label(label.trim().trim_matches(['\'', '"']).as_bytes())
            })
            .flatten()
    })
}

pub(crate) fn decode_page(bytes: &[u8], content_type: &str) -> String {
    let encoding = charset_parameter(content_type)
        .or_else(|| {
            // Only the ASCII declaration syntax matters before decoding the document.
            let prefix = String::from_utf8_lossy(&bytes[..bytes.len().min(CHARSET_SCAN_BYTES)]);
            let dom = dom_query::Document::from(prefix.as_ref());
            dom.select("meta").iter().find_map(|meta| {
                if let Some(encoding) = meta
                    .attr("charset")
                    .and_then(|label| encoding_rs::Encoding::for_label(label.as_bytes()))
                {
                    return Some(encoding);
                }
                if meta
                    .attr("http-equiv")
                    .is_some_and(|v| v.eq_ignore_ascii_case("content-type"))
                {
                    return meta
                        .attr("content")
                        .and_then(|value| charset_parameter(&value));
                }
                None
            })
        })
        .unwrap_or(encoding_rs::UTF_8);
    // A BOM overrides any declaration, as in the HTML spec; `decode` also strips it.
    encoding.decode(bytes).0.into_owned()
}

pub(crate) fn detect_waf(html: &str, status: u16) -> bool {
    let lower = html.to_lowercase();
    if lower.contains("just a moment")
        || lower.contains("attention required")
        || lower.contains("cf-browser-verification")
        || (lower.contains("cloudflare") && lower.contains("ray id"))
    {
        return true;
    }
    status == 403 || status == 429
}

#[cfg(test)]
mod tests;
