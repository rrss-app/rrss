//! 块 IR 结构化摘录：给摘要和简报成稿准备正文输入，不调用模型。
//! 日志 target：`rrss_core::summary_extract`。
//!
//! 正文在预算内就整篇给出；超出时按优先级挑**完整段落**（标题、导语、结尾、各节首段、
//! 信息密度高的段落），按原文顺序拼回，省略处用「…」标出。预算由 [`InputTier`] 决定。

use crate::text::truncate_chars;

use rrss_render::{Block, Document, inlines_to_plain};

/// 设置里的「输入档位」：每次送给模型多少正文。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InputTier {
    Low,
    #[default]
    Medium,
    High,
}

impl InputTier {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim() {
            "low" => Some(Self::Low),
            "medium" => Some(Self::Medium),
            "high" => Some(Self::High),
            _ => None,
        }
    }

    /// 单篇摘要的输入上限（字），高分或收藏的文章用较大的一档。
    pub fn summary_budget(self, high_priority: bool) -> usize {
        match (self, high_priority) {
            (Self::Low, false) => 2_000,
            (Self::Low, true) => 3_000,
            (Self::Medium, false) => 4_000,
            (Self::Medium, true) => 6_000,
            (Self::High, false) => 10_000,
            (Self::High, true) => 16_000,
        }
    }

    /// 简报成稿时每篇正文摘录的上限（字）。
    pub fn digest_item_chars(self) -> usize {
        match self {
            Self::Low => 1_500,
            Self::Medium => 3_000,
            Self::High => 6_000,
        }
    }

    /// 简报成稿时全部摘录的总上限（字），按入选篇数平分。
    pub fn digest_total_chars(self) -> usize {
        match self {
            Self::Low => 15_000,
            Self::Medium => 30_000,
            Self::High => 60_000,
        }
    }
}

/// `summary_json` 里记录摘要所依据正文的字段。
pub const BODY_SIG_KEY: &str = "body_sig";

/// 摘要所依据正文的签名：优先块 IR，没有时用原始 HTML。抓回全文会替换块 IR，签名随之变化。
pub fn body_signature(blocks_json: Option<&str>, content_html: Option<&str>) -> String {
    crate::body::signature(blocks_json, content_html)
}

/// 摘要是否来自旧正文。旧摘要没有签名时无法判断，不算过期。
pub fn summary_is_stale(
    summary_json: Option<&str>,
    blocks_json: Option<&str>,
    content_html: Option<&str>,
) -> bool {
    let Some(sig) = summary_json
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
        .and_then(|v| {
            v.get(BODY_SIG_KEY)
                .and_then(|s| s.as_str())
                .map(ToString::to_string)
        })
    else {
        return false;
    };
    sig != body_signature(blocks_json, content_html)
}

/// 标题最多保留的字数。
const HEADING_CHARS: usize = 100;
/// 省略标记。
const GAP: &str = "…";
/// 每千单位信号数达到这个值算「数字密集」，与各节首段同级。
const DENSE: usize = 40;
/// 单段截断时，剩余空间至少这么多加权单位才放入截断后的开头。
const MIN_PARTIAL_CHARS: usize = 160;
/// 允许低优先级保留的简短代码块上限：过滤大段源码 / 日志，只保留关键命令、签名与配置片段。
const SHORT_CODE_MAX_LINES: usize = 12;
const SHORT_CODE_MAX_UNITS: usize = 240;

/// 判断字符是否属于 CJK / 全角表意文字（按约 1 单位计；ASCII / 拉丁按 0.4 单位计，使中英文在同一档位下 token 成本接近）。
fn is_cjk(c: char) -> bool {
    matches!(
        c,
        '\u{3000}'..='\u{303F}'
            | '\u{3040}'..='\u{30FF}'
            | '\u{3400}'..='\u{4DBF}'
            | '\u{4E00}'..='\u{9FFF}'
            | '\u{AC00}'..='\u{D7AF}'
            | '\u{F900}'..='\u{FAFF}'
            | '\u{FF00}'..='\u{FFEF}'
    )
}

fn char_weight_x10(c: char) -> usize {
    if is_cjk(c) { 10 } else { 4 }
}

/// 按中英文加权的长度单位（纯中文约等于字数，纯英文约 2.5 字符折算 1 单位，与主流分词器 token 比例接近）。
pub fn weighted_len(text: &str) -> usize {
    let sum: usize = text.chars().map(char_weight_x10).sum();
    sum.div_ceil(10)
}

#[derive(Debug, Clone)]
enum Item {
    Heading(String),
    Paragraph(String),
    Code(String),
    Table(String),
}

impl Item {
    fn text(&self) -> &str {
        match self {
            Self::Heading(s) | Self::Paragraph(s) | Self::Code(s) | Self::Table(s) => s,
        }
    }

    fn is_heading(&self) -> bool {
        matches!(self, Self::Heading(_))
    }
}

/// 单篇摘要输入：标题 + 正文摘录，总长不超过档位预算。
pub fn extract_summary_payload(
    title: &str,
    blocks_json: Option<&str>,
    content_html: Option<&str>,
    high_priority: bool,
    tier: InputTier,
) -> String {
    let budget = tier.summary_budget(high_priority);
    let title = title.trim();
    let items = match parse_document(blocks_json, content_html) {
        Some(doc) if document_is_link_stub(&doc) => return link_stub_payload(title),
        Some(doc) => items_from_document(&doc),
        None => {
            let plain = crate::score::strip_html(content_html.unwrap_or(""));
            if plain_is_link_stub(&plain) {
                return link_stub_payload(title);
            }
            items_from_plain(&plain)
        }
    };
    if items.is_empty() {
        return title.to_string();
    }
    let body_budget = budget.saturating_sub(weighted_len(title) + 2);
    let body = select_to_budget(items, body_budget);
    if title.is_empty() {
        body
    } else {
        format!("{title}\n\n{body}")
    }
}

/// 手动「本篇摘要」分段压缩时最多切几段。
pub const MAX_SEGMENTS: usize = 5;

/// 手动摘要的长文分段：正文放得下一次预算（含 15% 容忍缓冲）、或只有链接时返回 `None`，照常一次摘要。
/// 否则按原文顺序切成至多 [`MAX_SEGMENTS`] 段，每段由完整段落组成、各自不超过 `budget`；
/// 比「段数 × 预算」还长的文章，每段内部再按同样的优先级挑段落。
pub fn split_segments(
    blocks_json: Option<&str>,
    content_html: Option<&str>,
    budget: usize,
) -> Option<Vec<String>> {
    let items = match parse_document(blocks_json, content_html) {
        Some(doc) if document_is_link_stub(&doc) => return None,
        Some(doc) => items_from_document(&doc),
        None => {
            let plain = crate::score::strip_html(content_html.unwrap_or(""));
            if plain_is_link_stub(&plain) {
                return None;
            }
            items_from_plain(&plain)
        }
    };
    let cost = |it: &Item| weighted_len(it.text()) + 2;
    let total: usize = items.iter().map(cost).sum();
    // 略微超出单次预算（<= 15%）时不值得拆成多段增加 3 倍调用，交由单次挑段处理。
    if budget == 0 || total <= budget + budget / 6 || items.iter().all(Item::is_heading) {
        return None;
    }
    let count = total.div_ceil(budget).clamp(2, MAX_SEGMENTS);
    let target = total.div_ceil(count);
    let mut groups: Vec<Vec<Item>> = vec![Vec::new()];
    let mut filled = 0usize;
    for item in items {
        let len = cost(&item);
        let group_count = groups.len();
        let last_non_empty = groups.last().is_some_and(|g| !g.is_empty());
        // 满了就另起一段；若上一段末尾有连续孤立小标题，一并挪到新段开头跟着正文走。
        if last_non_empty && filled + len > target && group_count < count {
            let mut moved_headings = Vec::new();
            if let Some(last) = groups.last_mut() {
                while last.iter().any(|it| !it.is_heading())
                    && last.last().is_some_and(Item::is_heading)
                {
                    moved_headings.push(last.pop().expect("trailing heading"));
                }
            }
            moved_headings.reverse();
            let next_filled: usize = moved_headings.iter().map(cost).sum();
            groups.push(moved_headings);
            filled = next_filled;
        }
        filled += len;
        groups.last_mut().expect("at least one group").push(item);
    }
    let segments: Vec<String> = groups
        .into_iter()
        .filter(|g| g.iter().any(|it| !it.is_heading()))
        .map(|g| select_to_budget(g, budget))
        .collect();
    (segments.len() >= 2).then_some(segments)
}

/// 估算文章经清洗后的正文加权长度，供简报成稿做余额回收分配。
pub fn body_weighted_len(blocks_json: Option<&str>, content_html: Option<&str>) -> usize {
    let items = match parse_document(blocks_json, content_html) {
        Some(doc) if document_is_link_stub(&doc) => return weighted_len(LINK_STUB_NOTE),
        Some(doc) => items_from_document(&doc),
        None => {
            let plain = crate::score::strip_html(content_html.unwrap_or(""));
            if plain_is_link_stub(&plain) {
                return weighted_len(LINK_STUB_NOTE);
            }
            items_from_plain(&plain)
        }
    };
    if items.iter().all(Item::is_heading) {
        return 0;
    }
    items.iter().map(|it| weighted_len(it.text()) + 2).sum()
}

/// 简报成稿用的正文摘录，不含标题（成稿输入已单独给出）。只有链接的条目返回「无正文」说明。
pub fn extract_body_excerpt(
    blocks_json: Option<&str>,
    content_html: Option<&str>,
    budget: usize,
) -> String {
    let items = match parse_document(blocks_json, content_html) {
        Some(doc) if document_is_link_stub(&doc) => return LINK_STUB_NOTE.to_string(),
        Some(doc) => items_from_document(&doc),
        None => {
            let plain = crate::score::strip_html(content_html.unwrap_or(""));
            if plain_is_link_stub(&plain) {
                return LINK_STUB_NOTE.to_string();
            }
            items_from_plain(&plain)
        }
    };
    // 简报只看正文，孤立的标题没有意义。
    if items.iter().all(Item::is_heading) {
        return String::new();
    }
    select_to_budget(items, budget)
}

fn parse_document(blocks_json: Option<&str>, content_html: Option<&str>) -> Option<Document> {
    crate::body::generation_document(blocks_json, content_html)
}

fn items_from_document(doc: &Document) -> Vec<Item> {
    let mut items = Vec::new();
    for block in &doc.blocks {
        match block {
            Block::Heading { inlines, .. } => {
                let text = inlines_to_plain(inlines).trim().to_string();
                if !text.is_empty() {
                    items.push(Item::Heading(format!(
                        "### {}",
                        truncate_chars(&text, HEADING_CHARS)
                    )));
                }
            }
            Block::Paragraph { inlines } | Block::Quote { inlines } => {
                let text = inlines_to_plain(inlines).trim().to_string();
                if !text.is_empty() && !is_boilerplate(&text) {
                    items.push(Item::Paragraph(text));
                }
            }
            Block::ListItem { inlines, .. } => {
                let text = inlines_to_plain(inlines).trim().to_string();
                if !text.is_empty() && !is_boilerplate(&text) {
                    items.push(Item::Paragraph(format!("- {text}")));
                }
            }
            Block::Table { .. } => {
                let text = rrss_render::block_plain(block);
                if !text.trim().is_empty() {
                    items.push(Item::Table(text));
                }
            }
            Block::CodeBlock { lang, text } => {
                let trimmed = text.trim();
                let line_count = trimmed.lines().count();
                if !trimmed.is_empty()
                    && line_count <= SHORT_CODE_MAX_LINES
                    && weighted_len(trimmed) <= SHORT_CODE_MAX_UNITS
                {
                    let fence = lang.as_deref().map(str::trim).unwrap_or("");
                    items.push(Item::Code(format!("```{fence}\n{trimmed}\n```")));
                }
            }
            // 长代码块、图片、嵌入和分隔线不是正文。
            Block::Image { .. } | Block::Embed { .. } | Block::Rule => {}
        }
    }
    items
}

fn items_from_plain(plain: &str) -> Vec<Item> {
    plain
        .split("\n\n")
        .map(str::trim)
        .filter(|s| !s.is_empty() && !is_boilerplate(s))
        .map(|p| Item::Paragraph(p.to_string()))
        .collect()
}

/// 放得下就整篇；否则按优先级挑完整段落，按原文顺序拼回，跳过的地方插入「…」。
fn select_to_budget(items: Vec<Item>, budget: usize) -> String {
    let cost = |s: &str| weighted_len(s) + 2;
    let total: usize = items.iter().map(|it| cost(it.text())).sum();
    if total <= budget {
        return join(items.iter().map(Item::text));
    }

    let paragraphs: Vec<usize> = (0..items.len())
        .filter(|&i| matches!(items[i], Item::Paragraph(_)))
        .collect();
    let mut texts: Vec<String> = items.iter().map(|it| it.text().to_string()).collect();

    // 优先级：标题（合计不超过四分之一预算）→ 导语与结尾 → 各节首段 → 信息密度高的段落 → 其余按原文顺序。
    let mut order: Vec<(u8, usize)> = Vec::new();
    for (i, item) in items.iter().enumerate() {
        if item.is_heading() {
            order.push((0, i));
        }
    }
    if let (Some(&first), Some(&last)) = (paragraphs.first(), paragraphs.last()) {
        order.push((1, first));
        if last != first {
            order.push((1, last));
        }
    }
    // 各节首段和数字密集的段落排第 2 级；简短代码块排第 3 级（总占比封顶 15%，不挤占导语、结尾与高密度正文）；
    // 其余普通段落排第 4 级；表格排第 5 级。
    let mut rest: Vec<(u8, usize, usize)> = paragraphs
        .iter()
        .map(|&i| {
            let dense = density(&texts[i]);
            let section_first = i > 0 && items[i - 1].is_heading();
            (
                if section_first || dense >= DENSE {
                    2
                } else {
                    4
                },
                dense,
                i,
            )
        })
        .collect();
    rest.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)).then(a.2.cmp(&b.2)));
    let (high_body, low_body): (Vec<_>, Vec<_>) = rest.into_iter().partition(|(p, _, _)| *p == 2);
    order.extend(high_body.into_iter().map(|(p, _, i)| (p, i)));
    order.extend(
        items
            .iter()
            .enumerate()
            .filter_map(|(i, item)| matches!(item, Item::Code(_)).then_some((3, i))),
    );
    order.extend(low_body.into_iter().map(|(p, _, i)| (p, i)));
    // 表格排在正文之后，不挤占导语、结尾与高密度正文；剩余空间足够大时按整行保留表头与前若干行。
    order.extend(
        items
            .iter()
            .enumerate()
            .filter_map(|(i, item)| matches!(item, Item::Table(_)).then_some((5, i))),
    );

    // 给省略标记留一点余量。
    let limit = budget.saturating_sub(budget / 20);
    let heading_cap = budget / 4;
    let code_cap = (budget * 15 / 100).max(SHORT_CODE_MAX_UNITS.min(limit / 4));
    let body_reserve = if paragraphs.len() > 2 {
        limit / 5
    } else {
        0
    };
    let mut picked = vec![false; items.len()];
    let mut used = 0usize;
    let mut heading_used = 0usize;
    let mut code_used = 0usize;
    for (priority, i) in order {
        if picked[i] {
            continue;
        }
        let len = cost(&texts[i]);
        if priority == 0 {
            if heading_used + len > heading_cap || used + len > limit {
                continue;
            }
            heading_used += len;
        } else if priority == 3 {
            if code_used + len > code_cap || used + len > limit {
                continue;
            }
            code_used += len;
        } else if used + len > limit {
            let room = limit.saturating_sub(used + 2);
            if room < MIN_PARTIAL_CHARS {
                continue;
            }
            if priority == 1 {
                // 导语 / 结尾超预算时截断放入，但为中段高密度正文保留至少 20% 空间，避免结尾吃光余量。
                let cap = if room > body_reserve + MIN_PARTIAL_CHARS {
                    room - body_reserve
                } else {
                    room
                };
                texts[i] = cut_at_sentence(&texts[i], cap);
            } else if priority == 2 && len > limit {
                // 单独一段就超整个预算的各节首段，在句末截取放入剩余空间。
                texts[i] = cut_at_sentence(&texts[i], room);
            } else if priority == 5 {
                // 超大表格按整行截取，保留表头与前面的关键数据行。
                let Some(truncated) = cut_table_rows(&texts[i], room) else {
                    continue;
                };
                texts[i] = truncated;
            } else {
                continue;
            }
        }
        used += cost(&texts[i]);
        picked[i] = true;
    }

    // 去掉末尾没有任何正文的孤立小标题。
    let mut last_non_heading = None;
    for i in (0..items.len()).rev() {
        if picked[i] && !items[i].is_heading() {
            last_non_heading = Some(i);
            break;
        }
    }
    if let Some(last_body) = last_non_heading {
        for i in (last_body + 1)..items.len() {
            if items[i].is_heading() {
                picked[i] = false;
            }
        }
    }

    let mut out: Vec<&str> = Vec::new();
    let mut skipped = false;
    for (i, text) in texts.iter().enumerate() {
        if picked[i] {
            if skipped && !out.is_empty() {
                out.push(GAP);
            }
            skipped = false;
            out.push(text);
        } else if !items[i].is_heading() {
            skipped = true;
        }
    }
    if skipped {
        out.push(GAP);
    }
    join(out.into_iter())
}

fn join<'a>(parts: impl Iterator<Item = &'a str>) -> String {
    parts.collect::<Vec<_>>().join("\n\n")
}

/// 信息密度：数字、句中专名/缩写、引号与货币/百分号越多越靠前。按每千加权单位计。
fn density(text: &str) -> usize {
    let mut signals = 0usize;
    let mut prev_alnum = false;
    let mut in_digits = false;
    let mut at_sentence_start = true;
    for c in text.chars() {
        if c.is_ascii_digit() {
            if !in_digits {
                signals += 2;
            }
            in_digits = true;
            at_sentence_start = false;
        } else {
            in_digits = matches!(c, '.' | ',') && in_digits;
            if matches!(c, '.' | '!' | '?' | '。' | '！' | '？' | '\n') {
                at_sentence_start = true;
            } else if !c.is_whitespace() && !matches!(c, '"' | '\'' | '“' | '「' | '(' | '[') {
                if c.is_ascii_uppercase() && !prev_alnum && !at_sentence_start {
                    signals += 1;
                }
                at_sentence_start = false;
            }
        }
        if matches!(c, '%' | '“' | '「' | '"' | '$' | '¥' | '€') {
            signals += 1;
        }
        prev_alnum = c.is_alphanumeric();
    }
    signals * 1000 / (weighted_len(text) + 50)
}

/// 截到 `max_units` 加权单位以内，尽量落在句末；截断时补「…」。
fn cut_at_sentence(text: &str, max_units: usize) -> String {
    if weighted_len(text) <= max_units {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let keep_x10 = max_units.saturating_sub(2) * 10;
    let mut acc = 0usize;
    let mut keep = 0usize;
    for (i, &c) in chars.iter().enumerate() {
        acc += char_weight_x10(c);
        if acc > keep_x10 {
            break;
        }
        keep = i + 1;
    }
    let floor = keep / 2;
    let end = (floor..keep)
        .rev()
        .find(|&i| matches!(chars[i], '。' | '！' | '？' | '.' | '!' | '?' | '；' | ';'))
        .map_or(keep, |i| i + 1);
    let head: String = chars[..end].iter().collect();
    format!("{} …", head.trim_end())
}

/// 超大表格按整行截到 `max_units` 以内，至少保留 2 行（表头 + 数据行），末尾加「…」。
fn cut_table_rows(table: &str, max_units: usize) -> Option<String> {
    let limit = max_units.saturating_sub(3);
    let mut kept = Vec::new();
    let mut used = 0usize;
    for line in table.lines() {
        let len = weighted_len(line) + 1;
        if used + len > limit {
            break;
        }
        used += len;
        kept.push(line);
    }
    if kept.len() < 2 {
        return None;
    }
    kept.push(GAP);
    Some(kept.join("\n"))
}

/// 只有标题与外链的条目在模型输入里的说明（摘要与简报共用）。
pub const LINK_STUB_NOTE: &str = "[No body text] The feed only gives a title and an outbound link, with no article content. Write only facts already in the title; do not add technical details that are not in the original (such as drivers, power, benchmark scores, prices or compatibility lists).";

/// 判断正文是否只包含外链元数据（如 HN 的 Article URL / Comments URL / Points 卡片）。
pub fn is_link_stub(blocks_json: Option<&str>, content_html: Option<&str>) -> bool {
    match parse_document(blocks_json, content_html) {
        Some(doc) => document_is_link_stub(&doc),
        None => {
            let plain = crate::score::strip_html(content_html.unwrap_or(""));
            plain_is_link_stub(&plain)
        }
    }
}

/// 供简报选题目录使用的简短导语（无摘要文章不再复读标题）：纯链接返回 `[No body text]`，无正文返回空串。
pub fn extract_lead_snippet(
    blocks_json: Option<&str>,
    content_html: Option<&str>,
    max_chars: usize,
) -> String {
    if max_chars == 0 {
        return String::new();
    }
    let items = match parse_document(blocks_json, content_html) {
        Some(doc) if document_is_link_stub(&doc) => return "[No body text]".to_string(),
        Some(doc) => items_from_document(&doc),
        None => {
            let plain = crate::score::strip_html(content_html.unwrap_or(""));
            if plain_is_link_stub(&plain) {
                return "[No body text]".to_string();
            }
            items_from_plain(&plain)
        }
    };
    let Some(first) = items.into_iter().find_map(|item| match item {
        Item::Paragraph(text) => Some(text),
        _ => None,
    }) else {
        return String::new();
    };
    let single_line = first.split_whitespace().collect::<Vec<_>>().join(" ");
    if single_line.chars().count() <= max_chars {
        single_line
    } else {
        cut_at_sentence(&single_line, max_chars)
    }
}

fn link_stub_payload(title: &str) -> String {
    let title = title.trim();
    if title.is_empty() {
        LINK_STUB_NOTE.to_string()
    } else {
        format!("{title}\n\n{LINK_STUB_NOTE}")
    }
}

fn document_is_link_stub(doc: &Document) -> bool {
    let lines: Vec<String> = doc
        .blocks
        .iter()
        .filter_map(|b| match b {
            Block::Paragraph { inlines }
            | Block::Quote { inlines }
            | Block::ListItem { inlines, .. } => {
                let t = inlines_to_plain(inlines).trim().to_string();
                if t.is_empty() { None } else { Some(t) }
            }
            Block::Table { .. } => Some(rrss_render::block_plain(b)),
            _ => None,
        })
        .collect();
    lines_are_link_stub(&lines)
}

fn plain_is_link_stub(plain: &str) -> bool {
    let lines: Vec<String> = plain
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(ToString::to_string)
        .collect();
    lines_are_link_stub(&lines)
}

fn lines_are_link_stub(lines: &[String]) -> bool {
    if lines.len() < 2 {
        return false;
    }
    let mut meta = 0usize;
    let mut other = 0usize;
    for line in lines {
        if is_link_stub_line(line) {
            meta += 1;
        } else if !is_boilerplate(line) {
            other += 1;
        }
    }
    meta >= 2 && other == 0
}

fn is_link_stub_line(text: &str) -> bool {
    let lower = text.trim().to_lowercase();
    lower.starts_with("article url")
        || lower.starts_with("comments url")
        || lower.starts_with("points:")
        || lower.starts_with("points：")
        || lower.starts_with("# comments")
        || lower.starts_with("comments:")
        || (lower.starts_with("http://") || lower.starts_with("https://"))
            && lower.chars().filter(|c| c.is_whitespace()).count() < 2
}

fn is_boilerplate(text: &str) -> bool {
    let lower = text.trim().to_lowercase();
    let chars = lower.chars().count();
    if chars == 0 || (lower.is_ascii() && chars < 4) {
        return true;
    }
    lower.starts_with("copyright")
        || lower.starts_with("all rights reserved")
        || lower.starts_with("版权所有")
        || lower.starts_with("转载请注明")
        || lower.starts_with("责任编辑：")
        || lower.starts_with("本文来自")
        || lower.starts_with("follow us on")
        || lower.starts_with("subscribe to")
}

#[cfg(test)]
mod tests {
    use super::*;
    use rrss_render::{Block, Document, Inline};

    fn para(text: impl Into<String>) -> Block {
        Block::Paragraph {
            inlines: vec![Inline::text(text.into())],
        }
    }

    fn heading(text: impl Into<String>) -> Block {
        Block::Heading {
            level: 2,
            inlines: vec![Inline::text(text.into())],
        }
    }

    fn json(blocks: Vec<Block>) -> String {
        rrss_render::document_to_json(&Document::new(blocks)).unwrap()
    }

    #[test]
    fn article_within_budget_is_sent_whole() {
        // 15 段、约 3000 字：旧做法每段只留首尾约 60 字，现在整篇给出。
        let blocks: Vec<Block> = (1..=15)
            .map(|i| {
                para(format!(
                    "第 {i} 段开头。{}中间写着关键数字 {i}0 亿元。结尾。",
                    "铺垫文字".repeat(40)
                ))
            })
            .collect();
        let body_len: usize = blocks
            .iter()
            .map(|b| match b {
                Block::Paragraph { inlines } => inlines_to_plain(inlines).chars().count(),
                _ => 0,
            })
            .sum();
        assert!(body_len < InputTier::Medium.summary_budget(false));
        let out =
            extract_summary_payload("标题", Some(&json(blocks)), None, false, InputTier::Medium);
        for i in 1..=15 {
            assert!(
                out.contains(&format!("关键数字 {i}0 亿元")),
                "paragraph {i} missing"
            );
        }
        assert!(!out.contains(GAP));
    }

    #[test]
    fn tables_are_whole_low_priority_paragraphs() {
        let table =
            rrss_render::markdown_to_document("| 数值 | 比例 |\n| --- | --- |\n| 123456 | 98% |")
                .blocks
                .remove(0);
        let plain = rrss_render::block_plain(&table);
        let paragraphs = [
            "导语交代事件背景。",
            "正文补充完整细节。",
            "结尾说明未来发展。",
        ];
        let mut blocks = vec![table.clone()];
        blocks.extend(paragraphs.iter().map(|s| para(*s)));
        blocks.push(table);
        let source = json(blocks);
        let complete = extract_body_excerpt(Some(&source), None, 1_000);
        assert_eq!(
            complete,
            format!("{plain}\n\n{}\n\n{plain}", paragraphs.join("\n\n"))
        );
        let limited = extract_body_excerpt(Some(&source), None, 45);
        assert!(paragraphs.iter().all(|p| limited.contains(p)), "{limited}");
        assert!(
            !limited.contains("123456"),
            "tables must not displace body: {limited}"
        );
    }

    #[test]
    fn long_article_keeps_whole_paragraphs_in_priority_order() {
        let mut blocks = vec![para(format!(
            "导语：Acme 发布 X2 芯片。{}",
            "背景介绍。".repeat(30)
        ))];
        for i in 1..=12 {
            blocks.push(heading(format!("第 {i} 节")));
            blocks.push(para(format!("第 {i} 节首段。{}", "说明文字。".repeat(60))));
            blocks.push(para(format!("第 {i} 节补充。{}", "延伸讨论。".repeat(60))));
        }
        blocks.push(para("数据段：售价 $499，功耗降低 35%，首批 200 万片。"));
        blocks.push(para(format!("结尾：{}", "展望未来。".repeat(20))));
        let doc = json(blocks);
        let out = extract_summary_payload("长文", Some(&doc), None, false, InputTier::Medium);
        let budget = InputTier::Medium.summary_budget(false);
        assert!(
            out.chars().count() <= budget,
            "{} > {budget}",
            out.chars().count()
        );
        assert!(out.contains("导语：Acme 发布 X2 芯片"));
        assert!(out.contains("结尾："));
        assert!(out.contains("### 第 1 节") && out.contains("### 第 12 节"));
        assert!(out.contains("第 1 节首段"));
        assert!(
            out.contains("售价 $499，功耗降低 35%"),
            "dense paragraph should win over filler"
        );
        assert!(out.contains(GAP));
        // 选中的段落是完整的，不再有「首 40 字 … 尾 20 字」式的拼接。
        assert!(!out.contains(" … 说明文字"));

        let high = extract_summary_payload("长文", Some(&doc), None, false, InputTier::High);
        let low = extract_summary_payload("长文", Some(&doc), None, false, InputTier::Low);
        assert!(high.chars().count() > out.chars().count());
        assert!(low.chars().count() <= InputTier::Low.summary_budget(false));
        assert!(low.contains("导语：Acme") && low.contains("结尾："));
    }

    #[test]
    fn long_lead_that_fits_is_kept_whole_and_other_paragraphs_are_not_trimmed() {
        // 导语约占预算的一半：以前段落多时会先被截到三分之一。
        let lead = format!("导语开头。{}导语最后一句。", "导语展开。".repeat(180));
        let mut blocks = vec![para(lead.clone())];
        for i in 1..=6 {
            blocks.push(para(format!(
                "第 {i} 段开头。{}第 {i} 段结尾。",
                "正文内容。".repeat(90)
            )));
        }
        let out = extract_body_excerpt(Some(&json(blocks)), None, 2_000);
        assert!(out.chars().count() <= 2_000);
        assert!(out.starts_with(&lead), "lead should be whole: {out}");
        // 选中的段落都带着自己的结尾，没被选中的整段省略。
        for i in 1..=6 {
            let head = out.contains(&format!("第 {i} 段开头"));
            let tail = out.contains(&format!("第 {i} 段结尾"));
            assert_eq!(head, tail, "paragraph {i} was cut");
        }
        assert!(out.contains("第 6 段结尾"));
        assert!(out.contains(GAP));
        assert!(!out.contains(" …"), "no sentence cut: {out}");
    }

    #[test]
    fn single_huge_paragraph_is_cut_at_a_sentence() {
        let text = "一句完整的话。".repeat(2000);
        let out = extract_summary_payload(
            "标题",
            None,
            Some(&format!("<p>{text}</p>")),
            false,
            InputTier::Low,
        );
        assert!(out.chars().count() <= InputTier::Low.summary_budget(false));
        assert!(out.ends_with("。 …"), "{out}");
        assert!(!out.contains('<'));
    }

    #[test]
    fn long_code_and_boilerplate_are_dropped_while_short_code_is_bounded() {
        let long_code = (0..20)
            .map(|i| format!("let line_{i} = compute_step({i}, state);"))
            .collect::<Vec<_>>()
            .join("\n");
        let doc = json(vec![
            para("第一段导言，交代背景。"),
            Block::CodeBlock {
                lang: Some("rust".into()),
                text: long_code,
            },
            heading("核心架构章节"),
            Block::CodeBlock {
                lang: Some("shell".into()),
                text: "rrss migrate --atomic".into(),
            },
            para("版权所有，转载请注明出处"),
            para("最终结论段落，总结所有成果。"),
        ]);
        let out =
            extract_summary_payload("测试文章标题", Some(&doc), None, false, InputTier::Medium);
        assert!(out.starts_with("测试文章标题\n\n"));
        assert!(out.contains("### 核心架构章节"));
        assert!(!out.contains("compute_step"), "long code dump must be dropped");
        assert!(out.contains("rrss migrate --atomic"), "short CLI snippet should be kept");
        assert!(!out.contains("版权所有"));
        assert!(out.contains("最终结论段落"));
    }

    #[test]
    fn excerpt_for_digest_has_no_title_and_honours_budget() {
        let short = json(vec![para(
            "Nvidia 同意以 130 亿美元收购 Hugging Face，交易尚待监管批准。",
        )]);
        let out = extract_body_excerpt(Some(&short), None, 3_000);
        assert_eq!(
            out,
            "Nvidia 同意以 130 亿美元收购 Hugging Face，交易尚待监管批准。"
        );

        let long = json(
            (0..80)
                .map(|i| para(format!("第 {i} 段。{}", "内容".repeat(50))))
                .collect(),
        );
        let out = extract_body_excerpt(Some(&long), None, 1_500);
        assert!(out.chars().count() <= 1_500);
        assert!(out.starts_with("第 0 段") && out.contains("第 79 段"));
        assert!(extract_body_excerpt(None, None, 1_500).is_empty());
    }

    #[test]
    fn body_signature_follows_blocks_then_html() {
        let before = body_signature(None, Some("<p>teaser</p>"));
        let with_sig = format!(r#"{{"summary":"s","{BODY_SIG_KEY}":"{before}"}}"#);
        assert!(!summary_is_stale(
            Some(&with_sig),
            None,
            Some("<p>teaser</p>")
        ));
        assert!(summary_is_stale(
            Some(&with_sig),
            Some(r#"{"v":1,"blocks":[]}"#),
            Some("<p>teaser</p>")
        ));
        assert!(!summary_is_stale(
            Some(r#"{"summary":"old"}"#),
            Some("changed"),
            None
        ));
        assert!(!summary_is_stale(None, Some("changed"), None));
    }

    #[test]
    fn long_articles_split_into_ordered_segments_of_whole_paragraphs() {
        let short = json(vec![para("短文一段。")]);
        assert!(split_segments(Some(&short), None, 2_000).is_none());

        let blocks: Vec<Block> = (0..40)
            .flat_map(|i| {
                let mut v = Vec::new();
                if i % 10 == 0 {
                    v.push(heading(format!("第 {} 章", i / 10 + 1)));
                }
                v.push(para(format!("第 {i} 段。{}", "论述内容".repeat(60))));
                v
            })
            .collect();
        let doc = json(blocks);
        let segments = split_segments(Some(&doc), None, 4_000).unwrap();
        assert!(
            (2..=MAX_SEGMENTS).contains(&segments.len()),
            "{}",
            segments.len()
        );
        for seg in &segments {
            assert!(seg.chars().count() <= 4_000);
            assert!(!seg.contains(GAP), "within the cap every paragraph is kept");
        }
        assert!(segments[0].starts_with("### 第 1 章"));
        assert!(segments.last().unwrap().contains("第 39 段"));

        // 超过「段数 × 预算」时每段内部再挑段落，仍然不超预算。
        let segments = split_segments(Some(&doc), None, 1_000).unwrap();
        assert_eq!(segments.len(), MAX_SEGMENTS);
        assert!(segments.iter().all(|s| s.chars().count() <= 1_000));
        assert!(segments.iter().any(|s| s.contains(GAP)));

        let stub = "<p>Article URL: https://a.test/x</p><p>Comments URL: https://news.test/1</p>";
        assert!(split_segments(None, Some(stub), 10).is_none());
    }

    #[test]
    fn tiers_round_trip_and_grow() {
        for tier in [InputTier::Low, InputTier::Medium, InputTier::High] {
            assert_eq!(InputTier::parse(tier.as_str()), Some(tier));
        }
        assert_eq!(InputTier::parse("huge"), None);
        assert!(InputTier::Low.summary_budget(true) < InputTier::Medium.summary_budget(true));
        assert!(InputTier::Medium.digest_total_chars() < InputTier::High.digest_total_chars());
    }

    #[test]
    fn link_stub_is_marked_for_summary_and_digest() {
        let html = "<p>Article URL: <a href=\"https://asahilinux.org/2026/08/progress-report-7-2/\">https://asahilinux.org/2026/08/progress-report-7-2/</a></p>\
             <p>Comments URL: <a href=\"https://news.ycombinator.com/item?id=49456851\">https://news.ycombinator.com/item?id=49456851</a></p>\
             <p>Points: 81</p><p># Comments: 8</p>";
        let title = "Asahi Linux Progress Report: Linux 7.2";
        let deep = extract_body_excerpt(None, Some(html), 3_000);
        assert!(deep.starts_with("[No body text]"), "{deep}");
        assert!(!deep.to_lowercase().contains("points:"), "{deep}");
        let summary = extract_summary_payload(title, None, Some(html), false, InputTier::Medium);
        assert!(summary.contains("[No body text]"), "{summary}");
        assert!(summary.contains("Linux 7.2"), "{summary}");
        assert!(
            !summary.to_lowercase().contains("comments url"),
            "{summary}"
        );
    }

    #[test]
    fn english_articles_get_token_aligned_budget_and_avoid_premature_splitting() {
        // 约 6,500 字符的英文长文（约 1,100 词）：按旧字符数会远超 Medium 4,000 上限并强制分段，
        // 按加权长度约 2,600 单位，单次预算即可完整送入，无需分段。
        let blocks: Vec<Block> = (1..=20)
            .map(|i| {
                para(format!(
                    "Paragraph {i} explains the runtime architecture in detail and reports benchmark latency of {i}2 ms across standard workloads. {}",
                    "Engineering context and concrete design rationale. ".repeat(4)
                ))
            })
            .collect();
        let doc = json(blocks);
        let out = extract_summary_payload("Runtime Notes", Some(&doc), None, false, InputTier::Medium);
        assert!(out.contains("Paragraph 1 explains"));
        assert!(out.contains("Paragraph 20 explains"));
        assert!(!out.contains(GAP), "should fit whole without truncation");
        assert!(
            split_segments(Some(&doc), None, InputTier::Medium.summary_budget(false)).is_none(),
            "should not split when weighted length fits"
        );
    }

    #[test]
    fn split_segments_has_hysteresis_and_keeps_headings_with_their_body() {
        // 略超预算（+8%，在 15% 容忍带内）不触发分段。
        let slight: Vec<Block> = (0..10)
            .map(|i| para(format!("第 {i} 段。{}", "论述文字。".repeat(20))))
            .collect();
        assert!(split_segments(Some(&json(slight)), None, 1_000).is_none());

        // 触发分段时，小标题不能孤零零留在上一段末尾。
        let mut blocks = Vec::new();
        for sec in 1..=8 {
            blocks.push(heading(format!("第 {sec} 节标题")));
            blocks.push(para(format!("第 {sec} 节正文。{}", "核心论证内容。".repeat(45))));
        }
        let segments = split_segments(Some(&json(blocks)), None, 800).unwrap();
        assert!(segments.len() >= 2);
        for seg in &segments {
            let last_block = seg.split("\n\n").last().unwrap_or("");
            assert!(
                !last_block.starts_with("### "),
                "orphaned trailing heading in segment: {seg}"
            );
        }
    }

    #[test]
    fn huge_table_keeps_header_and_top_rows_when_room_remains() {
        let mut md = String::from("| 模型名称 | 综合得分 | 推理延迟 |\n| --- | --- | --- |\n");
        for i in 1..=60 {
            md.push_str(&format!("| 模型方案-{i} | {i}9.5% | {i}2 毫秒 |\n"));
        }
        let table = rrss_render::markdown_to_document(&md).blocks.remove(0);
        let doc = json(vec![
            para("导语：评测汇总各模型性能。"),
            table,
            para("结尾：结论供参考。"),
        ]);
        let out = extract_body_excerpt(Some(&doc), None, 400);
        assert!(out.contains("模型方案-1"), "top table rows should be preserved: {out}");
        assert!(!out.contains("模型方案-60"), "tail rows should be truncated: {out}");
        assert!(out.contains("导语：评测汇总"));
        assert!(out.contains("结尾：结论供参考"));
        assert!(out.contains(GAP));
    }
}
