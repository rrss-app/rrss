//! 从块 IR 分块，不是生 HTML。日志 target：`rrss_core::chunk`。

use sha2::{Digest, Sha256};

use rrss_render::{RenderOpts, document_from_json, document_to_plain, html_to_document};

use crate::ai_store::truncate_chars;

pub const TITLE_SUMMARY_KIND: &str = "title_summary";
pub const BODY_KIND: &str = "body";

const BODY_CHUNK_CHARS: usize = 700;
const BODY_OVERLAP_CHARS: usize = 80;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkDraft {
    pub ordinal: i64,
    pub kind: String,
    pub text: String,
    pub source_hash: String,
}

/// 标题 + 已有短摘要一条；正文按块纯文本切窗。
pub fn drafts_for_article(
    title: &str,
    summary: Option<&str>,
    blocks_json: Option<&str>,
    content_html: Option<&str>,
) -> Vec<ChunkDraft> {
    let mut out = Vec::new();
    let title_text = title_summary_text(title, summary);
    if !title_text.is_empty() {
        out.push(draft(0, TITLE_SUMMARY_KIND, title_text));
    }
    let body = body_plain(blocks_json, content_html);
    for (i, piece) in split_body(&body).into_iter().enumerate() {
        out.push(draft(i as i64 + 1, BODY_KIND, piece));
    }
    out
}

pub fn title_summary_text(title: &str, summary: Option<&str>) -> String {
    let title = title.trim();
    let summary = summary.unwrap_or("").trim();
    match (title.is_empty(), summary.is_empty()) {
        (true, true) => String::new(),
        (false, true) => title.to_string(),
        (true, false) => summary.to_string(),
        (false, false) => format!("{title}\n{summary}"),
    }
}

fn body_plain(blocks_json: Option<&str>, content_html: Option<&str>) -> String {
    if let Some(json) = blocks_json.map(str::trim).filter(|s| !s.is_empty())
        && let Ok(doc) = document_from_json(json)
    {
        let plain = document_to_plain(&doc);
        if !plain.trim().is_empty() {
            return plain;
        }
    }
    if let Some(html) = content_html.map(str::trim).filter(|s| !s.is_empty()) {
        let doc = html_to_document(html, &RenderOpts::default());
        return document_to_plain(&doc);
    }
    String::new()
}

fn split_body(text: &str) -> Vec<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    if char_len(trimmed) <= BODY_CHUNK_CHARS {
        return vec![trimmed.to_string()];
    }
    let paras: Vec<&str> = trimmed
        .split("\n\n")
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    let mut chunks = Vec::new();
    let mut current = String::new();
    for para in paras {
        if !current.is_empty() && char_len(&current) + 2 + char_len(para) > BODY_CHUNK_CHARS {
            chunks.push(current.clone());
            current = overlap_tail(&current);
            if !current.is_empty() {
                current.push_str("\n\n");
            }
        }
        if current.is_empty() || current.ends_with("\n\n") {
            current.push_str(para);
        } else {
            current.push_str("\n\n");
            current.push_str(para);
        }
        while char_len(&current) > BODY_CHUNK_CHARS {
            let head = truncate_chars(&current, BODY_CHUNK_CHARS);
            chunks.push(head.clone());
            let rest = skip_chars(&current, char_len(&head).saturating_sub(BODY_OVERLAP_CHARS));
            current = rest.trim_start().to_string();
        }
    }
    if !current.trim().is_empty() {
        chunks.push(current);
    }
    chunks
}

fn draft(ordinal: i64, kind: &str, text: String) -> ChunkDraft {
    let source_hash = hash_text(kind, &text);
    ChunkDraft {
        ordinal,
        kind: kind.into(),
        text,
        source_hash,
    }
}

pub fn hash_text(kind: &str, text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(kind.as_bytes());
    hasher.update(b"|");
    hasher.update(text.as_bytes());
    hasher
        .finalize()
        .iter()
        .fold(String::with_capacity(64), |mut s, b| {
            use std::fmt::Write;
            let _ = write!(s, "{b:02x}");
            s
        })
}

fn char_len(s: &str) -> usize {
    s.chars().count()
}

fn overlap_tail(s: &str) -> String {
    let total = char_len(s);
    if total <= BODY_OVERLAP_CHARS {
        return s.to_string();
    }
    skip_chars(s, total - BODY_OVERLAP_CHARS)
}

fn skip_chars(s: &str, n: usize) -> String {
    match s.char_indices().nth(n) {
        Some((i, _)) => s[i..].to_string(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rrss_render::{document_to_json, markdown_to_document};

    #[test]
    fn title_summary_is_ordinal_zero() {
        let drafts = drafts_for_article("Hello", Some("A short summary."), None, None);
        assert_eq!(drafts.len(), 1);
        assert_eq!(drafts[0].ordinal, 0);
        assert_eq!(drafts[0].kind, TITLE_SUMMARY_KIND);
        assert_eq!(drafts[0].text, "Hello\nA short summary.");
    }

    #[test]
    fn body_comes_from_blocks_not_html_tags() {
        let doc = markdown_to_document("# Head\n\nA **city** library opened.\n");
        let json = document_to_json(&doc).unwrap();
        let drafts = drafts_for_article(
            "Head",
            None,
            Some(&json),
            Some("<p>should <b>not</b> win</p>"),
        );
        assert_eq!(drafts[0].kind, TITLE_SUMMARY_KIND);
        assert!(
            drafts
                .iter()
                .any(|d| d.kind == BODY_KIND && d.text.contains("city library"))
        );
        assert!(drafts.iter().all(|d| !d.text.contains("<b>")));
    }

    #[test]
    fn long_body_splits_on_paragraphs() {
        let para = "字".repeat(400);
        let html = format!("<p>{para}</p><p>{para}</p><p>{para}</p>");
        let drafts = drafts_for_article("t", None, None, Some(&html));
        let body: Vec<_> = drafts.iter().filter(|d| d.kind == BODY_KIND).collect();
        assert!(body.len() >= 2);
        assert!(
            body.iter()
                .all(|d| char_len(&d.text) <= BODY_CHUNK_CHARS + 50)
        );
    }
}
