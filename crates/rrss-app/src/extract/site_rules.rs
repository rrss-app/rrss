//! Embedded, versioned FiveFilters public-domain rules; never updated over the network.
use std::collections::HashMap;
use std::sync::OnceLock;

use rrss_render::{Block, inlines_to_plain};
use serde::Deserialize;
use url::Url;

use super::{ExtractError, Layer, extract_article};
use rrss_core::db::site_rules::SiteRule;
use rrss_core::db::site_rules::normalize_domain;

#[derive(Debug, Deserialize)]
pub(crate) struct BuiltinSiteRules {
    pub(crate) source: String,
    pub(crate) commit: String,
    pub(crate) rules: Vec<SiteRule>,
    #[serde(skip)]
    exact: HashMap<String, usize>,
    #[serde(skip)]
    wildcards: HashMap<String, usize>,
}

impl BuiltinSiteRules {
    /// Exact first, then progressively shorter suffixes. Never test the whole host
    /// against the wildcard table, so a wildcard cannot match its own bare domain.
    pub(crate) fn matching_rule(&self, host: &str) -> Option<&SiteRule> {
        let host = normalize_domain(host);
        if let Some(&index) = self.exact.get(host.as_str()) {
            return Some(&self.rules[index]);
        }
        let mut suffix = host.as_str();
        while let Some((_, rest)) = suffix.split_once('.') {
            if host.len() - rest.len() > 1
                && let Some(&index) = self.wildcards.get(rest)
            {
                return Some(&self.rules[index]);
            }
            suffix = rest;
        }
        None
    }
}

pub(crate) fn builtin_site_rules() -> &'static BuiltinSiteRules {
    static RULES: OnceLock<BuiltinSiteRules> = OnceLock::new();
    RULES.get_or_init(|| {
        let mut data: BuiltinSiteRules =
            serde_json::from_str(include_str!("../../data/site-rules.json"))
                .expect("checked-in site rules must be valid JSON");
        for (index, rule) in data.rules.iter().enumerate() {
            let domain = normalize_domain(&rule.domain);
            if let Some(suffix) = domain.strip_prefix("*.") {
                data.wildcards.insert(suffix.to_string(), index);
            } else {
                data.exact.insert(domain, index);
            }
        }
        data
    })
}

#[derive(Debug)]
pub(crate) struct SiteRuleTestReport {
    pub(crate) layer: Layer,
    pub(crate) title: String,
    pub(crate) author: Option<String>,
    pub(crate) date: Option<String>,
    /// Unicode character count, matching extraction's suspiciousness thresholds.
    pub(crate) text_chars: usize,
    pub(crate) image_count: usize,
    pub(crate) paragraphs: Vec<String>,
    pub(crate) stripped_elements: usize,
}

/// Offline and read-only: neither a database nor a network client is accepted.
pub(crate) fn test_site_rule(
    html: &str,
    url: &Url,
    rule: &SiteRule,
) -> Result<SiteRuleTestReport, ExtractError> {
    let result = extract_article(html, url, Some(rule))?;
    let paragraphs = result
        .document
        .blocks
        .iter()
        .filter_map(|block| match block {
            Block::Paragraph { inlines } => Some(inlines_to_plain(inlines)),
            _ => None,
        })
        .filter(|text| !text.trim().is_empty())
        .take(3)
        .collect();
    Ok(SiteRuleTestReport {
        layer: result.layer,
        title: result.title,
        author: result.byline,
        date: result.published_time,
        text_chars: result.text_chars,
        image_count: result.image_count,
        paragraphs,
        stripped_elements: result.stripped_elements,
    })
}

#[cfg(test)]
mod tests;
