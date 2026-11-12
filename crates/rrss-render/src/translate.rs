//! 块 IR ↔ 带占位标记的纯文本。只做抽取与嵌回，不调用任何翻译服务。
//!
//! 加粗 / 斜体 / 链接变成 `<xn>…</xn>`，代码变成自闭合 `<xn/>`。
//! 前缀 `x` 是为了合法 XML 名（不能以数字开头），DeepL `tag_handling=xml` 才能保住标签。
//! 硬换行收成空格，不当占位符（中英语序会打乱换行位置）。
//! `href` 与代码原文留在 wrappers 里，不进模型。日志 target：`rrss_render::translate`。

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::ir::{Block, Document, Inline};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranslateUnit {
    pub i: u32,
    pub t: String,
}

#[derive(Debug, Clone)]
enum Wrapper {
    Strong,
    Emphasis,
    Link { href: String },
    Code { text: String },
}

/// DeepL XML 标签名不能以数字开头，故用 `x0` 而不是 `0`。
const TAG: &str = "x";

fn push_open(out: &mut String, id: usize) {
    out.push('<');
    out.push_str(TAG);
    out.push_str(&id.to_string());
    out.push('>');
}

fn push_close(out: &mut String, id: usize) {
    out.push_str("</");
    out.push_str(TAG);
    out.push_str(&id.to_string());
    out.push('>');
}

fn push_void(out: &mut String, id: usize) {
    out.push('<');
    out.push_str(TAG);
    out.push_str(&id.to_string());
    out.push_str("/>");
}

pub fn extract_units(title: &str, doc: &Document) -> Vec<TranslateUnit> {
    TranslationPlan::new(title, doc).units
}

/// 文档块下标和可译编号分别维护；一个块最多对应一个单元，标题没有块下标。
#[derive(Debug, Clone)]
pub struct TranslationPlan {
    pub units: Vec<TranslateUnit>,
    pub block_ids: Vec<Option<usize>>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TranslationPatch {
    Title(String),
    Block { index: usize, block: Block },
}

/// 单元还原失败的原因。编号缺失和重复由调用方记录；其余来自占位校验。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatchError {
    MissingUnit,
    DuplicateUnit,
    Empty,
    MissingPlaceholder,
    DuplicatePlaceholder,
    UnknownPlaceholder,
    TagStructure,
    Remainder,
}

impl PatchError {
    pub fn reason(self) -> &'static str {
        match self {
            Self::MissingUnit => "missing_unit",
            Self::DuplicateUnit => "duplicate_unit",
            Self::Empty => "empty",
            Self::MissingPlaceholder => "missing_placeholder",
            Self::DuplicatePlaceholder => "duplicate_placeholder",
            Self::UnknownPlaceholder => "unknown_placeholder",
            Self::TagStructure => "tag_structure",
            Self::Remainder => "remainder",
        }
    }
}

impl TranslationPlan {
    pub fn new(title: &str, doc: &Document) -> Self {
        let mut units = Vec::new();
        let mut block_ids = Vec::new();
        let mut i = 0u32;
        if !title.trim().is_empty() {
            units.push(TranslateUnit {
                i,
                t: title.to_string(),
            });
            block_ids.push(None);
            i += 1;
        }
        for (index, block) in doc.blocks.iter().enumerate() {
            if let Some(t) = block_source(block)
                && !plain_from_tagged(&t).trim().is_empty()
            {
                units.push(TranslateUnit { i, t });
                block_ids.push(Some(index));
                i += 1;
            }
        }
        Self { units, block_ids }
    }

    /// 只还原当前单元。段落类先严格校验占位；失败再宽松解析，并丢掉缺失的
    /// 加粗 / 斜体 / 链接、把缺失代码补到段尾。图片说明和嵌入标题不降级。
    /// 整篇 [`apply_units`] 仍走原来的纯文本回退。
    pub fn patch(
        &self,
        source: &Document,
        id: u32,
        text: &str,
    ) -> Result<TranslationPatch, PatchError> {
        if text.trim().is_empty() {
            return Err(PatchError::Empty);
        }
        let Some(index) = self.block_ids.get(id as usize).copied() else {
            return Err(PatchError::MissingUnit);
        };
        let Some(index) = index else {
            return Ok(TranslationPatch::Title(text.into()));
        };
        let Some(mut block) = source.blocks.get(index).cloned() else {
            return Err(PatchError::MissingUnit);
        };
        match &mut block {
            Block::Heading { inlines, .. }
            | Block::Paragraph { inlines }
            | Block::ListItem { inlines, .. }
            | Block::Quote { inlines } => {
                let (_, wrappers) = inlines_to_tagged(inlines);
                match strict_inlines(text, &wrappers) {
                    Ok(translated) => *inlines = translated,
                    Err(error) => {
                        let Some((translated, reason)) = lenient_inlines(text, &wrappers) else {
                            return Err(error);
                        };
                        tracing::debug!(
                            unit = id,
                            reason,
                            "translation unit used placeholder fallback"
                        );
                        *inlines = translated;
                    }
                }
            }
            Block::Image { alt, .. } => *alt = text.into(),
            Block::Embed { title, .. } => *title = text.into(),
            _ => return Err(PatchError::TagStructure),
        }
        Ok(TranslationPatch::Block { index, block })
    }
}

pub fn apply_units(title: &str, doc: &Document, units: &[TranslateUnit]) -> (String, Document) {
    let map: HashMap<u32, &str> = units.iter().map(|u| (u.i, u.t.as_str())).collect();
    let mut i = 0u32;
    let mut new_title = title.to_string();
    if !title.trim().is_empty() {
        if let Some(t) = map.get(&i) {
            new_title = (*t).to_string();
        }
        i += 1;
    }
    let mut blocks = doc.blocks.clone();
    for block in &mut blocks {
        if let Some(src) = block_source(block)
            && !plain_from_tagged(&src).trim().is_empty()
        {
            if let Some(t) = map.get(&i) {
                apply_block(block, t);
            }
            i += 1;
        }
    }
    (new_title, Document { v: doc.v, blocks })
}

/// 去掉占位标记后的可见文字，供语种启发式使用。
pub fn units_plain(units: &[TranslateUnit]) -> String {
    let mut out = String::new();
    for u in units {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&plain_from_tagged(&u.t));
    }
    out
}

/// 目标语为中文时，正文已是汉字为主则不必送在线服务。
pub fn already_target_lang(plain: &str, target: &str) -> bool {
    let target = target.trim().to_ascii_lowercase();
    if target != "zh" && target != "zh-cn" && target != "zh-hans" {
        return false;
    }
    let mut han = 0usize;
    let mut letters = 0usize;
    for c in plain.chars() {
        if is_han(c) {
            han += 1;
            letters += 1;
        } else if c.is_alphabetic() {
            letters += 1;
        }
    }
    han >= 12 && letters > 0 && han * 100 / letters >= 40
}

fn is_han(c: char) -> bool {
    matches!(
        c,
        '\u{4E00}'..='\u{9FFF}' | '\u{3400}'..='\u{4DBF}' | '\u{F900}'..='\u{FAFF}'
    )
}

fn block_source(block: &Block) -> Option<String> {
    match block {
        Block::Heading { inlines, .. }
        | Block::Paragraph { inlines }
        | Block::ListItem { inlines, .. }
        | Block::Quote { inlines } => {
            let (tagged, _) = inlines_to_tagged(inlines);
            Some(tagged)
        }
        Block::Image { alt, .. } if !alt.trim().is_empty() => Some(alt.clone()),
        Block::Embed { title, .. } if !title.trim().is_empty() => Some(title.clone()),
        Block::CodeBlock { .. }
        | Block::Image { .. }
        | Block::Embed { .. }
        | Block::Table { .. }
        | Block::Rule => None,
    }
}

fn apply_block(block: &mut Block, translated: &str) {
    match block {
        Block::Heading { inlines, .. }
        | Block::Paragraph { inlines }
        | Block::ListItem { inlines, .. }
        | Block::Quote { inlines } => {
            let (_, wrappers) = inlines_to_tagged(inlines);
            *inlines = tagged_to_inlines(translated, &wrappers);
        }
        Block::Image { alt, .. } => *alt = translated.to_string(),
        Block::Embed { title, .. } => *title = translated.to_string(),
        Block::CodeBlock { .. } | Block::Table { .. } | Block::Rule => {}
    }
}

fn inlines_to_tagged(inlines: &[Inline]) -> (String, Vec<Wrapper>) {
    let mut out = String::new();
    let mut wrappers = Vec::new();
    write_tagged(inlines, &mut out, &mut wrappers);
    (out, wrappers)
}

fn write_tagged(inlines: &[Inline], out: &mut String, wrappers: &mut Vec<Wrapper>) {
    for inline in inlines {
        match inline {
            Inline::Text { text } => out.push_str(&escape_text(text)),
            Inline::Strong { children } => {
                let id = wrappers.len();
                wrappers.push(Wrapper::Strong);
                push_open(out, id);
                write_tagged(children, out, wrappers);
                push_close(out, id);
            }
            Inline::Emphasis { children } => {
                let id = wrappers.len();
                wrappers.push(Wrapper::Emphasis);
                push_open(out, id);
                write_tagged(children, out, wrappers);
                push_close(out, id);
            }
            Inline::Link { href, children } => {
                let id = wrappers.len();
                wrappers.push(Wrapper::Link { href: href.clone() });
                push_open(out, id);
                write_tagged(children, out, wrappers);
                push_close(out, id);
            }
            Inline::Code { text } => {
                let id = wrappers.len();
                wrappers.push(Wrapper::Code { text: text.clone() });
                push_void(out, id);
            }
            // 硬换行不当占位符：中英语序重排会把 `<n/>` 挪到词中/行首标点。
            // 原文 IR 仍保留 HardBreak；译文按空格流式排。
            Inline::HardBreak => {
                if !out.ends_with(' ') && !out.is_empty() {
                    out.push(' ');
                }
            }
        }
    }
}

fn tagged_to_inlines(tagged: &str, wrappers: &[Wrapper]) -> Vec<Inline> {
    match parse_seq(tagged, wrappers) {
        Ok((inlines, rest)) if rest.trim().is_empty() => {
            if inlines.is_empty() {
                vec![Inline::text(plain_from_tagged(tagged))]
            } else {
                inlines
            }
        }
        _ => vec![Inline::text(plain_from_tagged(tagged))],
    }
}

fn strict_inlines(text: &str, wrappers: &[Wrapper]) -> Result<Vec<Inline>, PatchError> {
    let (translated, rest) = parse_seq(text, wrappers)?;
    if !rest.trim().is_empty() {
        return Err(PatchError::Remainder);
    }
    if translated.is_empty() {
        return Err(PatchError::Empty);
    }
    let mut seen = vec![false; wrappers.len()];
    let mut rest = text;
    while let Some(start) = rest.find('<') {
        rest = &rest[start + 1..];
        let Some(end) = rest.find('>') else {
            return Err(PatchError::TagStructure);
        };
        let tag = &rest[..end];
        if !tag.starts_with('/') {
            let tag = tag.strip_suffix('/').unwrap_or(tag);
            let id: usize = tag
                .strip_prefix('x')
                .unwrap_or(tag)
                .parse()
                .map_err(|_| PatchError::TagStructure)?;
            let Some(used) = seen.get_mut(id) else {
                return Err(PatchError::UnknownPlaceholder);
            };
            if *used {
                return Err(PatchError::DuplicatePlaceholder);
            }
            *used = true;
        }
        rest = &rest[end + 1..];
    }
    if seen.iter().any(|used| !used) {
        return Err(PatchError::MissingPlaceholder);
    }
    Ok(translated)
}

fn degrade_reason(structure: bool, dropped: bool, appended: bool) -> &'static str {
    match (structure, dropped, appended) {
        (_, false, false) => "lenient",
        (false, true, false) => "dropped_format",
        (false, false, true) => "appended_code",
        (true, true, false) => "lenient+dropped_format",
        (true, false, true) => "lenient+appended_code",
        (false, true, true) => "dropped_format+appended_code",
        (true, true, true) => "lenient+dropped_format+appended_code",
    }
}

fn lenient_inlines(text: &str, wrappers: &[Wrapper]) -> Option<(Vec<Inline>, &'static str)> {
    let mut used = vec![false; wrappers.len()];
    let mut structure = false;
    let (mut inlines, _) = take_lenient(text, wrappers, &mut used, None, &mut structure);
    let mut dropped = false;
    let mut appended = false;
    for (id, wrapper) in wrappers.iter().enumerate() {
        if used[id] {
            continue;
        }
        match wrapper {
            Wrapper::Code { text } => {
                appended = true;
                push_space(&mut inlines);
                inlines.push(Inline::Code { text: text.clone() });
            }
            Wrapper::Strong | Wrapper::Emphasis | Wrapper::Link { .. } => dropped = true,
        }
    }
    if !has_content(&inlines) {
        return None;
    }
    Some((inlines, degrade_reason(structure, dropped, appended)))
}

fn has_content(inlines: &[Inline]) -> bool {
    inlines.iter().any(|inline| match inline {
        Inline::Text { text } => !text.trim().is_empty(),
        Inline::Code { .. } => true,
        Inline::Strong { children }
        | Inline::Emphasis { children }
        | Inline::Link { children, .. } => has_content(children),
        Inline::HardBreak => false,
    })
}

fn push_text(out: &mut Vec<Inline>, text: String) {
    if text.is_empty() {
        return;
    }
    if let Some(Inline::Text { text: last }) = out.last_mut() {
        last.push_str(&text);
    } else {
        out.push(Inline::text(text));
    }
}

fn push_space(inlines: &mut Vec<Inline>) {
    match inlines.last_mut() {
        Some(Inline::Text { text }) if !text.ends_with(' ') => text.push(' '),
        Some(Inline::Text { .. }) => {}
        _ => inlines.push(Inline::text(" ")),
    }
}

fn wrap_inline(wrapper: &Wrapper, children: Vec<Inline>) -> Inline {
    match wrapper {
        Wrapper::Strong => Inline::Strong { children },
        Wrapper::Emphasis => Inline::Emphasis { children },
        Wrapper::Link { href } => Inline::Link {
            href: href.clone(),
            children,
        },
        Wrapper::Code { text } => Inline::Code { text: text.clone() },
    }
}

enum LenientTag {
    Open,
    Close,
    Void,
}

fn skip_ws(bytes: &[u8], i: &mut usize) {
    while bytes.get(*i).is_some_and(u8::is_ascii_whitespace) {
        *i += 1;
    }
}

fn match_tag(s: &str) -> Option<(LenientTag, usize, usize)> {
    let bytes = s.as_bytes();
    if bytes.first() != Some(&b'<') {
        return None;
    }
    let mut i = 1usize;
    skip_ws(bytes, &mut i);
    let close = if bytes.get(i) == Some(&b'/') {
        i += 1;
        skip_ws(bytes, &mut i);
        true
    } else {
        false
    };
    if bytes.get(i) == Some(&b'x') && bytes.get(i + 1).is_some_and(|b| b.is_ascii_digit()) {
        i += 1;
    }
    let digits_at = i;
    while bytes.get(i).is_some_and(|b| b.is_ascii_digit()) {
        i += 1;
    }
    if i == digits_at {
        return None;
    }
    let id: usize = s[digits_at..i].parse().ok()?;
    skip_ws(bytes, &mut i);
    let void = if !close && bytes.get(i) == Some(&b'/') {
        i += 1;
        skip_ws(bytes, &mut i);
        true
    } else {
        false
    };
    if bytes.get(i) != Some(&b'>') {
        return None;
    }
    let kind = if close {
        LenientTag::Close
    } else if void {
        LenientTag::Void
    } else {
        LenientTag::Open
    };
    Some((kind, id, i + 1))
}

fn take_lenient<'a>(
    s: &'a str,
    wrappers: &[Wrapper],
    used: &mut [bool],
    until: Option<usize>,
    structure: &mut bool,
) -> (Vec<Inline>, &'a str) {
    let mut out = Vec::new();
    let mut rest = s;
    while let Some(at) = rest.find('<') {
        if at > 0 {
            push_text(&mut out, unescape_text(&rest[..at]));
        }
        rest = &rest[at..];
        let Some((kind, id, len)) = match_tag(rest) else {
            push_text(&mut out, "<".into());
            *structure = true;
            rest = &rest[1..];
            continue;
        };
        if rest[..len].bytes().any(|byte| byte.is_ascii_whitespace()) {
            *structure = true;
        }
        rest = &rest[len..];
        match kind {
            LenientTag::Close if until == Some(id) => return (out, rest),
            LenientTag::Close => *structure = true,
            LenientTag::Void => match wrappers.get(id) {
                Some(Wrapper::Code { text }) if !used[id] => {
                    used[id] = true;
                    out.push(Inline::Code { text: text.clone() });
                }
                _ => *structure = true,
            },
            LenientTag::Open
                if !used.get(id).copied().unwrap_or(true)
                    && matches!(
                        wrappers.get(id),
                        Some(Wrapper::Strong | Wrapper::Emphasis | Wrapper::Link { .. })
                    ) =>
            {
                used[id] = true;
                let (children, next) = take_lenient(rest, wrappers, used, Some(id), structure);
                rest = next;
                out.push(wrap_inline(&wrappers[id], children));
            }
            LenientTag::Open => *structure = true,
        }
    }
    if !rest.is_empty() {
        push_text(&mut out, unescape_text(rest));
    }
    if until.is_some() {
        *structure = true;
    }
    (out, "")
}

fn parse_seq<'a>(s: &'a str, wrappers: &[Wrapper]) -> Result<(Vec<Inline>, &'a str), PatchError> {
    let mut out = Vec::new();
    let mut rest = s;
    while !rest.is_empty() {
        if rest.starts_with("</") {
            break;
        }
        if rest.starts_with('<') {
            let (inline, next) = parse_tag(rest, wrappers)?;
            out.push(inline);
            rest = next;
            continue;
        }
        let split = rest.find('<').unwrap_or(rest.len());
        let text = unescape_text(&rest[..split]);
        if !text.is_empty() {
            out.push(Inline::text(text));
        }
        rest = &rest[split..];
    }
    Ok((out, rest))
}

fn parse_tag<'a>(s: &'a str, wrappers: &[Wrapper]) -> Result<(Inline, &'a str), PatchError> {
    let bytes = s.as_bytes();
    if bytes.first() != Some(&b'<') {
        return Err(PatchError::TagStructure);
    }
    // `<x12>`（现行）或旧式 `<12>`。标签内空白留给宽松解析。
    let mut i = 1usize;
    if bytes.get(1) == Some(&b'x') && bytes.get(2).is_some_and(|b| b.is_ascii_digit()) {
        i = 2;
    }
    let digits_at = i;
    while i < s.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if i == digits_at {
        return Err(PatchError::TagStructure);
    }
    let id: usize = s[digits_at..i]
        .parse()
        .map_err(|_| PatchError::TagStructure)?;
    let Some(wrapper) = wrappers.get(id) else {
        return Err(PatchError::UnknownPlaceholder);
    };
    let name = &s[1..i];
    if s[i..].starts_with("/>") {
        let inline = match wrapper {
            Wrapper::Code { text } => Inline::Code { text: text.clone() },
            _ => return Err(PatchError::TagStructure),
        };
        return Ok((inline, &s[i + 2..]));
    }
    if !s[i..].starts_with('>') {
        return Err(PatchError::TagStructure);
    }
    let inner = &s[i + 1..];
    let (children, after) = parse_seq(inner, wrappers)?;
    let close = format!("</{name}>");
    if !after.starts_with(&close) {
        return Err(PatchError::TagStructure);
    }
    let inline = match wrapper {
        Wrapper::Strong => Inline::Strong { children },
        Wrapper::Emphasis => Inline::Emphasis { children },
        Wrapper::Link { href } => Inline::Link {
            href: href.clone(),
            children,
        },
        Wrapper::Code { .. } => return Err(PatchError::TagStructure),
    };
    Ok((inline, &after[close.len()..]))
}

fn escape_text(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;")
}

fn unescape_text(s: &str) -> String {
    s.replace("&lt;", "<").replace("&amp;", "&")
}

fn plain_from_tagged(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while !rest.is_empty() {
        if let Some(i) = rest.find('<') {
            out.push_str(&unescape_text(&rest[..i]));
            rest = &rest[i..];
            if let Some(end) = rest.find('>') {
                rest = &rest[end + 1..];
            } else {
                break;
            }
        } else {
            out.push_str(&unescape_text(rest));
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::IR_VERSION;

    fn para(inlines: Vec<Inline>) -> Block {
        Block::Paragraph { inlines }
    }

    #[test]
    fn progressive_mapping_skips_shared_blocks_and_requires_every_placeholder_once() {
        let doc = Document::new(vec![
            Block::Rule,
            para(vec![
                Inline::text("Read "),
                Inline::Link {
                    href: "https://example.test".into(),
                    children: vec![Inline::text("this")],
                },
                Inline::Code {
                    text: "secret_code()".into(),
                },
            ]),
            Block::CodeBlock {
                lang: None,
                text: "unchanged".into(),
            },
            Block::Image {
                src: "https://image.test".into(),
                alt: "".into(),
            },
            para(vec![Inline::text("last")]),
        ]);
        let plan = TranslationPlan::new("title", &doc);
        assert_eq!(plan.block_ids, [None, Some(1), Some(4)]);
        let Ok(TranslationPatch::Block { index, block }) =
            plan.patch(&doc, 1, "<x1/>阅读<x0>链接</x0>")
        else {
            panic!("valid reorder");
        };
        assert_eq!(index, 1);
        let Block::Paragraph { inlines } = block else {
            panic!("paragraph");
        };
        assert!(matches!(&inlines[0], Inline::Code { text } if text == "secret_code()"));
        assert_eq!(
            inlines
                .iter()
                .filter(|inline| matches!(inline, Inline::Code { .. }))
                .count(),
            1,
            "strict restore must not append another copy of the code"
        );
        assert!(inlines.iter().any(
            |inline| matches!(inline, Inline::Link { href, .. } if href == "https://example.test")
        ));
        assert_eq!(
            plan.patch(&doc, 99, "wrong index"),
            Err(PatchError::MissingUnit)
        );
        assert_eq!(plan.patch(&doc, 0, " "), Err(PatchError::Empty));
        assert_eq!(
            plan.patch(&doc, 0, "标题"),
            Ok(TranslationPatch::Title("标题".into()))
        );
        let plain = Document::new(vec![
            Block::Image {
                src: "https://example.test/a.png".into(),
                alt: "cover".into(),
            },
            Block::Embed {
                kind: crate::EmbedKind::Video,
                url: "https://example.test/v".into(),
                title: "Watch".into(),
            },
        ]);
        let plain_plan = TranslationPlan::new("", &plain);
        match plain_plan.patch(&plain, 0, "封面 <a>").unwrap() {
            TranslationPatch::Block { block, .. } => match block {
                Block::Image { alt, .. } => assert_eq!(alt, "封面 <a>"),
                other => panic!("{other:?}"),
            },
            other => panic!("{other:?}"),
        }
        match plain_plan.patch(&plain, 1, "看 <这里>").unwrap() {
            TranslationPatch::Block { block, .. } => match block {
                Block::Embed { title, .. } => assert_eq!(title, "看 <这里>"),
                other => panic!("{other:?}"),
            },
            other => panic!("{other:?}"),
        }
    }

    fn link_plan() -> (Document, TranslationPlan) {
        let doc = Document::new(vec![para(vec![Inline::Link {
            href: "https://example.test".into(),
            children: vec![Inline::text("docs")],
        }])]);
        let plan = TranslationPlan::new("", &doc);
        (doc, plan)
    }

    fn paragraph_inlines(patch: TranslationPatch) -> Vec<Inline> {
        match patch {
            TranslationPatch::Block { block, .. } => match block {
                Block::Paragraph { inlines } => inlines,
                other => panic!("{other:?}"),
            },
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn missing_link_placeholder_drops_the_anchor_and_keeps_text() {
        let (doc, plan) = link_plan();
        let inlines = paragraph_inlines(plan.patch(&doc, 0, "见文档。").unwrap());
        assert_eq!(inlines, vec![Inline::text("见文档。")]);
    }

    #[test]
    fn duplicate_placeholder_keeps_the_first_and_the_extra_text() {
        let (doc, plan) = link_plan();
        let inlines = paragraph_inlines(plan.patch(&doc, 0, "<x0>一</x0><x0>二</x0>").unwrap());
        assert_eq!(
            inlines,
            vec![
                Inline::Link {
                    href: "https://example.test".into(),
                    children: vec![Inline::text("一")],
                },
                Inline::text("二"),
            ]
        );
    }

    #[test]
    fn raw_less_than_stays_text_and_the_link_still_restores() {
        let (doc, plan) = link_plan();
        let inlines = paragraph_inlines(plan.patch(&doc, 0, "a < b <x0>链接</x0>").unwrap());
        assert_eq!(
            inlines,
            vec![
                Inline::text("a < b "),
                Inline::Link {
                    href: "https://example.test".into(),
                    children: vec![Inline::text("链接")],
                },
            ]
        );
    }

    #[test]
    fn spaced_placeholder_tags_still_restore_the_link() {
        let (doc, plan) = link_plan();
        let inlines = paragraph_inlines(plan.patch(&doc, 0, "<x0 >链接</ x0>").unwrap());
        assert_eq!(
            inlines,
            vec![Inline::Link {
                href: "https://example.test".into(),
                children: vec![Inline::text("链接")],
            }]
        );
    }

    #[test]
    fn missing_code_placeholder_is_appended_after_a_space() {
        let doc = Document::new(vec![para(vec![
            Inline::text("Run "),
            Inline::Code {
                text: "cargo test".into(),
            },
            Inline::text(" now"),
        ])]);
        let plan = TranslationPlan::new("", &doc);
        let inlines = paragraph_inlines(plan.patch(&doc, 0, "运行一下").unwrap());
        assert_eq!(
            inlines,
            vec![
                Inline::text("运行一下 "),
                Inline::Code {
                    text: "cargo test".into(),
                },
            ]
        );
    }

    #[test]
    fn roundtrip_keeps_link_href_and_skips_code() {
        let doc = Document {
            v: IR_VERSION,
            blocks: vec![
                para(vec![
                    Inline::text("Hello "),
                    Inline::Strong {
                        children: vec![Inline::text("world")],
                    },
                    Inline::text(" and "),
                    Inline::Link {
                        href: "https://ex.test/a".into(),
                        children: vec![Inline::text("link")],
                    },
                    Inline::text(" plus "),
                    Inline::Code {
                        text: "x = 1".into(),
                    },
                    Inline::text("."),
                ]),
                Block::CodeBlock {
                    lang: Some("rs".into()),
                    text: "fn main() {}".into(),
                },
                Block::Image {
                    src: "https://ex.test/p.png".into(),
                    alt: "cover photo".into(),
                },
            ],
        };
        let units = extract_units("A title", &doc);
        assert_eq!(units.len(), 3);
        assert_eq!(units[0].t, "A title");
        assert!(units[1].t.contains("<x0>world</x0>"));
        assert!(units[1].t.contains("<x1>link</x1>"));
        assert!(units[1].t.contains("<x2/>"));
        assert!(!units.iter().any(|u| u.t.contains("fn main")));
        assert_eq!(units[2].t, "cover photo");

        let translated = vec![
            TranslateUnit {
                i: 0,
                t: "一个标题".into(),
            },
            TranslateUnit {
                i: 1,
                t: units[1]
                    .t
                    .replace("Hello ", "你好 ")
                    .replace("world", "世界")
                    .replace(" and ", " 和 ")
                    .replace("link", "链接")
                    .replace(" plus ", " 以及 ")
                    .replace(".", "。"),
            },
            TranslateUnit {
                i: 2,
                t: "封面照片".into(),
            },
        ];
        let (title, out) = apply_units("A title", &doc, &translated);
        assert_eq!(title, "一个标题");
        match &out.blocks[0] {
            Block::Paragraph { inlines } => {
                let href = inlines.iter().find_map(|n| match n {
                    Inline::Link { href, children } => Some((href.as_str(), children)),
                    _ => None,
                });
                let (href, children) = href.expect("link");
                assert_eq!(href, "https://ex.test/a");
                assert_eq!(children, &vec![Inline::text("链接")]);
                assert!(
                    inlines
                        .iter()
                        .any(|n| matches!(n, Inline::Code { text } if text == "x = 1"))
                );
            }
            other => panic!("{other:?}"),
        }
        match &out.blocks[1] {
            Block::CodeBlock { text, .. } => assert_eq!(text, "fn main() {}"),
            other => panic!("{other:?}"),
        }
        match &out.blocks[2] {
            Block::Image { src, alt } => {
                assert_eq!(src, "https://ex.test/p.png");
                assert_eq!(alt, "封面照片");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn mangled_tags_fall_back_to_plain() {
        let doc = Document {
            v: IR_VERSION,
            blocks: vec![para(vec![
                Inline::Strong {
                    children: vec![Inline::text("Hi")],
                },
                Inline::text(" there"),
            ])],
        };
        let units = extract_units("", &doc);
        let (title, out) = apply_units(
            "",
            &doc,
            &[TranslateUnit {
                i: 0,
                t: "你好那里".into(),
            }],
        );
        assert!(title.is_empty());
        match &out.blocks[0] {
            Block::Paragraph { inlines } => {
                assert_eq!(inlines, &vec![Inline::text("你好那里")]);
            }
            other => panic!("{other:?}"),
        }
        let _ = units;
    }

    #[test]
    fn legacy_numeric_tags_still_restore_markup() {
        let doc = Document {
            v: IR_VERSION,
            blocks: vec![para(vec![
                Inline::Link {
                    href: "https://ex.test/a".into(),
                    children: vec![Inline::text("link")],
                },
                Inline::text(" and "),
                Inline::Strong {
                    children: vec![Inline::text("hi")],
                },
            ])],
        };
        let (title, out) = apply_units(
            "",
            &doc,
            &[TranslateUnit {
                i: 0,
                t: "<0>链接</0> 和 <1>嗨</1>".into(),
            }],
        );
        assert!(title.is_empty());
        match &out.blocks[0] {
            Block::Paragraph { inlines } => {
                let href = inlines.iter().find_map(|n| match n {
                    Inline::Link { href, children } => Some((href.as_str(), children)),
                    _ => None,
                });
                let (href, children) = href.expect("link");
                assert_eq!(href, "https://ex.test/a");
                assert_eq!(children, &vec![Inline::text("链接")]);
                assert!(
                    inlines
                        .iter()
                        .any(|n| matches!(n, Inline::Strong { children } if children == &vec![Inline::text("嗨")]))
                );
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn skips_mostly_han_for_zh_target() {
        let plain = "新图书馆在市中心开放，周末有故事会和志愿者帮忙还书。";
        assert!(already_target_lang(plain, "zh"));
        assert!(!already_target_lang(
            "The new library opened downtown this weekend.",
            "zh"
        ));
    }

    #[test]
    fn hard_break_extracts_as_space_not_tag() {
        let doc = Document {
            v: IR_VERSION,
            blocks: vec![para(vec![
                Inline::text("Hello"),
                Inline::HardBreak,
                Inline::text("world"),
            ])],
        };
        let units = extract_units("", &doc);
        assert_eq!(units.len(), 1);
        assert_eq!(units[0].t, "Hello world");
        assert!(!units[0].t.contains('<'), "{}", units[0].t);

        let (title, out) = apply_units(
            "",
            &doc,
            &[TranslateUnit {
                i: 0,
                t: "你好世界".into(),
            }],
        );
        assert!(title.is_empty());
        match &out.blocks[0] {
            Block::Paragraph { inlines } => {
                assert_eq!(inlines, &vec![Inline::text("你好世界")]);
                assert!(!inlines.iter().any(|n| matches!(n, Inline::HardBreak)));
            }
            other => panic!("{other:?}"),
        }
    }
}
