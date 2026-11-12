//! 消毒后的 HTML → 块。日志 target：`rrss_render::html`。
//!
//! 换行规则（阶段 6 / 12d，按上下文分流，禁止把源码折行当成 `<br>`）：
//!
//! | 上下文 | 单个 `\n` | 连续 `\n\n` | `<br>` | `<br><br>` |
//! | --- | --- | --- | --- | --- |
//! | `<pre>` | 字面保留 | 字面保留 | （进代码块文本） | （进代码块文本） |
//! | 已在 `<p>` / `<li>` / 标题 / 引用 / `<div>` | HTML 空白 → 空格 | 脏源段界，拆段 | `HardBreak` | 拆段 |
//! | 几乎无块级标签的纯文本 | `HardBreak` | 拆段 | `HardBreak` | 拆段 |
//!
//! LWN 等博客在 `<p>` 里 80 列折行属于第一类；V2EX / RSSHub 无标签 blob 属于第三类。

use url::Url;

use crate::ir::{Block, Inline};
use crate::sanitize::clean_html;

pub fn html_to_blocks(html: &str, base: Option<&Url>) -> Vec<Block> {
    let (html, embeds) = crate::embed::excise_embeds(html, base);
    let cleaned = clean_html(&html, base);
    let mut blocks = walk(&cleaned, &table_candidates(&html));
    if blocks.is_empty() && !cleaned.trim().is_empty() {
        blocks.push(Block::Paragraph {
            inlines: vec![Inline::text(strip_tags(&cleaned))],
        });
    }
    splice_embed_slots(&mut blocks, &embeds);
    tracing::debug!(blocks = blocks.len(), "html walked");
    blocks
}

const EMBED_SLOT_PREFIX: &str = "RRSS_EMBED_SLOT_";

fn splice_embed_slots(blocks: &mut Vec<Block>, embeds: &[Block]) {
    let mut out = Vec::with_capacity(blocks.len());
    for block in blocks.drain(..) {
        let Block::Paragraph { ref inlines } = block else {
            out.push(block);
            continue;
        };
        let text = crate::inlines_to_plain(inlines);
        let Some(rest) = text.trim().strip_prefix(EMBED_SLOT_PREFIX) else {
            out.push(block);
            continue;
        };
        if let Ok(index) = rest.parse::<usize>()
            && let Some(embed) = embeds.get(index)
        {
            out.push(embed.clone());
        }
    }
    *blocks = out;
}

fn strip_tags(html: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    html_unescape(&out)
}

fn walk(html: &str, table_candidates: &[bool]) -> Vec<Block> {
    let mut p = Parser {
        input: html,
        pos: 0,
        blocks: Vec::new(),
        inlines: Vec::new(),
        inline_kind: Vec::new(),
        list_kind: Vec::new(),
        in_pre: false,
        pre_buf: String::new(),
        heading: None,
        in_quote: false,
        in_p: false,
        div_depth: 0,
        table_depth: 0,
        table_candidates: table_candidates.iter(),
    };
    p.parse();
    p.flush_pending_list_item();
    p.flush_paragraph();
    p.blocks
}

enum ListKind {
    Ul,
    Ol { next: u32 },
}

struct Parser<'a> {
    input: &'a str,
    pos: usize,
    blocks: Vec<Block>,
    inlines: Vec<Inline>,
    inline_kind: Vec<&'static str>,
    list_kind: Vec<ListKind>,
    in_pre: bool,
    pre_buf: String,
    heading: Option<u8>,
    in_quote: bool,
    in_p: bool,
    div_depth: u32,
    table_depth: usize,
    table_candidates: std::slice::Iter<'a, bool>,
}

impl<'a> Parser<'a> {
    fn parse(&mut self) {
        while self.pos < self.input.len() {
            if self.input.as_bytes()[self.pos] == b'<' {
                self.parse_tag();
            } else {
                let start = self.pos;
                while self.pos < self.input.len() && self.input.as_bytes()[self.pos] != b'<' {
                    self.pos += 1;
                }
                let text = html_unescape(&self.input[start..self.pos]);
                if self.in_pre {
                    self.pre_buf.push_str(&text);
                } else if !text.is_empty() {
                    self.push_text(text);
                }
            }
        }
    }

    fn parse_tag(&mut self) {
        let rest = &self.input[self.pos..];
        let Some(end) = rest.find('>') else {
            self.pos = self.input.len();
            return;
        };
        let raw = &rest[..=end];
        self.pos += raw.len();
        let inner = raw.trim_start_matches('<').trim_end_matches('>').trim();
        let closing = inner.starts_with('/');
        let name = inner
            .trim_start_matches('/')
            .split(|c: char| c.is_whitespace() || c == '/')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        match (closing, name.as_str()) {
            (false, "table") => {
                let candidate = self.table_candidates.next().copied().unwrap_or(false);
                if self.table_depth == 0
                    && candidate
                    && let Some((consumed, table)) = simple_table(&self.input[self.pos..])
                {
                    self.flush_pending_list_item();
                    table.append_to(&mut self.blocks);
                    self.pos += consumed;
                } else {
                    // 复杂表格仍忽略表格标签，沿原有段落 / 图片处理路径；不转换嵌套表格。
                    self.table_depth += 1;
                }
            }
            (true, "table") => self.table_depth = self.table_depth.saturating_sub(1),
            (false, "p") => {
                self.flush_paragraph();
                self.in_p = true;
            }
            (true, "p") => {
                self.flush_paragraph();
                self.in_p = false;
            }
            (false, "h1" | "h2" | "h3" | "h4" | "h5" | "h6") => {
                self.flush_paragraph();
                self.heading = Some(name.as_bytes()[1] - b'0');
            }
            (true, "h1" | "h2" | "h3" | "h4" | "h5" | "h6") => {
                let level = self.heading.take().unwrap_or(2);
                let mut inlines = std::mem::take(&mut self.inlines);
                strip_heading_anchors(&mut inlines);
                if !inlines_empty(&inlines) {
                    self.blocks.push(Block::Heading { level, inlines });
                }
            }
            (false, "ul") => {
                self.flush_pending_list_item();
                self.list_kind.push(ListKind::Ul);
            }
            (true, "ul") => {
                self.list_kind.pop();
            }
            (false, "ol") => {
                self.flush_pending_list_item();
                self.list_kind.push(ListKind::Ol { next: 1 });
            }
            (true, "ol") => {
                self.list_kind.pop();
            }
            (false, "li") => self.flush_pending_list_item(),
            (true, "li") => self.flush_pending_list_item(),
            (false, "pre") => {
                self.flush_paragraph();
                self.in_pre = true;
                self.pre_buf.clear();
            }
            (true, "pre") => {
                self.in_pre = false;
                let text = std::mem::take(&mut self.pre_buf);
                self.blocks.push(Block::CodeBlock { lang: None, text });
            }
            (false, "blockquote") => {
                self.flush_paragraph();
                self.in_quote = true;
            }
            (true, "blockquote") => {
                let inlines = std::mem::take(&mut self.inlines);
                if !inlines_empty(&inlines) {
                    self.blocks.push(Block::Quote { inlines });
                }
                self.in_quote = false;
            }
            (false, "img") => {
                let src = attr(raw, "src").unwrap_or_default();
                let alt = attr(raw, "alt").unwrap_or_default();
                if !src.is_empty() {
                    self.blocks.push(Block::Image { src, alt });
                }
            }
            (false, "hr") => self.blocks.push(Block::Rule),
            (false, "br") => self.push_break(),
            (false, "div") => {
                self.flush_paragraph();
                self.div_depth = self.div_depth.saturating_add(1);
            }
            (true, "div") => {
                self.flush_paragraph();
                self.div_depth = self.div_depth.saturating_sub(1);
            }
            (false, "a") => {
                self.inline_kind.push("a");
                if let Some(href) = attr(raw, "href") {
                    self.inline_kind.pop();
                    self.inline_kind.push("a");
                    self.open_link(href);
                }
            }
            (true, "a") => self.close_link(),
            (false, "em" | "i") => self.inline_kind.push("em"),
            (true, "em" | "i") => self.wrap_inline("em"),
            (false, "strong" | "b") => self.inline_kind.push("strong"),
            (true, "strong" | "b") => self.wrap_inline("strong"),
            (false, "code") if !self.in_pre => self.inline_kind.push("code"),
            (true, "code") if !self.in_pre => self.wrap_code(),
            _ => {}
        }
    }

    fn push_text(&mut self, text: String) {
        if !text.as_bytes().contains(&b'\n') && !text.as_bytes().contains(&b'\r') {
            if text.chars().all(char::is_whitespace) && self.inlines.is_empty() {
                return;
            }
            self.inlines.push(Inline::text(text));
            return;
        }
        let mut buf = String::new();
        let mut newline_run = 0u32;
        for c in text.chars() {
            if c == '\r' {
                continue;
            }
            if c == '\n' {
                newline_run += 1;
                continue;
            }
            if newline_run > 0 {
                self.flush_text_buf(&mut buf);
                self.apply_newline_run(newline_run);
                newline_run = 0;
            }
            buf.push(c);
        }
        self.flush_text_buf(&mut buf);
        if newline_run > 0 {
            self.apply_newline_run(newline_run);
        }
    }

    /// 源码里的换行：有块级容器时跟 HTML 空白折叠；无标签 blob 才当结构。
    fn apply_newline_run(&mut self, n: u32) {
        if n == 0 {
            return;
        }
        if self.collapses_html_whitespace() {
            if n >= 2 && self.can_split_paragraph() {
                self.flush_paragraph();
            } else {
                self.push_collapsed_space();
            }
            return;
        }
        if n >= 2 {
            self.flush_paragraph();
        } else {
            self.push_break();
        }
    }

    fn collapses_html_whitespace(&self) -> bool {
        self.heading.is_some()
            || !self.list_kind.is_empty()
            || self.in_quote
            || self.in_p
            || self.div_depth > 0
    }

    fn can_split_paragraph(&self) -> bool {
        self.heading.is_none() && !self.in_pre && self.list_kind.is_empty()
    }

    fn push_collapsed_space(&mut self) {
        if self.inlines.is_empty() {
            return;
        }
        match self.inlines.last_mut() {
            Some(Inline::Text { text }) if text.ends_with(char::is_whitespace) => {}
            Some(Inline::Text { text }) => text.push(' '),
            Some(Inline::HardBreak) => {}
            Some(_) => self.inlines.push(Inline::text(" ")),
            None => {}
        }
    }

    fn flush_text_buf(&mut self, buf: &mut String) {
        if buf.is_empty() {
            return;
        }
        let text = std::mem::take(buf);
        if text.chars().all(char::is_whitespace) && self.inlines.is_empty() {
            return;
        }
        self.inlines.push(Inline::text(text));
    }

    fn push_break(&mut self) {
        if self.inlines.is_empty() {
            return;
        }
        if matches!(self.inlines.last(), Some(Inline::HardBreak)) {
            self.flush_paragraph();
        } else {
            self.inlines.push(Inline::HardBreak);
        }
    }

    fn open_link(&mut self, href: String) {
        self.inlines.push(Inline::Link {
            href,
            children: Vec::new(),
        });
        self.inline_kind.push("a-open");
    }

    fn close_link(&mut self) {
        while matches!(self.inline_kind.last(), Some(&"a-open") | Some(&"a")) {
            self.inline_kind.pop();
        }
        // children already accumulated as following text; fold trailing texts into last Link
        let mut children = Vec::new();
        while let Some(inline) = self.inlines.pop() {
            match inline {
                Inline::Link { href, children: c } if c.is_empty() => {
                    children.reverse();
                    self.inlines.push(Inline::Link { href, children });
                    return;
                }
                other => children.push(other),
            }
        }
        children.reverse();
        self.inlines.extend(children);
    }

    fn wrap_inline(&mut self, kind: &str) {
        let mut children = Vec::new();
        if let Some(k) = self.inline_kind.last().copied()
            && k == kind
        {
            self.inline_kind.pop();
        }
        if let Some(Inline::Text { text }) = self.inlines.pop() {
            children.push(Inline::text(text));
        }
        let wrapped = if kind == "strong" {
            Inline::Strong { children }
        } else {
            Inline::Emphasis { children }
        };
        self.inlines.push(wrapped);
    }

    fn wrap_code(&mut self) {
        self.inline_kind.pop();
        if let Some(Inline::Text { text }) = self.inlines.pop() {
            self.inlines.push(Inline::Code { text });
        }
    }

    fn list_depth(&self) -> u8 {
        self.list_kind.len() as u8
    }

    fn flush_pending_list_item(&mut self) {
        if self.list_kind.is_empty() {
            self.flush_paragraph();
            return;
        }
        let inlines = std::mem::take(&mut self.inlines);
        if inlines_empty(&inlines) {
            return;
        }
        let indent = self.list_depth().saturating_sub(1);
        let marker = match self.list_kind.last_mut() {
            Some(ListKind::Ol { next }) => {
                let m = format!("{next}.");
                *next += 1;
                m
            }
            _ => "•".into(),
        };
        self.blocks.push(Block::ListItem {
            indent,
            marker,
            inlines,
        });
    }

    fn flush_paragraph(&mut self) {
        if self.heading.is_some() || self.in_pre || !self.list_kind.is_empty() {
            return;
        }
        let inlines = std::mem::take(&mut self.inlines);
        if inlines_empty(&inlines) {
            return;
        }
        if self.in_quote {
            self.blocks.push(Block::Quote { inlines });
        } else {
            self.blocks.push(Block::Paragraph { inlines });
        }
    }
}

fn table_inline_tag(name: &str) -> bool {
    matches!(
        name,
        "a" | "b" | "br" | "code" | "del" | "em" | "i" | "s" | "span" | "strike" | "strong" | "u"
    )
}

/// 在清洗前判断结构，避免 section 等未入白名单的块标签消失后被误认成简单表格。
fn table_candidates(html: &str) -> Vec<bool> {
    use html5ever::tokenizer::{
        BufferQueue, TagKind, Token, TokenSink, TokenSinkResult, Tokenizer,
    };
    use std::cell::RefCell;
    #[derive(Default)]
    struct Tables {
        candidates: Vec<bool>,
        stack: Vec<usize>,
    }
    #[derive(Default)]
    struct Sink(RefCell<Tables>);
    impl TokenSink for Sink {
        type Handle = ();
        fn process_token(&self, token: Token, _: u64) -> TokenSinkResult<()> {
            if let Token::TagToken(tag) = token {
                let mut state = self.0.borrow_mut();
                let name = tag.name.as_ref();
                if name == "table" && tag.kind == TagKind::EndTag {
                    state.stack.pop();
                } else if tag.kind == TagKind::StartTag {
                    if let Some(&index) = state.stack.last()
                        && matches!(
                            name,
                            "address"
                                | "article"
                                | "aside"
                                | "blockquote"
                                | "details"
                                | "dialog"
                                | "div"
                                | "dl"
                                | "dt"
                                | "dd"
                                | "fieldset"
                                | "figcaption"
                                | "figure"
                                | "footer"
                                | "form"
                                | "h1"
                                | "h2"
                                | "h3"
                                | "h4"
                                | "h5"
                                | "h6"
                                | "header"
                                | "hgroup"
                                | "hr"
                                | "li"
                                | "main"
                                | "menu"
                                | "nav"
                                | "ol"
                                | "p"
                                | "pre"
                                | "section"
                                | "summary"
                                | "table"
                                | "ul"
                        )
                    {
                        state.candidates[index] = false;
                    }
                    if name == "table" {
                        let index = state.candidates.len();
                        state.candidates.push(true);
                        state.stack.push(index);
                    }
                    use html5ever::tokenizer::states::RawKind;
                    match name {
                        "script" => return TokenSinkResult::RawData(RawKind::ScriptData),
                        "style" | "xmp" | "iframe" | "noembed" | "noframes" => {
                            return TokenSinkResult::RawData(RawKind::Rawtext);
                        }
                        "title" | "textarea" => return TokenSinkResult::RawData(RawKind::Rcdata),
                        _ => {}
                    }
                }
            }
            TokenSinkResult::Continue
        }
    }
    let input = BufferQueue::default();
    input.push_back(html.into());
    let tokenizer = Tokenizer::new(Sink::default(), Default::default());
    let _ = tokenizer.feed(&input);
    tokenizer.end();
    tokenizer.sink.0.into_inner().candidates
}

/// 输入已由 ammonia 规范化；单元格只允许内联标签，其他情况交还旧 walker。
fn simple_table(html: &str) -> Option<(usize, crate::table::Table)> {
    let mut table = crate::table::Table::default();
    let mut row = Vec::new();
    let mut align = Vec::new();
    let mut row_has_th = false;
    let mut head_found = false;
    let mut first_align = Vec::new();
    let mut pos = 0;
    loop {
        let start = pos + html[pos..].find('<')?;
        if !html[pos..start].trim().is_empty() {
            return None;
        }
        let end = start + html[start..].find('>')? + 1;
        let raw = &html[start..end];
        let name = raw[1..raw.len() - 1].split_whitespace().next()?;
        pos = end;
        match name {
            "thead" | "/thead" | "tbody" | "/tbody" | "tfoot" | "/tfoot" | "tr" => {}
            "th" | "td" => {
                for span in ["rowspan", "colspan"] {
                    if attr(raw, span).is_some_and(|v| v.trim().parse::<usize>() != Ok(1)) {
                        return None;
                    }
                }
                let close = format!("</{name}>");
                let cell_end = pos + html[pos..].find(&close)?;
                let cell = &html[pos..cell_end];
                for tag in cell.split('<').skip(1) {
                    let tag = tag
                        .trim_start_matches('/')
                        .split(|c: char| c.is_whitespace() || c == '>')
                        .next()?;
                    if !table_inline_tag(tag) {
                        return None;
                    }
                }
                let mut inlines = Vec::new();
                for block in walk(&format!("<p>{cell}</p>"), &[]) {
                    let Block::Paragraph { inlines: part } = block else {
                        return None;
                    };
                    if !inlines.is_empty() {
                        inlines.push(Inline::HardBreak);
                    }
                    inlines.extend(part);
                }
                row.push(inlines);
                align.push(
                    match attr(raw, "align")
                        .unwrap_or_default()
                        .to_ascii_lowercase()
                        .as_str()
                    {
                        "left" => "l",
                        "center" => "c",
                        "right" => "r",
                        _ => "",
                    }
                    .into(),
                );
                row_has_th |= name == "th";
                pos = cell_end + close.len();
            }
            "/tr" => {
                if row_has_th && !head_found {
                    table.head = std::mem::take(&mut row);
                    table.align = std::mem::take(&mut align);
                    head_found = true;
                } else {
                    if table.rows.is_empty() {
                        first_align = align.clone();
                    }
                    table.rows.push(std::mem::take(&mut row));
                }
                align.clear();
                row_has_th = false;
            }
            "/table" => {
                if !head_found {
                    if table.rows.is_empty() {
                        return None;
                    }
                    table.head = table.rows.remove(0);
                    table.align = first_align;
                }
                return (!table.head.is_empty()).then_some((pos, table));
            }
            _ => return None,
        }
    }
}

/// 标题里常见的「本节链接」符号，点了只跳到本节，不是正文。
const ANCHOR_GLYPHS: [&str; 6] = ["#", "¶", "§", "⌗", "🔗", "⚓"];

fn is_anchor_glyph(text: &str) -> bool {
    ANCHOR_GLYPHS.contains(&text.trim())
}

/// 去掉标题里的锚点链接：页内链接只含锚点符号（或只有图标，内容为空）。
/// 没有页面地址时相对锚点的 href 已被清洗掉，只剩文字，所以首尾单独的锚点符号也去掉。
fn strip_heading_anchors(inlines: &mut Vec<Inline>) {
    inlines.retain(|inline| {
        !matches!(inline, Inline::Link { href, children }
            if href.contains('#') && (children.is_empty() || is_anchor_glyph(&crate::inlines_to_plain(children))))
    });
    for leading in [false, true] {
        let index = if leading {
            0
        } else {
            inlines.len().saturating_sub(1)
        };
        let rest_has_text = |inlines: &[Inline]| {
            inlines
                .iter()
                .enumerate()
                .any(|(i, inline)| i != index && !inlines_empty(std::slice::from_ref(inline)))
        };
        let Some(Inline::Text { text }) = inlines.get(index) else {
            continue;
        };
        let trimmed = if is_anchor_glyph(text) {
            rest_has_text(inlines).then(String::new)
        } else if leading {
            text.trim_start()
                .split_once(char::is_whitespace)
                .filter(|(head, tail)| is_anchor_glyph(head) && !tail.trim().is_empty())
                .map(|(_, tail)| tail.trim_start().to_string())
        } else {
            text.trim_end()
                .rsplit_once(char::is_whitespace)
                .filter(|(head, tail)| is_anchor_glyph(tail) && !head.trim().is_empty())
                .map(|(head, _)| head.trim_end().to_string())
        };
        if let Some(trimmed) = trimmed {
            if trimmed.is_empty() {
                inlines.remove(index);
            } else {
                inlines[index] = Inline::text(trimmed);
            }
        }
    }
    if let Some(Inline::Text { text }) = inlines.first_mut() {
        *text = text.trim_start().to_string();
    }
    if let Some(Inline::Text { text }) = inlines.last_mut() {
        let end = text.trim_end().len();
        text.truncate(end);
    }
}

fn inlines_empty(inlines: &[Inline]) -> bool {
    inlines.iter().all(|i| match i {
        Inline::Text { text } => text.trim().is_empty(),
        Inline::HardBreak => true,
        _ => false,
    })
}

fn attr(tag: &str, name: &str) -> Option<String> {
    let needle = format!("{name}=\"");
    if let Some(i) = tag.find(&needle) {
        let rest = &tag[i + needle.len()..];
        let end = rest.find('"')?;
        return Some(html_unescape(&rest[..end]));
    }
    let needle = format!("{name}='");
    if let Some(i) = tag.find(&needle) {
        let rest = &tag[i + needle.len()..];
        let end = rest.find('\'')?;
        return Some(html_unescape(&rest[..end]));
    }
    None
}

fn html_unescape(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::Block;
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

    fn paragraphs(html: &str) -> Vec<(String, bool)> {
        html_to_blocks(html, None)
            .into_iter()
            .filter_map(|b| match b {
                Block::Paragraph { inlines } => {
                    Some((inlines_to_plain(&inlines), has_hard_break(&inlines)))
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn p_source_wrap_collapses_to_space() {
        let paras = paragraphs(
            "<p>The Debian Project is voting\non the <a href=\"https://ex.test/g\">first\nproposal</a>.</p>",
        );
        assert_eq!(paras.len(), 1, "{paras:?}");
        assert!(
            paras[0].0.contains("voting on the"),
            "soft wrap must become a space: {paras:?}"
        );
        assert!(
            paras[0].0.contains("first proposal"),
            "link text must reflow: {paras:?}"
        );
        assert!(
            !paras[0].1,
            "source wrap must not become HardBreak: {paras:?}"
        );
    }

    #[test]
    fn p_blank_line_splits_dirty_feed() {
        let paras = paragraphs("<p>带换行的纯文本：\n\n第二行之后应成新段</p>");
        assert!(
            paras.iter().any(|(t, _)| t.contains("带换行的纯文本")),
            "{paras:?}"
        );
        assert!(
            paras.iter().any(|(t, _)| t.contains("第二行之后应成新段")),
            "{paras:?}"
        );
        assert!(
            !paras
                .iter()
                .any(|(t, _)| t.contains("带换行的纯文本") && t.contains("第二行之后应成新段")),
            "\\n\\n inside <p> splits dirty RSS: {paras:?}"
        );
    }

    #[test]
    fn br_stays_hard_break_double_br_splits() {
        let single = paragraphs("<p>上<br>下</p>");
        assert_eq!(single.len(), 1, "{single:?}");
        assert_eq!(single[0].0, "上 下");
        assert!(single[0].1, "explicit <br> must stay: {single:?}");

        let double = paragraphs("<p>甲<br><br>乙</p>");
        assert!(double.iter().any(|(t, _)| t.contains('甲')), "{double:?}");
        assert!(double.iter().any(|(t, _)| t.contains('乙')), "{double:?}");
        assert!(double.len() >= 2, "br+br splits: {double:?}");
    }

    #[test]
    fn untagged_newlines_are_structure() {
        let paras = paragraphs("第一段\n\n第二段\n第三行");
        assert!(paras.len() >= 2, "\\n\\n starts a paragraph: {paras:?}");
        assert!(paras[0].0.contains("第一段"), "{paras:?}");
        let second = paras
            .iter()
            .find(|(t, _)| t.contains("第二段"))
            .expect("second paragraph");
        assert!(second.0.contains("第三行"), "{paras:?}");
        assert!(second.1, "untagged single \\n is HardBreak: {paras:?}");
    }

    #[test]
    fn list_source_wrap_collapses_br_kept() {
        let blocks = html_to_blocks("<ul><li>foo\nbar</li><li>baz<br>qux</li></ul>", None);
        let items: Vec<(String, bool)> = blocks
            .into_iter()
            .filter_map(|b| match b {
                Block::ListItem { inlines, .. } => {
                    Some((inlines_to_plain(&inlines), has_hard_break(&inlines)))
                }
                _ => None,
            })
            .collect();
        assert_eq!(items.len(), 2, "{items:?}");
        assert_eq!(items[0].0, "foo bar");
        assert!(!items[0].1, "li wrap is space: {items:?}");
        assert_eq!(items[1].0, "baz qux");
        assert!(items[1].1, "li <br> stays: {items:?}");
    }

    #[test]
    fn pre_keeps_literal_newlines() {
        let blocks = html_to_blocks("<pre>open 09:00\nclose 21:00</pre>", None);
        match &blocks[0] {
            Block::CodeBlock { text, .. } => {
                assert!(text.contains('\n'), "{text:?}");
                assert!(text.contains("open 09:00"), "{text:?}");
                assert!(text.contains("close 21:00"), "{text:?}");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn div_wrap_collapses_divs_still_split() {
        let paras = paragraphs("<div>第一段\n折行</div><div>第二段</div>");
        assert!(
            paras
                .iter()
                .any(|(t, br)| t.contains("第一段") && t.contains("折行") && !br),
            "{paras:?}"
        );
        assert!(
            !paras
                .iter()
                .any(|(t, _)| t.contains("第一段") && t.contains("第二段")),
            "{paras:?}"
        );
    }
}
