//! 超长单元按句拆片与拼回，只服务模型翻译。
//!
//! 只在占位嵌套深度为 0 处切开，绝不切进 `<xN>…</xN>`；各片占位编号按出现顺序归一为 0 起始，
//! 译后用 [`restore_segment_tags`] 一次扫描还原，再用 [`join_translated_segments`] 拼回整单元，
//! 交给 [`super::TranslationPlan::patch`] 校验嵌回。块与单元仍一一对应。

use super::{is_han, match_tag, plain_from_tagged};

/// 一个单元最多拆出的片数；超长输入把每片字数放大到能装下，不拆出成百上千片。
pub const MAX_UNIT_SEGMENTS: usize = 32;

/// 超长单元的一片。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaggedSegment {
    /// 本片文本，占位编号已归一为 0 起始。
    pub text: String,
    /// 局部占位编号 → 原单元占位编号。
    pub tag_map: Vec<usize>,
    /// 原文在本片之后的切点有空白，拼回时据此决定是否留空格。
    pub trailing_space: bool,
}

impl TaggedSegment {
    /// 译文的占位是否与本片原文逐一对应（每个编号恰好一次、类型一致、嵌套闭合）且有可见文字。
    /// 只用于判定能否写入片缓存；不满足的译文仍可交给整单元的宽松还原。
    pub fn accepts(&self, translated: &str) -> bool {
        let (Some(source), Some(output)) = (tokenize(&self.text), tokenize(translated)) else {
            return false;
        };
        let kinds = |tokens: &[Token]| {
            let mut kinds = vec![None; self.tag_map.len()];
            for token in tokens {
                let (id, void) = match token.kind {
                    Kind::Open(id) => (id, false),
                    Kind::Void(id) => (id, true),
                    _ => continue,
                };
                match kinds.get_mut(id) {
                    Some(slot @ None) => *slot = Some(void),
                    _ => return None,
                }
            }
            Some(kinds)
        };
        let has_text = output
            .iter()
            .any(|token| matches!(token.kind, Kind::Char(c) if !c.is_whitespace()));
        has_text
            && kinds(&source).is_some_and(|source| {
                source.iter().all(Option::is_some) && kinds(&output) == Some(source)
            })
    }

    /// 译文至少包含可见文字，且所有成对占位标签在片内正确闭合（不把未闭合的链接或粗体泄漏到下一片）。
    /// 不要求所有占位都无损保留；仅用于无法继续细分时决定能否安全交给整单元的宽松还原。
    pub fn joinable(&self, translated: &str) -> bool {
        let mut stack = Vec::new();
        let mut has_text = false;
        let mut rest = translated;
        while let Some(at) = rest.find('<') {
            if rest[..at].chars().any(|c| !c.is_whitespace()) {
                has_text = true;
            }
            rest = &rest[at..];
            let Some((kind, id, len)) = match_tag(rest) else {
                // 非占位 `<`（例如模型输出了未转义的 `a < b`）按可见文字计，不影响标签栈。
                has_text = true;
                rest = &rest[1..];
                continue;
            };
            match kind {
                super::LenientTag::Open => stack.push(id),
                super::LenientTag::Close => {
                    if stack.pop() != Some(id) {
                        return false;
                    }
                }
                super::LenientTag::Void => {}
            }
            rest = &rest[len..];
        }
        if rest.chars().any(|c| !c.is_whitespace()) {
            has_text = true;
        }
        has_text && stack.is_empty()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Char(char),
    Open(usize),
    Close(usize),
    Void(usize),
}

#[derive(Debug, Clone, Copy)]
struct Token {
    start: usize,
    end: usize,
    kind: Kind,
}

impl Token {
    fn width(&self, text: &str) -> usize {
        match self.kind {
            Kind::Char(_) => 1,
            _ => text[self.start..self.end].chars().count(),
        }
    }
    fn char(&self) -> Option<char> {
        match self.kind {
            Kind::Char(c) => Some(c),
            _ => None,
        }
    }
}

/// 只认本模块写出的 `<xN>`、`</xN>`、`<xN/>`。
fn strict_tag(s: &str) -> Option<(Kind, usize)> {
    let bytes = s.as_bytes();
    let close = bytes.get(1) == Some(&b'/');
    let mut i = if close { 2 } else { 1 };
    if bytes.get(i) != Some(&b'x') {
        return None;
    }
    i += 1;
    let digits = i;
    while bytes.get(i).is_some_and(u8::is_ascii_digit) {
        i += 1;
    }
    if i == digits {
        return None;
    }
    let id = s[digits..i].parse().ok()?;
    match (close, bytes.get(i), bytes.get(i + 1)) {
        (false, Some(b'/'), Some(b'>')) => Some((Kind::Void(id), i + 2)),
        (false, Some(b'>'), _) => Some((Kind::Open(id), i + 1)),
        (true, Some(b'>'), _) => Some((Kind::Close(id), i + 1)),
        _ => None,
    }
}

/// 切成字符与占位；遇到不认识的 `<` 或未闭合 / 交错的标签返回 `None`，调用方保守不拆。
fn tokenize(text: &str) -> Option<Vec<Token>> {
    let mut tokens = Vec::new();
    let mut stack = Vec::new();
    let mut at = 0;
    while let Some(c) = text[at..].chars().next() {
        if c == '<' {
            let (kind, len) = strict_tag(&text[at..])?;
            match kind {
                Kind::Open(id) => stack.push(id),
                Kind::Close(id) if stack.pop() != Some(id) => return None,
                _ => {}
            }
            tokens.push(Token {
                start: at,
                end: at + len,
                kind,
            });
            at += len;
        } else {
            let end = at + c.len_utf8();
            tokens.push(Token {
                start: at,
                end,
                kind: Kind::Char(c),
            });
            at = end;
        }
    }
    stack.is_empty().then_some(tokens)
}

const SENTENCE_CJK: &[char] = &['。', '！', '？', '…'];
const SENTENCE_ASCII: &[char] = &['.', '!', '?'];
const CLAUSE_CJK: &[char] = &['；', '：', '，', '、'];
const CLAUSE_ASCII: &[char] = &[';', ','];
/// 句末标点后紧随的右引号 / 括号，切点放在它们之后。
const CLOSERS: &[char] = &[
    '”', '’', '」', '』', '）', '》', '〉', '】', '〕', '"', '\'', ')', ']',
];
const ABBREVIATIONS: &[&str] = &[
    "e.g.", "i.e.", "etc.", "mr.", "mrs.", "ms.", "dr.", "prof.", "vs.", "u.s.", "u.k.", "st.",
    "jr.", "sr.", "no.", "fig.", "cf.", "approx.",
];

/// 从 `k` 往前跳过闭合标签和右引号，返回标点所在的字符位置。
fn punct_before(tokens: &[Token], mut k: usize) -> Option<usize> {
    loop {
        match tokens[k].kind {
            Kind::Close(_) => {}
            Kind::Char(c) if CLOSERS.contains(&c) => {}
            Kind::Char(_) => return Some(k),
            _ => return None,
        }
        k = k.checked_sub(1)?;
    }
}

/// 英文句点前是常见缩写或单个大写首字母（`J. Smith`）时不是句末。
fn is_abbreviation(tokens: &[Token], dot: usize) -> bool {
    let mut word: Vec<char> = vec!['.'];
    let mut k = dot;
    while let Some(prev) = k.checked_sub(1) {
        match tokens[prev].char() {
            Some(c) if c.is_alphabetic() || c == '.' => word.push(c),
            _ => break,
        }
        k = prev;
    }
    word.reverse();
    let word: String = word.into_iter().collect::<String>().to_lowercase();
    let mut letters = word.chars().filter(|c| c.is_alphabetic());
    let initial = matches!((letters.next(), letters.next()), (Some(_), None))
        && word.chars().count() == 2
        && tokens[dot - 1].char().is_some_and(char::is_uppercase);
    initial || ABBREVIATIONS.contains(&word.as_str())
}

/// 正文里的 `<` 与 `&` 在进翻译前转义为 `&lt;` / `&amp;`；它们末尾的分号不是分句标点。
fn is_entity_semicolon(tokens: &[Token], semi: usize) -> bool {
    let matches_suffix = |entity: &[char]| {
        semi + 1 >= entity.len()
            && entity.iter().enumerate().all(|(offset, expected)| {
                tokens[semi + 1 - entity.len() + offset].char() == Some(*expected)
            })
    };
    matches_suffix(&['&', 'l', 't', ';']) || matches_suffix(&['&', 'a', 'm', 'p', ';'])
}

/// 切在第 `k` 个记号之前的优先级：1 句末、2 分句或换行、3 普通空白；`None` 表示不能切。
/// 调用方保证该处嵌套深度为 0。
fn cut_level(tokens: &[Token], k: usize) -> Option<u8> {
    let next = tokens[k].char();
    let prev = punct_before(tokens, k - 1).and_then(|at| Some((at, tokens[at].char()?)));
    if next.is_some_and(char::is_whitespace) {
        // 只切在空白段开头。
        if tokens[k - 1].char().is_some_and(char::is_whitespace) {
            return None;
        }
        return Some(match prev {
            Some((_, c)) if SENTENCE_CJK.contains(&c) => 1,
            Some((at, c)) if SENTENCE_ASCII.contains(&c) => {
                if c == '.' && is_abbreviation(tokens, at) {
                    3
                } else {
                    1
                }
            }
            Some((at, c))
                if CLAUSE_CJK.contains(&c)
                    || (CLAUSE_ASCII.contains(&c)
                        && !(c == ';' && is_entity_semicolon(tokens, at))) =>
            {
                2
            }
            _ if next == Some('\n') => 2,
            _ => 3,
        });
    }
    // 中日文标点后不需要空白，但不能把连续标点或右引号切开。
    if next.is_some_and(|c| {
        CLOSERS.contains(&c) || SENTENCE_CJK.contains(&c) || CLAUSE_CJK.contains(&c)
    }) {
        return None;
    }
    match prev {
        Some((_, c)) if SENTENCE_CJK.contains(&c) => Some(1),
        Some((_, c)) if CLAUSE_CJK.contains(&c) => Some(2),
        _ => None,
    }
}

fn single(text: &str) -> Vec<TaggedSegment> {
    let mut tag_map = Vec::new();
    if let Some(tokens) = tokenize(text) {
        for token in tokens {
            if let Kind::Open(id) | Kind::Void(id) = token.kind {
                tag_map.push(id);
            }
        }
    }
    vec![TaggedSegment {
        text: text.into(),
        tag_map,
        trailing_space: false,
    }]
}

/// 把超过 `max_chars`（按字符计，含占位）的单元按句 / 分句 / 空白切成若干片。
///
/// 每片贪心取到不超过上限的最靠右合法切点，句末优先于分句，分句优先于空白；
/// 上限内找不到切点（整段被一个链接包住、无空白长串）时推进到下一个合法切点。
/// 不需要拆、标签畸形或整段没有切点时原样返回单片。
pub fn split_tagged_unit(text: &str, max_chars: usize) -> Vec<TaggedSegment> {
    let total = text.chars().count();
    if max_chars == 0 || total <= max_chars {
        return single(text);
    }
    let Some(tokens) = tokenize(text) else {
        return single(text);
    };
    let max_chars = max_chars.max(total.div_ceil(MAX_UNIT_SEGMENTS));
    let len = tokens.len();
    // chars[k]：前 k 个记号的字符数；words[k]：其中非空白字符数，用于不切出没有文字的片。
    let mut chars = vec![0; len + 1];
    let mut words = vec![0; len + 1];
    let mut depth = 0usize;
    let mut levels = vec![None; len + 1];
    for (k, token) in tokens.iter().enumerate() {
        if k > 0 && depth == 0 {
            levels[k] = cut_level(&tokens, k);
        }
        match token.kind {
            Kind::Open(_) => depth += 1,
            Kind::Close(_) => depth -= 1,
            _ => {}
        }
        chars[k + 1] = chars[k] + token.width(text);
        words[k + 1] = words[k] + usize::from(token.char().is_some_and(|c| !c.is_whitespace()));
    }
    let mut ranges = Vec::new();
    let mut start = 0;
    while chars[len] - chars[start] > max_chars && ranges.len() + 1 < MAX_UNIT_SEGMENTS {
        let limit = chars[start] + max_chars;
        let usable =
            |k: &usize| levels[*k].is_some() && words[*k] > words[start] && words[len] > words[*k];
        let mut best: Option<(u8, usize)> = None;
        for k in (start + 1..len).filter(usable) {
            if chars[k] > limit {
                break;
            }
            let level = levels[k].unwrap();
            if best.is_none_or(|(current, _)| level <= current) {
                best = Some((level, k));
            }
        }
        let Some(cut) = best
            .map(|(_, k)| k)
            .or_else(|| (start + 1..len).find(|k| usable(k)))
        else {
            break;
        };
        let mut next = cut;
        while tokens
            .get(next)
            .and_then(Token::char)
            .is_some_and(char::is_whitespace)
        {
            next += 1;
        }
        ranges.push((start, cut, next > cut));
        start = next;
    }
    if ranges.is_empty() {
        return single(text);
    }
    ranges.push((start, len, false));
    ranges
        .into_iter()
        .map(|(from, to, trailing_space)| {
            let mut tag_map = Vec::new();
            let mut out = String::new();
            for token in &tokens[from..to] {
                let local = |tag_map: &mut Vec<usize>, id: usize| {
                    tag_map.iter().position(|v| *v == id).unwrap_or_else(|| {
                        tag_map.push(id);
                        tag_map.len() - 1
                    })
                };
                match token.kind {
                    Kind::Char(_) => out.push_str(&text[token.start..token.end]),
                    Kind::Open(id) => {
                        let id = local(&mut tag_map, id);
                        super::push_open(&mut out, id);
                    }
                    Kind::Close(id) => {
                        let id = local(&mut tag_map, id);
                        super::push_close(&mut out, id);
                    }
                    Kind::Void(id) => {
                        let id = local(&mut tag_map, id);
                        super::push_void(&mut out, id);
                    }
                }
            }
            TaggedSegment {
                text: out,
                tag_map,
                trailing_space,
            }
        })
        .collect()
}

/// 把片译文里的局部占位编号一次扫描换回原编号；不能链式 `replace`，否则 `<x0>→<x1>` 会再被换成 `<x2>`。
/// 不认识的局部编号原样保留，交给整单元校验拒绝。
pub fn restore_segment_tags(translated: &str, tag_map: &[usize]) -> String {
    let mut out = String::with_capacity(translated.len());
    let mut rest = translated;
    while let Some(at) = rest.find('<') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let Some((kind, id, len)) = match_tag(rest) else {
            out.push('<');
            rest = &rest[1..];
            continue;
        };
        match tag_map.get(id) {
            Some(original) => match kind {
                super::LenientTag::Open => super::push_open(&mut out, *original),
                super::LenientTag::Close => super::push_close(&mut out, *original),
                super::LenientTag::Void => super::push_void(&mut out, *original),
            },
            None => out.push_str(&rest[..len]),
        }
        rest = &rest[len..];
    }
    out.push_str(rest);
    out
}

fn is_cjk(c: char) -> bool {
    is_han(c)
        || matches!(
            c,
            '\u{3000}'..='\u{30FF}' | '\u{FF00}'..='\u{FFEF}' | '\u{3400}'..='\u{4DBF}'
        )
}

fn cjk_target(lang: &str) -> bool {
    let lang = lang.trim().to_ascii_lowercase();
    ["zh", "ja", "cmn"].iter().any(|p| {
        lang == *p || lang.starts_with(&format!("{p}-")) || lang.starts_with(&format!("{p}_"))
    })
}

/// 按目标语拼回各片译文。中文 / 日文：左片以中日文标点结尾或右片以中日文字符开头时直接相接，
/// 其余原文切点有空白时留一个空格。其它语言：原文切点有空白，或两侧都不是中日文字符时留一个空格。
pub fn join_translated_segments(
    segments: &[String],
    trailing_spaces: &[bool],
    target_lang: &str,
) -> String {
    let cjk = cjk_target(target_lang);
    let mut out = String::new();
    for (i, segment) in segments.iter().enumerate() {
        let segment = segment.trim();
        if segment.is_empty() {
            continue;
        }
        if !out.is_empty() {
            let spaced = i
                .checked_sub(1)
                .and_then(|prev| trailing_spaces.get(prev))
                .copied()
                .unwrap_or(false);
            let left = plain_from_tagged(&out).trim_end().chars().last();
            let right = plain_from_tagged(segment).trim_start().chars().next();
            let space = if cjk {
                let tight = left.is_some_and(|c| {
                    SENTENCE_CJK.contains(&c) || CLAUSE_CJK.contains(&c) || "”’」』）》".contains(c)
                }) || right.is_some_and(is_cjk);
                spaced && !tight
            } else {
                spaced || !(left.is_some_and(is_cjk) || right.is_some_and(is_cjk))
            };
            if space {
                out.push(' ');
            }
        }
        out.push_str(segment);
    }
    out
}

#[cfg(test)]
mod tests;
