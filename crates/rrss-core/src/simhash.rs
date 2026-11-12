//! 64-bit Charikar simhash。日志 target：`rrss_core::simhash`。

use std::{collections::HashSet, sync::LazyLock};

use sha2::{Digest, Sha256};

const BODY_CHARS: usize = 2000;
// Distinct content words: repeated footer words must not make a short excerpt eligible.
pub const MIN_CONTENT_TOKENS: usize = 12;
const STOP_WORDS: &str = "the and for with from this that these those into over under about your you our their its are was were has have had not but how why what when where who will can new using use 的 了 在 是 和 与 为 将 对 这 一个";
static SOURCE_FOOTER: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?is)\b(?:the\s+(?:post|article)\b.{0,500}?\b(?:appeared|was published)\s+first\s+on|(?:originally|first)\s+published\s+(?:on|at))\b.{0,200}\s*$")
        .expect("source attribution footer")
});

/// Tokenization stays shared with simhash; ignore function words and single CJK characters.
pub(crate) fn content_tokens(text: &str) -> HashSet<String> {
    tokenize(text)
        .into_iter()
        .filter(|t| {
            let min_len = if t.is_ascii() { 3 } else { 2 };
            t.chars().count() >= min_len && !STOP_WORDS.split_whitespace().any(|s| s == t)
        })
        .collect()
}

/// 标题 + 去来源尾注后的正文前段；实质词不足时为 0，不参与近邻匹配。
pub fn fingerprint(title: &str, body: &str) -> i64 {
    let mut text = String::with_capacity(title.len() + body.len() + 1);
    text.push_str(title);
    text.push(' ');
    let body = SOURCE_FOOTER.replace(body, "");
    let body = crate::ai_store::truncate_chars(&body, BODY_CHARS);
    text.push_str(&body);
    let tokens = tokenize(&text);
    if content_tokens(&text).len() < MIN_CONTENT_TOKENS {
        return 0;
    }
    let mut counts = [0i32; 64];
    for token in tokens {
        let bits = hash64(&token);
        for (i, count) in counts.iter_mut().enumerate() {
            if (bits >> i) & 1 == 1 {
                *count += 1;
            } else {
                *count -= 1;
            }
        }
    }
    let mut out = 0u64;
    for (i, count) in counts.iter().enumerate() {
        if *count >= 0 {
            out |= 1u64 << i;
        }
    }
    out as i64
}

pub fn fingerprint_article(title: &str, html: Option<&str>) -> i64 {
    let body = crate::score::strip_html(html.unwrap_or(""));
    fingerprint(title, &body)
}

pub fn hamming(a: i64, b: i64) -> u32 {
    (a as u64 ^ b as u64).count_ones()
}

pub fn tokenize(s: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let lower = s.to_lowercase();
    let chars: Vec<char> = lower.chars().collect();
    let mut latin = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_ascii_alphanumeric() {
            latin.push(c);
            i += 1;
            continue;
        }
        if !latin.is_empty() {
            tokens.push(std::mem::take(&mut latin));
        }
        if is_cjk(c) {
            tokens.push(c.to_string());
            if i + 1 < chars.len() && is_cjk(chars[i + 1]) {
                tokens.push(format!("{}{}", c, chars[i + 1]));
            }
        }
        i += 1;
    }
    if !latin.is_empty() {
        tokens.push(latin);
    }
    tokens
}

fn hash64(token: &str) -> u64 {
    let digest = Sha256::digest(token.as_bytes());
    u64::from_le_bytes(digest[0..8].try_into().expect("sha256 prefix"))
}

fn is_cjk(c: char) -> bool {
    matches!(
        c,
        '\u{4E00}'..='\u{9FFF}'
            | '\u{3400}'..='\u{4DBF}'
            | '\u{3040}'..='\u{30FF}'
            | '\u{AC00}'..='\u{D7AF}'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_titles_are_close() {
        let a = fingerprint("市政厅新图书馆开放", "市民可凭证件借阅。");
        let b = fingerprint("市政厅新图书馆开放", "市民可凭证件借阅。");
        assert_eq!(a, b);
        assert_ne!(a, 0);
        assert_eq!(hamming(a, b), 0);
    }

    #[test]
    fn near_duplicate_cjk_is_closer_than_unrelated() {
        let a = fingerprint("市政厅新图书馆今日开放", "市民可凭证件到新馆借阅图书。");
        let b = fingerprint("市政厅新图书馆开放", "市民可凭证件到新馆借阅图书。");
        let c = fingerprint("英超争冠夜", "曼城客场击败阿森纳。");
        assert!(
            hamming(a, b) < hamming(a, c),
            "{} vs {}",
            hamming(a, b),
            hamming(a, c)
        );
    }

    #[test]
    fn unrelated_is_far() {
        let a = fingerprint("市政厅新图书馆开放", "借阅规则更新。");
        let b = fingerprint("英超争冠夜", "曼城客场击败阿森纳。");
        assert!(hamming(a, b) > 12, "hamming {}", hamming(a, b));
    }

    #[test]
    fn empty_is_zero() {
        assert_eq!(fingerprint("   ", ""), 0);
    }
}
