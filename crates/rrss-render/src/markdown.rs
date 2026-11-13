//! Markdown → 同一块 IR。模型输出当不可信，HTML 事件再走 ammonia。
//!
//! CommonMark：段内单换行是 SoftBreak（收成空格）；行尾两个空格或 `\` 才是 HardBreak。

use pulldown_cmark::{Alignment, CodeBlockKind, Event, Options, Parser, Tag, TagEnd};

use crate::html::{html_to_blocks, normalize_code_language};
use crate::ir::{Block, Inline};

pub fn markdown_to_blocks(md: &str) -> Vec<Block> {
    let parser = Parser::new_ext(md, Options::ENABLE_TABLES);
    let mut blocks = Vec::new();
    let mut inlines = Vec::new();
    let mut inline_stack: Vec<Inline> = Vec::new();
    let mut heading: Option<u8> = None;
    let mut list_ordered: Vec<bool> = Vec::new();
    let mut ol_next: Vec<u32> = Vec::new();
    let mut code = String::new();
    let mut code_lang = None;
    let mut in_quote = false;
    let mut in_code = false;
    let mut table: Option<crate::table::Table> = None;
    let mut row = Vec::new();

    for ev in parser {
        match ev {
            Event::Start(Tag::Table(align)) => {
                if list_ordered.is_empty() {
                    flush_p(&mut blocks, &mut inlines, in_quote);
                } else {
                    flush_list_item(&mut blocks, &mut inlines, &list_ordered, &mut ol_next);
                }
                table = Some(crate::table::Table {
                    align: align
                        .into_iter()
                        .map(|a| {
                            match a {
                                Alignment::Left => "l",
                                Alignment::Center => "c",
                                Alignment::Right => "r",
                                Alignment::None => "",
                            }
                            .into()
                        })
                        .collect(),
                    ..Default::default()
                });
            }
            Event::End(TagEnd::TableCell) => row.push(std::mem::take(&mut inlines)),
            Event::End(TagEnd::TableHead) => {
                if let Some(table) = &mut table {
                    table.head = std::mem::take(&mut row);
                }
            }
            Event::End(TagEnd::TableRow) => {
                if let Some(table) = &mut table {
                    table.rows.push(std::mem::take(&mut row));
                }
            }
            Event::End(TagEnd::Table) => {
                if let Some(table) = table.take() {
                    table.append_to(&mut blocks);
                }
            }
            Event::Start(Tag::Paragraph) => {}
            Event::End(TagEnd::Paragraph) => {
                if !list_ordered.is_empty() {
                    // 列表项里的段落留给 ListItem，不要先收成独立 p（否则父项结束时会再吐一个空 •）。
                    append_collapsed_space(&mut inlines);
                } else {
                    flush_p(&mut blocks, &mut inlines, in_quote);
                }
            }
            Event::Start(Tag::Heading { level, .. }) => {
                heading = Some(level as u8);
            }
            Event::End(TagEnd::Heading(_)) => {
                let level = heading.take().unwrap_or(2);
                let kids = std::mem::take(&mut inlines);
                blocks.push(Block::Heading {
                    level,
                    inlines: kids,
                });
            }
            Event::Start(Tag::List(start)) => {
                if !list_ordered.is_empty() {
                    flush_list_item(&mut blocks, &mut inlines, &list_ordered, &mut ol_next);
                }
                list_ordered.push(start.is_some());
                ol_next.push(start.unwrap_or(1) as u32);
            }
            Event::End(TagEnd::List(_)) => {
                list_ordered.pop();
                ol_next.pop();
            }
            Event::Start(Tag::Item) => {}
            Event::End(TagEnd::Item) => {
                flush_list_item(&mut blocks, &mut inlines, &list_ordered, &mut ol_next);
            }
            Event::Start(Tag::BlockQuote(_)) => in_quote = true,
            Event::End(TagEnd::BlockQuote(_)) => {
                flush_p(&mut blocks, &mut inlines, true);
                in_quote = false;
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                in_code = true;
                code.clear();
                code_lang = match kind {
                    CodeBlockKind::Fenced(info) => normalize_code_language(
                        info.split_whitespace()
                            .next()
                            .unwrap_or("")
                            .split(',')
                            .next()
                            .unwrap_or("")
                            .trim_matches(['{', '}', '.']),
                    ),
                    _ => None,
                };
            }
            Event::End(TagEnd::CodeBlock) => {
                in_code = false;
                blocks.push(Block::CodeBlock {
                    lang: code_lang.take(),
                    text: std::mem::take(&mut code),
                });
            }
            Event::Start(Tag::Emphasis) => inline_stack.push(Inline::Emphasis {
                children: Vec::new(),
            }),
            Event::End(TagEnd::Emphasis) => pop_wrap(&mut inlines, &mut inline_stack, "em"),
            Event::Start(Tag::Strong) => inline_stack.push(Inline::Strong {
                children: Vec::new(),
            }),
            Event::End(TagEnd::Strong) => pop_wrap(&mut inlines, &mut inline_stack, "strong"),
            Event::Start(Tag::Link { dest_url, .. }) => {
                inline_stack.push(Inline::Link {
                    href: dest_url.to_string(),
                    children: Vec::new(),
                });
            }
            Event::End(TagEnd::Link) => pop_wrap(&mut inlines, &mut inline_stack, "a"),
            Event::Text(t) => {
                if in_code {
                    code.push_str(&t);
                } else {
                    push_inline(&mut inlines, &mut inline_stack, Inline::text(t.to_string()));
                }
            }
            Event::Code(t) => {
                push_inline(
                    &mut inlines,
                    &mut inline_stack,
                    Inline::Code {
                        text: t.to_string(),
                    },
                );
            }
            Event::SoftBreak => {
                push_soft_break(&mut inlines, &mut inline_stack);
            }
            Event::HardBreak => {
                push_inline(&mut inlines, &mut inline_stack, Inline::HardBreak);
            }
            Event::Rule => blocks.push(Block::Rule),
            Event::Html(html) | Event::InlineHtml(html) => {
                let nested = html_to_blocks(&html, None);
                if table.is_some() {
                    for block in nested {
                        push_inline(
                            &mut inlines,
                            &mut inline_stack,
                            Inline::text(crate::block_plain(&block)),
                        );
                    }
                } else {
                    blocks.extend(nested);
                }
            }
            _ => {}
        }
    }
    flush_p(&mut blocks, &mut inlines, in_quote);
    blocks
}

fn flush_list_item(
    blocks: &mut Vec<Block>,
    inlines: &mut Vec<Inline>,
    list_ordered: &[bool],
    ol_next: &mut [u32],
) {
    if list_ordered.is_empty() {
        return;
    }
    let kids = std::mem::take(inlines);
    if inlines_empty(&kids) {
        return;
    }
    let indent = list_ordered.len().saturating_sub(1) as u8;
    let marker = if list_ordered.last().copied() == Some(true) {
        let n = ol_next.last_mut().map(|n| {
            let cur = *n;
            *n += 1;
            cur
        });
        format!("{}.", n.unwrap_or(1))
    } else {
        "•".into()
    };
    blocks.push(Block::ListItem {
        indent,
        marker,
        inlines: kids,
    });
}

fn inlines_empty(inlines: &[Inline]) -> bool {
    inlines.iter().all(|i| match i {
        Inline::Text { text } => text.trim().is_empty(),
        Inline::HardBreak => true,
        _ => false,
    })
}

fn flush_p(blocks: &mut Vec<Block>, inlines: &mut Vec<Inline>, quote: bool) {
    let kids = std::mem::take(inlines);
    if kids.is_empty() {
        return;
    }
    if quote {
        blocks.push(Block::Quote { inlines: kids });
    } else {
        blocks.push(Block::Paragraph { inlines: kids });
    }
}

fn push_inline(inlines: &mut Vec<Inline>, stack: &mut [Inline], piece: Inline) {
    inline_target(inlines, stack).push(piece);
}

fn push_soft_break(inlines: &mut Vec<Inline>, stack: &mut [Inline]) {
    append_collapsed_space(inline_target(inlines, stack));
}

fn inline_target<'a>(inlines: &'a mut Vec<Inline>, stack: &'a mut [Inline]) -> &'a mut Vec<Inline> {
    if let Some(
        Inline::Strong { children } | Inline::Emphasis { children } | Inline::Link { children, .. },
    ) = stack.last_mut()
    {
        children
    } else {
        inlines
    }
}

fn append_collapsed_space(target: &mut Vec<Inline>) {
    match target.last_mut() {
        Some(Inline::Text { text }) if text.ends_with(char::is_whitespace) => {}
        Some(Inline::Text { text }) => text.push(' '),
        Some(Inline::HardBreak) => {}
        Some(_) => target.push(Inline::text(" ")),
        None => {}
    }
}

fn pop_wrap(inlines: &mut Vec<Inline>, stack: &mut Vec<Inline>, _kind: &str) {
    if let Some(frame) = stack.pop() {
        if let Some(
            Inline::Strong { children }
            | Inline::Emphasis { children }
            | Inline::Link { children, .. },
        ) = stack.last_mut()
        {
            children.push(frame);
        } else {
            inlines.push(frame);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{Block, Inline};
    use crate::text::inlines_to_plain;

    fn has_hard_break(inlines: &[Inline]) -> bool {
        inlines.iter().any(|i| match i {
            Inline::HardBreak => true,
            Inline::Strong { children }
            | Inline::Emphasis { children }
            | Inline::Link { children, .. } => has_hard_break(children),
            _ => false,
        })
    }

    fn first_paragraph(md: &str) -> Vec<Inline> {
        markdown_to_blocks(md)
            .into_iter()
            .find_map(|b| match b {
                Block::Paragraph { inlines } => Some(inlines),
                _ => None,
            })
            .expect("paragraph")
    }

    #[test]
    fn fenced_code_languages() {
        let cases = [
            ("JS", Some("javascript")),
            ("ts title=demo", Some("typescript")),
            ("{.python}", Some("python")),
            ("rust,ignore", Some("rust")),
            ("py", Some("python")),
            ("sh", Some("bash")),
            ("shell", Some("bash")),
            ("zsh", Some("bash")),
            ("console", Some("bash")),
            ("rs", Some("rust")),
            ("yml", Some("yaml")),
            ("c++", Some("cpp")),
            ("cs", Some("csharp")),
            ("c#", Some("csharp")),
            ("golang", Some("go")),
            ("md", Some("markdown")),
            ("html", Some("html")),
            ("Custom-42+#", Some("custom-42+#")),
            ("", None),
            ("text", None),
            ("plain", None),
            ("plaintext", None),
            ("NONE", None),
            ("nohighlight", None),
            (
                "abcdefghijklmnopqrstuvwxyz123456",
                Some("abcdefghijklmnopqrstuvwxyz123456"),
            ),
            ("abcdefghijklmnopqrstuvwxyz1234567", None),
            ("bad_name", None),
            ("rüst", None),
        ];
        for (info, expected) in cases {
            assert_eq!(
                markdown_to_blocks(&format!("```{info}\nx\n```")),
                vec![Block::CodeBlock {
                    lang: expected.map(str::to_string),
                    text: "x\n".to_string(),
                }],
                "{info:?}",
            );
        }
    }

    #[test]
    fn soft_break_becomes_space() {
        let inlines = first_paragraph("Hello\nworld and *soft\nbreak*.");
        let plain = inlines_to_plain(&inlines);
        assert!(plain.contains("Hello world"), "{plain}");
        assert!(plain.contains("soft break"), "{plain}");
        assert!(!has_hard_break(&inlines), "{inlines:?}");
    }

    #[test]
    fn two_space_hard_break_kept() {
        let inlines = first_paragraph("上  \n下");
        assert!(has_hard_break(&inlines), "{inlines:?}");
        assert_eq!(inlines_to_plain(&inlines), "上 下");
    }

    #[test]
    fn backslash_hard_break_kept() {
        let inlines = first_paragraph("上\\\n下");
        assert!(has_hard_break(&inlines), "{inlines:?}");
        assert_eq!(inlines_to_plain(&inlines), "上 下");
    }

    #[test]
    fn nested_list_does_not_emit_empty_parent_item() {
        let md = "\
### AI 模型与智能体前沿
- **新一代高性价比模型发布**：
  - **GLM-5.3-Flash**：分析。
  - **Qwen3.8-Flash-Next**：成本效益。
- **WebMCP 框架**：标准交互。
";
        let blocks = markdown_to_blocks(md);
        let items: Vec<(u8, String)> = blocks
            .iter()
            .filter_map(|b| match b {
                Block::ListItem {
                    indent, inlines, ..
                } => Some((*indent, inlines_to_plain(inlines))),
                _ => None,
            })
            .collect();
        assert!(
            items.iter().all(|(_, t)| !t.trim().is_empty()),
            "empty list items: {items:?}"
        );
        assert!(
            items
                .iter()
                .any(|(indent, t)| *indent == 0 && t.contains("新一代高性价比模型发布")),
            "{items:?}"
        );
        assert!(
            items
                .iter()
                .any(|(indent, t)| *indent == 1 && t.contains("GLM-5.3-Flash")),
            "{items:?}"
        );
        assert!(
            items
                .iter()
                .any(|(indent, t)| *indent == 0 && t.contains("WebMCP")),
            "{items:?}"
        );
        assert_eq!(items.len(), 4, "{items:?}");
    }
}
