//! HTML / Markdown → 块 IR。无 UI 依赖。日志 target：`rrss_render`。

mod embed;
mod html;
mod ir;
mod markdown;
mod sanitize;
mod table;
mod text;
mod translate;

pub use embed::{DirectMedia, EmbedClass, PresentedEmbed, classify_for_read, present_embed};
pub use ir::{Block, Document, EmbedKind, IR_VERSION, Inline};
pub use text::{inlines_to_markdown, inlines_to_plain};
pub use translate::{
    PatchError, TranslateUnit, TranslationPatch, TranslationPlan, already_target_lang, apply_units,
    extract_units, units_plain,
};

use thiserror::Error;
use url::Url;

#[derive(Debug, Error)]
pub enum Error {
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, Default)]
pub struct RenderOpts {
    pub base_url: Option<Url>,
}

pub fn html_to_document(html: &str, opts: &RenderOpts) -> Document {
    let blocks = html::html_to_blocks(html, opts.base_url.as_ref());
    Document::new(blocks)
}

pub fn markdown_to_document(md: &str) -> Document {
    Document::new(markdown::markdown_to_blocks(md))
}

pub fn document_to_json(doc: &Document) -> Result<String> {
    Ok(serde_json::to_string(doc)?)
}

pub fn document_from_json(json: &str) -> Result<Document> {
    Ok(serde_json::from_str(json)?)
}

/// 块序列纯文本，供分块 / 嵌入。跳过水平线。
pub fn document_to_plain(doc: &Document) -> String {
    let mut parts = Vec::new();
    for block in &doc.blocks {
        let text = block_plain(block);
        if !text.trim().is_empty() {
            parts.push(text);
        }
    }
    parts.join("\n\n")
}

/// 单块可见纯文本；表格保留单元格与行的分隔，供检索和摘要共用。
pub fn block_plain(block: &Block) -> String {
    match block {
        Block::Heading { inlines, .. }
        | Block::Paragraph { inlines }
        | Block::ListItem { inlines, .. }
        | Block::Quote { inlines } => inlines_to_plain(inlines),
        Block::Table { head, rows, .. } => std::iter::once(head)
            .chain(rows)
            .map(|row| {
                row.iter()
                    .map(|cell| inlines_to_plain(cell))
                    .collect::<Vec<_>>()
                    .join("\t")
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Block::CodeBlock { text, .. } => text.clone(),
        Block::Image { alt, .. } => alt.clone(),
        Block::Embed { title, .. } => title.clone(),
        Block::Rule => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heading_anchor_links_are_not_text() {
        let base = RenderOpts {
            base_url: Some("https://ex.test/post".parse().unwrap()),
        };
        let heading = |html: &str, opts: &RenderOpts| match &html_to_document(html, opts).blocks[0]
        {
            Block::Heading { inlines, .. } => inlines.clone(),
            other => panic!("expected heading, got {other:?}"),
        };
        for (html, opts) in [
            ("<h2>Docker <a href=\"#docker\">#</a></h2>", &base),
            (
                "<h2>Docker <a href=\"#docker\">⌗</a></h2>",
                &RenderOpts::default(),
            ),
            (
                "<h2>Docker<a class=\"anchor\" href=\"#docker\" aria-hidden=\"true\"><svg></svg></a></h2>",
                &base,
            ),
            ("<h2><a href=\"#docker\">¶</a> Docker</h2>", &base),
        ] {
            assert_eq!(inlines_to_plain(&heading(html, opts)), "Docker", "{html}");
        }
        // Real links and text that merely contains # stay.
        let kept = heading(
            "<h2>Use <a href=\"https://ex.test/c\">C#</a> here</h2>",
            &base,
        );
        assert!(kept.iter().any(|i| matches!(i, Inline::Link { .. })));
        assert_eq!(inlines_to_plain(&kept), "Use C# here");
        assert_eq!(
            inlines_to_plain(&heading("<h2>Issue #42</h2>", &base)),
            "Issue #42"
        );
    }

    #[test]
    fn html_headings_and_paragraph() {
        let doc = html_to_document(
            "<h1>Hello</h1><p>A <em>city</em> and <a href=\"https://ex.test/\">link</a>.</p>",
            &RenderOpts::default(),
        );
        assert!(matches!(doc.blocks[0], Block::Heading { level: 1, .. }));
        assert!(matches!(doc.blocks[1], Block::Paragraph { .. }));
        let json = document_to_json(&doc).unwrap();
        let back = document_from_json(&json).unwrap();
        assert_eq!(doc, back);
    }

    #[test]
    fn html_list_and_pre() {
        let doc = html_to_document(
            "<ul><li>one</li><li>two</li></ul><pre><code>x=1</code></pre>",
            &RenderOpts::default(),
        );
        assert!(
            doc.blocks
                .iter()
                .any(|b| matches!(b, Block::ListItem { .. }))
        );
        assert!(
            doc.blocks
                .iter()
                .any(|b| matches!(b, Block::CodeBlock { .. }))
        );
    }

    #[test]
    fn html_strips_script_and_keeps_embed() {
        let doc = html_to_document(
            r#"<p>hi</p><script>alert(1)</script><iframe src="https://www.youtube.com/embed/abc" title="t"></iframe>"#,
            &RenderOpts::default(),
        );
        assert!(doc.blocks.iter().any(|b| matches!(b, Block::Embed { .. })));
        let json = document_to_json(&doc).unwrap();
        assert!(!json.contains("alert"));
    }

    #[test]
    fn markdown_basic() {
        let doc = markdown_to_document("# Title\n\nHello *world* and `code`.\n\n- a\n- b\n");
        assert!(matches!(doc.blocks[0], Block::Heading { level: 1, .. }));
        assert!(
            doc.blocks
                .iter()
                .any(|b| matches!(b, Block::ListItem { .. }))
        );
        let plain = document_to_plain(&doc);
        assert!(plain.contains("Title"));
        assert!(plain.contains("Hello world"));
        assert!(!plain.contains('*'));
    }

    #[test]
    fn html_image_rewrites_relative_src() {
        let doc = html_to_document(
            r#"<p>hi</p><img src="/pic.png" alt="cover">"#,
            &RenderOpts {
                base_url: Some("https://ex.test/blog/post".parse().unwrap()),
            },
        );
        let img = doc
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Image { src, alt } => Some((src.as_str(), alt.as_str())),
                _ => None,
            })
            .expect("image block");
        assert_eq!(img.0, "https://ex.test/pic.png");
        assert_eq!(img.1, "cover");
    }

    #[test]
    fn html_div_and_double_br_split_paragraphs() {
        let doc = html_to_document(
            include_str!("../../../testdata/render/rsshub-div-br.html"),
            &RenderOpts::default(),
        );
        let plains: Vec<String> = doc
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Paragraph { inlines } => Some(inlines_to_plain(inlines)),
                _ => None,
            })
            .collect();
        assert!(
            plains.iter().any(|p| p.contains("第一段内容")),
            "{plains:?}"
        );
        assert!(
            plains.iter().any(|p| p.contains("第二段内容")),
            "{plains:?}"
        );
        assert!(
            plains.iter().any(|p| p.contains("第三段在双 br 之后")),
            "{plains:?}"
        );
        assert!(plains.iter().any(|p| p.contains("第四段")), "{plains:?}");
        assert!(plains.len() >= 4, "expected div/br splits, got {plains:?}");
        assert!(
            !plains
                .iter()
                .any(|p| p.contains("第一段内容") && p.contains("第二段内容")),
            "divs must not concatenate: {plains:?}"
        );
        assert!(
            !plains
                .iter()
                .any(|p| p.contains("带换行的纯文本") && p.contains("第二行之后应成新段")),
            "blank line inside <p> splits: {plains:?}"
        );
        let wrap = plains
            .iter()
            .find(|p| p.contains("源码折行"))
            .expect("source-wrap paragraph");
        assert!(wrap.contains("不应变成 br"), "{plains:?}");
        let wrap_has_br = doc.blocks.iter().any(|b| match b {
            Block::Paragraph { inlines } => {
                inlines_to_plain(inlines).contains("源码折行")
                    && inlines
                        .iter()
                        .any(|n| matches!(n, crate::Inline::HardBreak))
            }
            _ => false,
        });
        assert!(!wrap_has_br, "single \\n inside <p> must not be <br>");
    }

    #[test]
    fn html_blank_lines_split_when_no_block_tags() {
        let doc = html_to_document("第一段\n\n第二段\n第三行", &RenderOpts::default());
        let plains: Vec<String> = doc
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Paragraph { inlines } => Some(inlines_to_plain(inlines)),
                _ => None,
            })
            .collect();
        assert!(
            plains.len() >= 2,
            "\\n\\n should start a paragraph: {plains:?}"
        );
        assert!(plains[0].contains("第一段"), "{plains:?}");
        assert!(plains.iter().any(|p| p.contains("第二段")), "{plains:?}");
        let second = doc.blocks.iter().find_map(|b| match b {
            Block::Paragraph { inlines } if inlines_to_plain(inlines).contains("第二段") => {
                Some(inlines)
            }
            _ => None,
        });
        let second = second.expect("second paragraph");
        assert!(
            second.iter().any(|n| matches!(n, Inline::HardBreak)),
            "untagged single \\n is HardBreak: {second:?}"
        );
    }

    #[test]
    fn html_nested_ol_ul_keeps_markers_and_indent() {
        let doc = html_to_document(
            include_str!("../../../testdata/render/v2ex-nested-ol.html"),
            &RenderOpts::default(),
        );
        let items: Vec<(u8, String, String)> = doc
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::ListItem {
                    indent,
                    marker,
                    inlines,
                } => Some((*indent, marker.clone(), inlines_to_plain(inlines))),
                _ => None,
            })
            .collect();
        assert!(
            items.iter().all(|(_, _, t)| !t.trim().is_empty()),
            "no empty list items: {items:?}"
        );
        let top: Vec<_> = items
            .iter()
            .filter(|(indent, marker, _)| *indent == 0 && marker.ends_with('.'))
            .collect();
        assert_eq!(top.len(), 2, "two top-level numbered items: {items:?}");
        assert_eq!(top[0].1, "1.");
        assert!(top[0].2.contains("先完成医保异地长期备案"), "{top:?}");
        assert_eq!(top[1].1, "2.");
        assert!(top[1].2.contains("申请门特"), "{top:?}");
        assert!(
            items.iter().any(|(indent, marker, text)| *indent == 1
                && marker == "•"
                && text.contains("不会同步到国家")),
            "{items:?}"
        );
        assert!(
            items.iter().any(|(indent, marker, text)| *indent == 2
                && marker == "•"
                && text.contains("临时备案不需要任何资料")),
            "{items:?}"
        );
        assert!(
            !items
                .iter()
                .any(|(_, marker, _)| marker == "6." || marker == "7."),
            "nested ul must not steal ol numbering: {items:?}"
        );
    }
}
