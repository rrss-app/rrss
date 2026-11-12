//! 内联 → CommonMark / 纯文本。纯文本供 RAG 分块与 Simhash，必须保留。

use crate::ir::Inline;

/// 转成段落级 CommonMark。前端渲染走 Block IR，不依赖这份 markdown。
pub fn inlines_to_markdown(inlines: &[Inline]) -> String {
    let mut out = String::new();
    write_markdown(inlines, &mut out);
    out
}

/// 可见纯文本。链接取显示标签。RAG / Simhash 用 [`inlines_to_plain`]。
pub fn inlines_to_plain(inlines: &[Inline]) -> String {
    let mut out = String::new();
    write_plain(inlines, &mut out);
    out
}

fn trailing_url_punct(c: char) -> bool {
    matches!(
        c,
        '.' | ','
            | ';'
            | ':'
            | '!'
            | '?'
            | ')'
            | ']'
            | '}'
            | '"'
            | '\''
            | '。'
            | '，'
            | '、'
            | '；'
            | '：'
            | '！'
            | '？'
            | '）'
            | '」'
            | '』'
            | '》'
    )
}

fn trim_url_end(text: &str, start: usize) -> usize {
    let mut end = text[start..]
        .find(|c: char| c.is_whitespace())
        .map(|i| start + i)
        .unwrap_or(text.len());
    while end > start {
        let slice = &text[start..end];
        let Some(last) = slice.chars().last() else {
            break;
        };
        if trailing_url_punct(last) {
            end -= last.len_utf8();
        } else {
            break;
        }
    }
    end
}

fn link_label(url: &str) -> String {
    match url::Url::parse(url) {
        Ok(parsed) => parsed
            .host_str()
            .map(|host| host.to_string())
            .unwrap_or_else(|| "链接".into()),
        Err(_) => "链接".into(),
    }
}

fn url_like_label(label: &str) -> bool {
    label.starts_with("http://") || label.starts_with("https://")
}

fn link_display(label: &str, href: &str) -> String {
    if label == href || url_like_label(label) {
        link_label(href)
    } else {
        label.to_string()
    }
}

fn parse_inline_md_link(rest: &str) -> Option<(String, String, usize)> {
    if !rest.starts_with('[') {
        return None;
    }
    let close = rest.find(']')?;
    if rest.as_bytes().get(close + 1) != Some(&b'(') {
        return None;
    }
    let href_start = close + 2;
    let href_end = rest[href_start..].find(')')? + href_start;
    let label = rest[1..close].to_string();
    let href = rest[href_start..href_end].to_string();
    if label.is_empty() || href.is_empty() {
        return None;
    }
    Some((label, href, href_end + 1))
}

fn push_md_text(out: &mut String, text: &str) {
    out.push_str(&escape_md(text));
}

fn push_md_link(out: &mut String, label: &str, href: &str) {
    let display = link_display(label, href);
    out.push('[');
    out.push_str(&escape_md(&display));
    out.push_str("](");
    out.push_str(href);
    out.push(')');
}

fn write_autolinked_markdown(out: &mut String, text: &str) {
    let mut i = 0usize;
    while i < text.len() {
        if text[i..].starts_with('[')
            && let Some((label, href, consumed)) = parse_inline_md_link(&text[i..])
        {
            push_md_link(out, &label, &href);
            i += consumed;
            continue;
        }
        let rest = &text[i..];
        let rel = rest
            .find("https://")
            .or_else(|| rest.find("http://"))
            .unwrap_or(rest.len());
        if rel == rest.len() {
            push_md_text(out, &text[i..]);
            break;
        }
        if rel > 0 {
            push_md_text(out, &text[i..i + rel]);
        }
        let url_start = i + rel;
        let url_end = trim_url_end(text, url_start);
        if url_end <= url_start + 7 {
            push_md_text(out, &text[url_start..]);
            break;
        }
        let url = &text[url_start..url_end];
        push_md_link(out, url, url);
        i = url_end;
    }
}

fn write_markdown(inlines: &[Inline], out: &mut String) {
    for inline in inlines {
        match inline {
            Inline::Text { text } => write_autolinked_markdown(out, text),
            Inline::Strong { children } => {
                out.push_str("**");
                write_markdown(children, out);
                out.push_str("**");
            }
            Inline::Emphasis { children } => {
                out.push('*');
                write_markdown(children, out);
                out.push('*');
            }
            Inline::Code { text } => {
                let text = text.replace('`', "'");
                out.push('`');
                out.push_str(&text);
                out.push('`');
            }
            Inline::Link { href, children } => {
                let label = inlines_to_plain(children);
                push_md_link(out, &label, href);
            }
            Inline::HardBreak => {
                // CommonMark hard line break; React 侧用 <br> 保留同一语义。
                out.push_str("  \n");
            }
        }
    }
}

fn write_plain(inlines: &[Inline], out: &mut String) {
    for inline in inlines {
        match inline {
            Inline::Text { text } => out.push_str(text),
            Inline::Strong { children }
            | Inline::Emphasis { children }
            | Inline::Link { children, .. } => write_plain(children, out),
            Inline::Code { text } => out.push_str(text),
            // 纯文本给分块 / Simhash，硬换行收成空格以免切碎词。
            Inline::HardBreak => out.push(' '),
        }
    }
}

fn escape_md(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '*' | '_' | '`' | '[' | ']' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Inline;

    #[test]
    fn autolinks_bare_url_in_text() {
        let inlines = [Inline::Text {
            text: "项目地址：https://geomock.com/zh".into(),
        }];
        let md = inlines_to_markdown(&inlines);
        assert_eq!(md, "项目地址：[geomock.com](https://geomock.com/zh)");
    }

    #[test]
    fn autolink_strips_trailing_chinese_punctuation() {
        let inlines = [Inline::Text {
            text: "见 https://geomock.com/zh。".into(),
        }];
        let md = inlines_to_markdown(&inlines);
        assert_eq!(md, "见 [geomock.com](https://geomock.com/zh)。");
    }

    #[test]
    fn markdown_uses_emphasis_markers_not_font_tags() {
        let inlines = [
            Inline::Strong {
                children: vec![Inline::Text {
                    text: "重点".into(),
                }],
            },
            Inline::Code {
                text: "cargo test".into(),
            },
        ];
        let md = inlines_to_markdown(&inlines);
        assert_eq!(md, "**重点**`cargo test`");
        assert!(!md.contains("<font"));
        assert_eq!(inlines_to_plain(&inlines), "重点cargo test");
    }

    #[test]
    fn hard_break_is_markdown_line_break() {
        let inlines = [
            Inline::Text { text: "上".into() },
            Inline::HardBreak,
            Inline::Text { text: "下".into() },
        ];
        assert_eq!(inlines_to_markdown(&inlines), "上  \n下");
        assert_eq!(inlines_to_plain(&inlines), "上 下");
    }

    #[test]
    fn escapes_markdown_specials() {
        let inlines = [Inline::Text {
            text: "用 *星号* 和 [括号]".into(),
        }];
        let md = inlines_to_markdown(&inlines);
        assert_eq!(md, "用 \\*星号\\* 和 \\[括号\\]");
        assert_eq!(inlines_to_plain(&inlines), "用 *星号* 和 [括号]");
    }

    #[test]
    fn normalizes_existing_markdown_url_link() {
        let url = "https://9to5mac.com/2026/08/24/report-new-mac-mini";
        let inlines = [Inline::Text {
            text: format!("[{url}]({url})"),
        }];
        let md = inlines_to_markdown(&inlines);
        assert_eq!(
            md,
            "[9to5mac.com](https://9to5mac.com/2026/08/24/report-new-mac-mini)"
        );
    }
}
