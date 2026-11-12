//! 训练条特征抽取。日志 target：`rrss_core::features`。

use serde::{Deserialize, Serialize};

use crate::rules::Dimension;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeatureChip {
    pub dimension: Dimension,
    pub pattern: String,
    pub label: String,
}

/// 抽出可点选芯片。划词摘录来自阅读器选区；无选区时不加正文短语。
pub fn extract_features(
    author: Option<&str>,
    tags: &[String],
    title: &str,
    canonical_url: Option<&str>,
    excerpt: Option<&str>,
) -> Vec<FeatureChip> {
    extract_features_with_bigrams(author, tags, title, canonical_url, excerpt, false)
}

/// 排序可启用中文二元组；训练条保留原有标题芯片。
pub fn extract_features_with_bigrams(
    author: Option<&str>,
    tags: &[String],
    title: &str,
    canonical_url: Option<&str>,
    excerpt: Option<&str>,
    chinese_bigrams: bool,
) -> Vec<FeatureChip> {
    let mut out = Vec::new();
    if let Some(author) = author.map(str::trim).filter(|s| !s.is_empty()) {
        out.push(FeatureChip {
            dimension: Dimension::Author,
            pattern: author.to_string(),
            label: format!("作者：{author}"),
        });
    }
    for tag in tags {
        let tag = tag.trim();
        if tag.is_empty() {
            continue;
        }
        out.push(FeatureChip {
            dimension: Dimension::Tag,
            pattern: tag.to_string(),
            label: format!("标签：{tag}"),
        });
    }
    if let Some(host) = canonical_url.and_then(domain) {
        out.push(FeatureChip {
            dimension: Dimension::UrlPhrase,
            pattern: host.clone(),
            label: format!("域名：{host}"),
        });
    }
    for token in title_tokens(title, chinese_bigrams) {
        out.push(FeatureChip {
            dimension: Dimension::TitleKw,
            pattern: token.clone(),
            label: format!("标题：{token}"),
        });
    }
    if let Some(excerpt) = excerpt.map(str::trim).filter(|s| !s.is_empty()) {
        let pattern = if excerpt.chars().count() > 80 {
            excerpt.chars().take(80).collect()
        } else {
            excerpt.to_string()
        };
        out.push(FeatureChip {
            dimension: Dimension::BodyPhrase,
            pattern: pattern.clone(),
            label: format!("摘录：{pattern}"),
        });
    }
    out
}

fn domain(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    let host = parsed.host_str()?;
    Some(host.trim_start_matches("www.").to_string())
}

fn title_tokens(title: &str, chinese_bigrams: bool) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let push = |tokens: &mut Vec<String>, current: &mut String| {
        let trimmed = current.trim();
        if trimmed.chars().count() >= 2 {
            tokens.push(trimmed.to_string());
        }
        current.clear();
    };
    let mut previous = None;
    for c in title.chars() {
        if chinese_bigrams && matches!(c, '\u{3400}'..='\u{9fff}') {
            push(&mut tokens, &mut current);
            if let Some(prev) = previous {
                tokens.push(format!("{prev}{c}"));
            }
            previous = Some(c);
            continue;
        }
        previous = None;
        if c.is_whitespace() || is_split_punct(c) {
            push(&mut tokens, &mut current);
        } else {
            current.push(c);
        }
    }
    push(&mut tokens, &mut current);
    if tokens.is_empty() && !chinese_bigrams {
        let trimmed = title.trim();
        if trimmed.chars().count() >= 2 {
            tokens.push(trimmed.to_string());
        }
    }
    tokens
}

fn is_split_punct(c: char) -> bool {
    c.is_ascii_punctuation()
        || matches!(
            c,
            '，' | '。'
                | '、'
                | '；'
                | '：'
                | '！'
                | '？'
                | '（'
                | '）'
                | '《'
                | '》'
                | '「'
                | '」'
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_author_domain_title_words() {
        let chips = extract_features(
            Some("Ada Example"),
            &["City".into()],
            "City opens new library",
            Some("https://news.example.com/library"),
            Some("opened downtown"),
        );
        assert!(
            chips
                .iter()
                .any(|c| c.dimension == Dimension::Author && c.pattern == "Ada Example")
        );
        assert!(
            chips
                .iter()
                .any(|c| c.dimension == Dimension::Tag && c.pattern == "City")
        );
        assert!(
            chips
                .iter()
                .any(|c| c.dimension == Dimension::UrlPhrase && c.pattern == "news.example.com")
        );
        assert!(
            chips
                .iter()
                .any(|c| c.dimension == Dimension::TitleKw && c.pattern == "library")
        );
        assert!(
            chips
                .iter()
                .any(|c| c.dimension == Dimension::BodyPhrase && c.pattern == "opened downtown")
        );
    }

    #[test]
    fn bigrams_are_optional_and_do_not_cross_punctuation_or_latin() {
        let chips =
            extract_features_with_bigrams(None, &[], "中文，标题 Rust 开发", None, None, true);
        assert_eq!(
            chips.iter().map(|c| c.pattern.as_str()).collect::<Vec<_>>(),
            ["中文", "标题", "Rust", "开发"]
        );
        assert!(extract_features_with_bigrams(None, &[], "中", None, None, true).is_empty());
    }

    #[test]
    fn cjk_title_is_one_chip() {
        let chips = extract_features(None, &[], "城市开放新图书馆", None, None);
        assert_eq!(chips.len(), 1);
        assert_eq!(chips[0].pattern, "城市开放新图书馆");
        assert_eq!(chips[0].dimension, Dimension::TitleKw);
    }
}
