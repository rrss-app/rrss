//! `testdata/render/article.html` → 块 IR。不碰 UI、不下载。

use rrss_render::{Block, RenderOpts, html_to_document, inlines_to_markdown, inlines_to_plain};

const HTML: &str = include_str!("../../../testdata/render/article.html");

fn fixture_doc() -> rrss_render::Document {
    html_to_document(
        HTML,
        &RenderOpts {
            base_url: Some("https://fixture.example/article".parse().unwrap()),
        },
    )
}

#[test]
fn fixture_block_kinds() {
    let kinds: Vec<&'static str> = fixture_doc()
        .blocks
        .iter()
        .map(|b| match b {
            Block::Heading { .. } => "h",
            Block::Paragraph { .. } => "p",
            Block::ListItem { .. } => "li",
            Block::CodeBlock { .. } => "pre",
            Block::Image { .. } => "img",
            Block::Quote { .. } => "q",
            Block::Embed { .. } => "embed",
            Block::Rule => "hr",
            Block::Table { .. } => "table",
        })
        .collect();
    assert_eq!(
        kinds,
        ["h", "p", "p", "img", "li", "li", "q", "pre", "p"],
        "visual twin: apps/desktop LIBRARY_FIXTURE / /#render"
    );
}

#[test]
fn fixture_heading_and_inlines() {
    let doc = fixture_doc();
    let Block::Heading { level, inlines } = &doc.blocks[0] else {
        panic!("expected heading");
    };
    assert_eq!(*level, 2);
    assert_eq!(inlines_to_plain(inlines), "开馆日");

    let Block::Paragraph { inlines } = &doc.blocks[1] else {
        panic!("expected intro paragraph");
    };
    let md = inlines_to_markdown(inlines);
    assert!(md.contains("**9–21**"), "{md}");
    assert!(md.contains("*故事会*"), "{md}");
    assert!(
        md.contains("[时刻表](https://library.example/hours)"),
        "{md}"
    );
}

#[test]
fn fixture_keeps_emoji_in_paragraph() {
    let doc = fixture_doc();
    let emoji_para = doc.blocks.iter().find_map(|b| match b {
        Block::Paragraph { inlines } => {
            let plain = inlines_to_plain(inlines);
            plain.contains('📚').then_some(plain)
        }
        _ => None,
    });
    let plain = emoji_para.expect("paragraph with 📚");
    assert!(plain.contains('🎉'), "{plain}");
    let last = doc.blocks.last().expect("last block");
    let Block::Paragraph { inlines } = last else {
        panic!("last block should be the glyph probe paragraph");
    };
    let tail = inlines_to_plain(inlines);
    assert!(tail.contains('⭐'), "{tail}");
    assert!(tail.contains("ABC"), "{tail}");
}

#[test]
fn fixture_image_rewrites_relative_src() {
    let doc = fixture_doc();
    let img = doc
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Image { src, alt } => Some((src.as_str(), alt.as_str())),
            _ => None,
        })
        .expect("image block");
    assert_eq!(img.0, "https://fixture.example/cover.png");
    assert_eq!(img.1, "馆舍外观");
}

#[test]
fn fixture_relative_img_dropped_without_base() {
    let doc = html_to_document(HTML, &RenderOpts::default());
    assert!(
        doc.blocks.iter().all(|b| !matches!(b, Block::Image { .. })),
        "ammonia denies relative src without base_url; image must not appear"
    );
}
