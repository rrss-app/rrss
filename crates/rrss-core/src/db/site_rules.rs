//! Declarative extraction rules. Matching is offline and independent of storage.
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SiteRule {
    pub domain: String,
    #[serde(default)]
    pub javascript_disabled: bool,
    pub body: Option<String>,
    pub title: Option<String>,
    pub author: Option<String>,
    pub date: Option<String>,
    #[serde(default)]
    pub strip: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct StoredSiteRule {
    pub id: i64,
    pub rule: SiteRule,
    pub enabled: bool,
    /// Unix milliseconds, like every schema timestamp.
    pub updated_at: i64,
}

pub fn normalize_domain(domain: &str) -> String {
    let lower = domain.trim().to_ascii_lowercase();
    let (prefix, host) = lower
        .strip_prefix("*.")
        .map_or(("", lower.as_str()), |host| ("*.", host));
    format!("{prefix}{}", host.strip_prefix("www.").unwrap_or(host))
}

/// Exact first, then the longest wildcard suffix; bare hosts never match a wildcard.
pub fn match_site_rule<'a>(
    host: &str,
    rules: impl IntoIterator<Item = &'a SiteRule>,
) -> Option<&'a SiteRule> {
    let host = normalize_domain(host);
    rules
        .into_iter()
        .filter_map(|rule| {
            let domain = normalize_domain(&rule.domain);
            if let Some(suffix) = domain.strip_prefix("*.") {
                host.strip_suffix(suffix)
                    .is_some_and(|prefix| prefix.ends_with('.') && prefix.len() > 1)
                    .then_some(((false, suffix.len()), rule))
            } else {
                (host == domain).then_some(((true, domain.len()), rule))
            }
        })
        .max_by_key(|(priority, _)| *priority)
        .map(|(_, rule)| rule)
}

pub fn list(conn: &Connection) -> Result<Vec<StoredSiteRule>> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, domain, body, title, author, date, strip, enabled, updated_at, javascript_disabled
         FROM site_rules ORDER BY domain",
    )?;
    let rows = stmt.query_map([], |row| {
        let strip: Option<String> = row.get(6)?;
        let strip = strip
            .map(|json| {
                serde_json::from_str(&json).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        6,
                        rusqlite::types::Type::Text,
                        Box::new(e),
                    )
                })
            })
            .transpose()?
            .unwrap_or_default();
        Ok(StoredSiteRule {
            id: row.get(0)?,
            rule: SiteRule {
                domain: row.get(1)?,
                javascript_disabled: row.get(9)?,
                body: row.get(2)?,
                title: row.get(3)?,
                author: row.get(4)?,
                date: row.get(5)?,
                strip,
            },
            enabled: row.get(7)?,
            updated_at: row.get(8)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn upsert(conn: &Connection, rule: &SiteRule, enabled: bool) -> Result<()> {
    let domain = normalize_domain(&rule.domain);
    let host = domain.strip_prefix("*.").unwrap_or(&domain);
    if host.contains('*') || url::Host::parse(host).is_err() {
        return Err(Error::InvalidRule("invalid site domain".into()));
    }
    let strip = (!rule.strip.is_empty())
        .then(|| serde_json::to_string(&rule.strip))
        .transpose()
        .map_err(|e| Error::InvalidRule(e.to_string()))?;
    conn.execute(
        "INSERT INTO site_rules(domain, body, title, author, date, strip, enabled, updated_at, javascript_disabled)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(domain) DO UPDATE SET body=excluded.body, title=excluded.title,
         author=excluded.author, date=excluded.date, strip=excluded.strip,
         enabled=excluded.enabled, updated_at=excluded.updated_at,
         javascript_disabled=excluded.javascript_disabled",
        params![
            domain,
            rule.body,
            rule.title,
            rule.author,
            rule.date,
            strip,
            enabled,
            crate::feeds::now_ms(),
            rule.javascript_disabled
        ],
    )?;
    Ok(())
}

pub fn delete(conn: &Connection, domain: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM site_rules WHERE domain=?1",
        [normalize_domain(domain)],
    )?;
    Ok(())
}

pub fn set_enabled(conn: &Connection, domain: &str, enabled: bool) -> Result<()> {
    conn.execute(
        "UPDATE site_rules SET enabled=?2, updated_at=?3 WHERE domain=?1",
        params![normalize_domain(domain), enabled, crate::feeds::now_ms()],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests;
