//! Pre-filter cues and evidence grounding for long-term memory extraction.
//! Contains Chinese and English user-message cue words (not UI copy).

use crate::memories::PENDING_BACKLOG_LIMIT;

const EXPLICIT_SIGNALS_ZH: &[&str] = &[
    "记住",
    "记下来",
    "记一下",
    "帮我记",
    "以后",
    "今后",
    "从今往后",
    "下次",
    "每次",
    "总是",
    "一直",
    "别每次",
    "不要每次",
    "别总是",
    "不要总是",
    "永远不要",
    "千万别",
    "偏好",
    "默认用",
    "默认给我",
    "习惯",
    "通常",
    "一般都",
    "一般",
    "平时",
];

const EXPLICIT_SIGNALS_EN: &[&str] = &[
    "remember that",
    "remember this",
    "keep in mind",
    "from now on",
    "in the future",
    "next time",
    "always",
    "never",
    "every time",
    "by default",
    "i usually",
    "i typically",
    "i prefer",
    "my preference",
    "don't ever",
    "do not ever",
];

const SOFT_SIGNALS_ZH: &[&str] = &[
    "我主要",
    "我关注",
    "我比较关注",
    "我更关注",
    "我只看",
    "我对",
    "不感兴趣",
    "没兴趣",
    "不太感兴趣",
    "不太想看",
    "不喜欢",
    "更喜欢",
    "很喜欢",
    "讨厌",
    "很烦",
    "我是",
    "我做",
    "我在做",
    "我用",
    "主力机",
    "换回",
    "换到",
    "我换了",
    "我最近换",
    "我改用",
    "我现在用",
    "我不再",
    "我不看",
    "我平时",
    "每天",
    "每周",
    "对我来说",
    "我的工作",
    "我的职业",
    "我的背景",
    "太啰嗦",
    "太学术",
    "太长了",
    "太专业",
    "太浅",
    "客套",
    "废话",
    "结尾反问",
    "直接回答",
    "别说",
    "不要说",
    "少说",
    "别老",
    "不要老",
    "别分点",
    "不要分点",
    "少堆",
    "少用",
    "多用",
    "多打比方",
    "打比方",
    "直接给结论",
    "直接说结论",
    "先给结论",
    "简洁",
    "通俗",
    "简报里",
    "别默认",
    "不要默认",
];

const SOFT_SIGNALS_EN: &[&str] = &[
    "i'm a ",
    "i am a ",
    "i work as",
    "i work on",
    "i use ",
    "i switched to",
    "i moved to",
    "i follow",
    "i care about",
    "i'm interested in",
    "interested in",
    "not interested",
    "don't care about",
    "do not care about",
    "daily driver",
    "my job",
    "my role",
    "i read ",
    "hate ",
    "annoying",
    "too verbose",
    "too long",
    "too academic",
    "too technical",
    "too brief",
    "more concise",
    "be concise",
    "no pleasantries",
    "no filler",
    "skip filler",
    "just answer",
    "skip the ",
    "avoid ",
    "stop ",
    "plain language",
    "analogies",
    "get straight to",
    "lead with",
    "bottom line",
    "in my digest",
];

fn contains_ascii_phrase(haystack_lower: &str, needle: &str) -> bool {
    let bytes = haystack_lower.as_bytes();
    let needle_bytes = needle.as_bytes();
    if needle_bytes.is_empty() || needle_bytes.len() > bytes.len() {
        return false;
    }
    let starts_word = needle_bytes[0].is_ascii_alphanumeric();
    let ends_word = needle_bytes[needle_bytes.len() - 1].is_ascii_alphanumeric();
    let mut offset = 0;
    while let Some(pos) = haystack_lower[offset..].find(needle) {
        let start = offset + pos;
        let end = start + needle_bytes.len();
        let left_ok = !starts_word || start == 0 || !bytes[start - 1].is_ascii_alphanumeric();
        let right_ok = !ends_word || end == bytes.len() || !bytes[end].is_ascii_alphanumeric();
        if left_ok && right_ok {
            return true;
        }
        offset = start + 1;
    }
    false
}

pub fn has_explicit_memory_signal(clean_text: &str) -> bool {
    if EXPLICIT_SIGNALS_ZH.iter().any(|s| clean_text.contains(s)) {
        return true;
    }
    let lower = clean_text.to_ascii_lowercase();
    EXPLICIT_SIGNALS_EN
        .iter()
        .any(|s| contains_ascii_phrase(&lower, s))
}

fn has_soft_memory_signal(clean_text: &str) -> bool {
    if SOFT_SIGNALS_ZH.iter().any(|s| clean_text.contains(s)) {
        return true;
    }
    let lower = clean_text.to_ascii_lowercase();
    SOFT_SIGNALS_EN
        .iter()
        .any(|s| contains_ascii_phrase(&lower, s))
}

/// Low-noise gate: skip ordinary reading Q&A so the extractor only runs when
/// the user expresses durable background, preferences, standing rules, or
/// stylistic corrections. When the pending queue already reached its backlog
/// cap, only explicit "remember / from now on" signals trigger extraction.
pub fn should_extract_memory(clean_text: &str, pending_count: usize) -> bool {
    let trimmed = clean_text.trim();
    if trimmed.chars().count() < 6 {
        return false;
    }
    if has_explicit_memory_signal(trimmed) {
        return true;
    }
    pending_count < PENDING_BACKLOG_LIMIT && has_soft_memory_signal(trimmed)
}

fn is_cjk(ch: char) -> bool {
    matches!(
        ch as u32,
        0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0x3040..=0x30FF | 0xAC00..=0xD7AF
    )
}

fn normalize_for_grounding(text: &str) -> String {
    text.chars()
        .filter(|ch| ch.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Verify that a candidate's evidence span is grounded in the user's own
/// cleaned message (ignoring punctuation/whitespace/case differences).
pub fn evidence_matches(clean_user_message: &str, evidence: &str) -> bool {
    let norm_ev = normalize_for_grounding(evidence);
    let min_len = if norm_ev.chars().any(is_cjk) { 3 } else { 4 };
    if norm_ev.chars().count() < min_len {
        return false;
    }
    normalize_for_grounding(clean_user_message).contains(&norm_ev)
}
