//! Block IR 加载：全文优先 `fulltext_md` / `blocks_json`。

use rrss_core::ArticleRow;
use rrss_render::Document;

/// 实际阅读输入版本；不使用馈源去重 hash，也不包含媒体下载 / 阅读状态。
/// 改变正文正规化或版本算法时提升前缀；翻译抽取版本另计入配置身份。
pub fn source_hash(title: &str, doc: &Document) -> String {
    rrss_core::db::latency::measure("reader_source_hash", || {
        let input = serde_json::to_string(&(title, doc)).expect("serialize typed document");
        format!("reader-v1:{}", rrss_core::ai_store::hash_input(&input))
    })
}

pub fn load_document(article: &ArticleRow) -> Document {
    rrss_core::body::article_document(article)
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
