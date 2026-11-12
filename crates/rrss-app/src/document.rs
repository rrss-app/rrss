//! Block IR 加载：全文优先 `fulltext_md` / `blocks_json`。

use rrss_core::ArticleRow;
use rrss_render::{Document, IR_VERSION};

/// 实际阅读输入版本；不使用馈源去重 hash，也不包含媒体下载 / 阅读状态。
/// 改变正文正规化或版本算法时提升前缀；翻译抽取版本另计入配置身份。
pub fn source_hash(title: &str, doc: &Document) -> String {
    let input = serde_json::to_string(&(title, doc)).expect("serialize typed document");
    format!("reader-v1:{}", rrss_core::ai_store::hash_input(&input))
}

pub fn load_document(article: &ArticleRow) -> Document {
    if article
        .fulltext_md
        .as_ref()
        .is_some_and(|md| !md.trim().is_empty())
    {
        if let Some(json) = &article.blocks_json
            && let Ok(doc) = rrss_render::document_from_json(json)
            && doc.v == IR_VERSION
            && !doc.blocks.is_empty()
        {
            return doc;
        }
        return rrss_render::markdown_to_document(article.fulltext_md.as_deref().unwrap_or(""));
    }
    let html = article.content_html.as_deref().unwrap_or("");
    if !html.trim().is_empty() {
        let opts = rrss_render::RenderOpts {
            base_url: article
                .canonical_url
                .as_deref()
                .and_then(|u| u.parse().ok()),
        };
        return rrss_render::html_to_document(html, &opts);
    }
    if let Some(json) = &article.blocks_json
        && let Ok(doc) = rrss_render::document_from_json(json)
        && doc.v == IR_VERSION
    {
        return doc;
    }
    Document::new(vec![])
}

pub fn article_truncated(article: &ArticleRow) -> bool {
    if article
        .fulltext_md
        .as_ref()
        .is_some_and(|s| !s.trim().is_empty())
    {
        return false;
    }
    let url = article.canonical_url.as_deref().unwrap_or("");
    if !url.starts_with("http") {
        return false;
    }
    let html_len = article
        .content_html
        .as_deref()
        .map(|s| s.trim().len())
        .unwrap_or(0);
    html_len > 0 && html_len < 800
}
