//! Effective site rules and settings operations; rule testing never writes an article.
use crate::{
    extract::{self, site_rules::builtin_site_rules},
    fulltext::{FulltextError, FulltextErrorKind},
    views::*,
};
use rrss_core::{Error, Result, db::site_rules as db};
use rusqlite::Connection;
use std::time::Instant;

pub(crate) struct ResolvedSiteRule {
    pub rule: Option<db::SiteRule>,
    pub user_rule: bool,
}

/// Enabled user rules replace the whole built-in rule; built-ins use the shared indexes.
pub(crate) fn resolve_site_rule(conn: &Connection, host: &str) -> Result<ResolvedSiteRule> {
    let users = db::list(conn)?;
    let user = db::match_site_rule(host, users.iter().filter(|r| r.enabled).map(|r| &r.rule));
    Ok(ResolvedSiteRule {
        rule: user
            .or_else(|| builtin_site_rules().matching_rule(host))
            .cloned(),
        user_rule: user.is_some(),
    })
}

impl From<SiteRuleView> for db::SiteRule {
    fn from(v: SiteRuleView) -> Self {
        Self {
            domain: v.domain,
            javascript_disabled: v.javascript_disabled.unwrap_or(false),
            body: v.body,
            title: v.title,
            author: v.author,
            date: v.date,
            strip: v.strip,
        }
    }
}
impl From<db::SiteRule> for SiteRuleView {
    fn from(v: db::SiteRule) -> Self {
        Self {
            domain: v.domain,
            javascript_disabled: Some(v.javascript_disabled),
            body: v.body,
            title: v.title,
            author: v.author,
            date: v.date,
            strip: v.strip,
        }
    }
}
fn domain(raw: &str) -> Result<String> {
    let normalized = db::normalize_domain(raw);
    let host = normalized.strip_prefix("*.").unwrap_or(&normalized);
    if host.is_empty()
        || host.contains('*')
        || host.contains(['/', ':', '@', '?', '#', '%'])
        || url::Host::parse(host).is_err()
    {
        return Err(Error::Web(
            rrss_core::error::WebFailure::SiteInputDomainInvalid,
        ));
    }
    Ok(normalized)
}
fn validate(rule: &db::SiteRule) -> Result<()> {
    domain(&rule.domain)?;
    for (field, selector) in [
        ("body", &rule.body),
        ("title", &rule.title),
        ("author", &rule.author),
        ("date", &rule.date),
    ]
    .into_iter()
    .filter_map(|(name, s)| s.as_deref().map(|s| (name, s)))
    .chain(rule.strip.iter().map(|s| ("strip", s.as_str())))
    {
        if !selector.trim().is_empty() && dom_query::Matcher::new(selector).is_err() {
            return Err(Error::Web(
                rrss_core::error::WebFailure::SiteSelectorInvalid {
                    field: field.into(),
                },
            ));
        }
    }
    Ok(())
}
pub fn site_rules_page(conn: &Connection) -> Result<SiteRulesPage> {
    let builtin = builtin_site_rules();
    Ok(SiteRulesPage {
        users: db::list(conn)?
            .into_iter()
            .map(|v| StoredSiteRuleView {
                rule: v.rule.into(),
                enabled: v.enabled,
                updated_at: v.updated_at,
            })
            .collect(),
        builtin_count: builtin.rules.len() as i64,
        source: builtin.source.clone(),
        commit: builtin.commit.clone(),
    })
}
pub fn save_site_rule(conn: &Connection, rule: SiteRuleView, enabled: bool) -> Result<()> {
    let javascript = rule.javascript_disabled;
    let mut rule: db::SiteRule = rule.into();
    validate(&rule)?;
    if javascript.is_none()
        && let Some(existing) = db::list(conn)?
            .into_iter()
            .find(|r| r.rule.domain == db::normalize_domain(&rule.domain))
    {
        rule.javascript_disabled = existing.rule.javascript_disabled;
    }
    db::upsert(conn, &rule, enabled)
}
pub fn delete_site_rule(conn: &Connection, raw: &str) -> Result<()> {
    db::delete(conn, &domain(raw)?)
}
pub fn set_site_rule_enabled(conn: &Connection, raw: &str, enabled: bool) -> Result<()> {
    db::set_enabled(conn, &domain(raw)?, enabled)
}
/// Create an exact host policy, preserving the effective extraction rule rather
/// than changing a wildcard policy for unrelated hosts.
pub fn disable_site_javascript(conn: &Connection, raw: &str) -> Result<()> {
    let url = url::Url::parse(raw)
        .map_err(|_| Error::Web(rrss_core::error::WebFailure::SiteUrlInvalid))?;
    if !rrss_core::url_norm::is_http_url(&url) {
        return Err(Error::Web(rrss_core::error::WebFailure::SiteHttpOnly));
    }
    let host = domain(url.host_str().unwrap_or_default())?;
    let mut rule = resolve_site_rule(conn, &host)?.rule.unwrap_or_default();
    rule.domain = host;
    rule.javascript_disabled = true;
    save_site_rule(conn, rule.into(), true)
}
pub async fn test_site_rule(
    network: &crate::network::NetworkClients,
    raw: &str,
    rule: SiteRuleView,
) -> std::result::Result<SiteRuleTestView, FulltextError> {
    let started = Instant::now();
    let invalid = |error: UiError| FulltextError::coded(FulltextErrorKind::Empty, Some(error));
    let rule = rule.into();
    validate(&rule).map_err(|e| invalid(e.into()))?;
    let url = url::Url::parse(raw.trim())
        .map_err(|_| invalid(crate::ui_error::web::WebError::SiteTestUrlInvalid.into()))?;
    if !rrss_core::url_norm::is_http_url(&url) {
        return Err(invalid(
            crate::ui_error::web::WebError::SiteTestUrlInvalid.into(),
        ));
    }
    let (html, actual_url) = crate::fulltext::fetch_page_with_url(network, url.as_str()).await?;
    tokio::task::spawn_blocking(move || {
        let report = extract::site_rules::test_site_rule(&html, &actual_url, &rule)?;
        Ok(SiteRuleTestView {
            layer: WebLinkLayer::from_extraction(report.layer, true),
            title: report.title,
            author: report.author,
            date: report.date,
            text_chars: report.text_chars as i64,
            image_count: report.image_count as i64,
            paragraphs: report.paragraphs,
            stripped_elements: report.stripped_elements as i64,
            elapsed_ms: started.elapsed().as_millis().min(i64::MAX as u128) as i64,
        })
    })
    .await
    .map_err(|_| invalid(crate::ui_error::web::WebError::SiteTestFailed.into()))?
}

#[cfg(test)]
mod tests;
