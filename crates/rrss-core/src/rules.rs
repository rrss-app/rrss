//! 规则 CRUD。日志 target：`rrss_core::rules`。

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::feeds::now_ms;

pub const DEFAULT_WEIGHT: i64 = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Polarity {
    Like,
    Dislike,
}

impl Polarity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Like => "like",
            Self::Dislike => "dislike",
        }
    }

    pub fn parse_str(value: &str) -> Result<Self> {
        match value {
            "like" => Ok(Self::Like),
            "dislike" => Ok(Self::Dislike),
            other => Err(Error::InvalidRule(format!("unknown polarity {other}"))),
        }
    }

    pub fn signed_weight(self, weight: i64) -> i64 {
        match self {
            Self::Like => weight,
            Self::Dislike => -weight,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Dimension {
    Author,
    Tag,
    TitleKw,
    TitleRe,
    UrlPhrase,
    UrlRe,
    BodyPhrase,
    BodyRe,
}

impl Dimension {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Author => "author",
            Self::Tag => "tag",
            Self::TitleKw => "title_kw",
            Self::TitleRe => "title_re",
            Self::UrlPhrase => "url_phrase",
            Self::UrlRe => "url_re",
            Self::BodyPhrase => "body_phrase",
            Self::BodyRe => "body_re",
        }
    }

    pub fn parse_str(value: &str) -> Result<Self> {
        match value {
            "author" => Ok(Self::Author),
            "tag" => Ok(Self::Tag),
            "title_kw" => Ok(Self::TitleKw),
            "title_re" => Ok(Self::TitleRe),
            "url_phrase" => Ok(Self::UrlPhrase),
            "url_re" => Ok(Self::UrlRe),
            "body_phrase" => Ok(Self::BodyPhrase),
            "body_re" => Ok(Self::BodyRe),
            other => Err(Error::InvalidRule(format!("unknown dimension {other}"))),
        }
    }

    pub fn is_regex(self) -> bool {
        matches!(self, Self::TitleRe | Self::UrlRe | Self::BodyRe)
    }

    pub fn label_zh(self) -> &'static str {
        match self {
            Self::Author => "作者",
            Self::Tag => "标签",
            Self::TitleKw => "标题",
            Self::TitleRe => "标题正则",
            Self::UrlPhrase => "URL",
            Self::UrlRe => "URL 正则",
            Self::BodyPhrase => "正文",
            Self::BodyRe => "正文正则",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScopeKind {
    Global,
    Folder,
    Feed,
}

impl ScopeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::Folder => "folder",
            Self::Feed => "feed",
        }
    }

    pub fn parse_str(value: &str) -> Result<Self> {
        match value {
            "global" => Ok(Self::Global),
            "folder" => Ok(Self::Folder),
            "feed" => Ok(Self::Feed),
            other => Err(Error::InvalidRule(format!("unknown scope {other}"))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleSource {
    Manual,
    Suggested,
}

impl RuleSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Suggested => "suggested",
        }
    }

    pub fn parse_str(value: &str) -> Result<Self> {
        match value {
            "manual" => Ok(Self::Manual),
            "suggested" => Ok(Self::Suggested),
            other => Err(Error::InvalidRule(format!("unknown source {other}"))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rule {
    pub id: i64,
    pub polarity: Polarity,
    pub dimension: Dimension,
    pub pattern: String,
    pub scope_kind: ScopeKind,
    pub scope_id: Option<i64>,
    pub weight: i64,
    pub source: RuleSource,
    pub enabled: bool,
    pub pack_id: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct RuleFilter {
    pub polarity: Option<Polarity>,
    pub dimension: Option<Dimension>,
    pub enabled: Option<bool>,
    pub source: Option<RuleSource>,
    pub scope_kind: Option<ScopeKind>,
    pub scope_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewRule {
    pub polarity: Polarity,
    pub dimension: Dimension,
    pub pattern: String,
    pub scope_kind: ScopeKind,
    pub scope_id: Option<i64>,
    pub weight: i64,
    pub source: RuleSource,
    pub enabled: bool,
    pub pack_id: Option<String>,
}

impl NewRule {
    pub fn manual_global(
        polarity: Polarity,
        dimension: Dimension,
        pattern: impl Into<String>,
    ) -> Self {
        Self {
            polarity,
            dimension,
            pattern: pattern.into(),
            scope_kind: ScopeKind::Global,
            scope_id: None,
            weight: DEFAULT_WEIGHT,
            source: RuleSource::Manual,
            enabled: true,
            pack_id: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleAction {
    Like,
    Dislike,
    Neutral,
}

pub fn validate_pattern(dimension: Dimension, pattern: &str) -> Result<()> {
    let pattern = pattern.trim();
    if pattern.is_empty() {
        return Err(Error::InvalidRule("empty pattern".into()));
    }
    if dimension.is_regex() {
        regex::RegexBuilder::new(pattern)
            .case_insensitive(true)
            .build()
            .map_err(|err| Error::InvalidRule(format!("invalid regex: {err}")))?;
    }
    Ok(())
}

fn resolve_scope(
    conn: &Connection,
    scope_kind: ScopeKind,
    scope_id: Option<i64>,
) -> Result<Option<i64>> {
    match scope_kind {
        ScopeKind::Global => Ok(None),
        ScopeKind::Feed => {
            let id =
                scope_id.ok_or_else(|| Error::InvalidRule("feed scope needs scope_id".into()))?;
            crate::feeds::get_feed(conn, id)?;
            Ok(Some(id))
        }
        ScopeKind::Folder => {
            let id =
                scope_id.ok_or_else(|| Error::InvalidRule("folder scope needs scope_id".into()))?;
            let exists: i64 =
                conn.query_row("SELECT COUNT(*) FROM folders WHERE id = ?1", [id], |row| {
                    row.get(0)
                })?;
            if exists == 0 {
                return Err(Error::FolderNotFound);
            }
            Ok(Some(id))
        }
    }
}

pub fn insert_rule(conn: &Connection, spec: NewRule) -> Result<i64> {
    let pattern = spec.pattern.trim();
    validate_pattern(spec.dimension, pattern)?;
    let weight = if spec.weight == 0 {
        DEFAULT_WEIGHT
    } else {
        spec.weight.abs()
    };
    let scope_id = resolve_scope(conn, spec.scope_kind, spec.scope_id)?;
    conn.execute(
        "INSERT INTO rules (polarity, dimension, pattern, scope_kind, scope_id, weight, source, enabled, pack_id, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            spec.polarity.as_str(),
            spec.dimension.as_str(),
            pattern,
            spec.scope_kind.as_str(),
            scope_id,
            weight,
            spec.source.as_str(),
            i64::from(spec.enabled),
            spec.pack_id,
            now_ms()
        ],
    )?;
    let id = conn.last_insert_rowid();
    tracing::info!(
        rule_id = id,
        dimension = spec.dimension.as_str(),
        polarity = spec.polarity.as_str(),
        pack_id = spec.pack_id.as_deref(),
        "rule inserted"
    );
    Ok(id)
}

pub fn update_rule(conn: &Connection, rule: &Rule) -> Result<()> {
    let pattern = rule.pattern.trim();
    validate_pattern(rule.dimension, pattern)?;
    let weight = if rule.weight == 0 {
        DEFAULT_WEIGHT
    } else {
        rule.weight.abs()
    };
    let scope_id = resolve_scope(conn, rule.scope_kind, rule.scope_id)?;
    let n = conn.execute(
        "UPDATE rules SET polarity = ?1, dimension = ?2, pattern = ?3, scope_kind = ?4,
            scope_id = ?5, weight = ?6, source = ?7, enabled = ?8, pack_id = ?9
         WHERE id = ?10",
        params![
            rule.polarity.as_str(),
            rule.dimension.as_str(),
            pattern,
            rule.scope_kind.as_str(),
            scope_id,
            weight,
            rule.source.as_str(),
            i64::from(rule.enabled),
            rule.pack_id,
            rule.id
        ],
    )?;
    if n == 0 {
        Err(Error::RuleNotFound)
    } else {
        tracing::info!(rule_id = rule.id, "rule updated");
        Ok(())
    }
}

pub fn get_rule(conn: &Connection, id: i64) -> Result<Rule> {
    conn.query_row(
        "SELECT id, polarity, dimension, pattern, scope_kind, scope_id, weight, source, enabled, pack_id
         FROM rules WHERE id = ?1",
        [id],
        rule_from_row,
    )
    .optional()?
    .ok_or(Error::RuleNotFound)
}

pub fn list_rules(conn: &Connection, filter: &RuleFilter) -> Result<Vec<Rule>> {
    let mut stmt = conn.prepare(
        "SELECT id, polarity, dimension, pattern, scope_kind, scope_id, weight, source, enabled, pack_id
         FROM rules
         ORDER BY CASE WHEN source = 'suggested' AND enabled = 0 THEN 0 ELSE 1 END, id DESC",
    )?;
    let rows = stmt.query_map([], rule_from_row)?;
    let mut out = Vec::new();
    for row in rows {
        let rule = row?;
        if filter.matches(&rule) {
            out.push(rule);
        }
    }
    Ok(out)
}

pub fn list_enabled_rules(conn: &Connection) -> Result<Vec<Rule>> {
    list_rules(
        conn,
        &RuleFilter {
            enabled: Some(true),
            ..RuleFilter::default()
        },
    )
}

pub fn set_rule_action(conn: &Connection, id: i64, action: RuleAction) -> Result<()> {
    let n = match action {
        RuleAction::Like => conn.execute(
            "UPDATE rules SET polarity = 'like', enabled = 1 WHERE id = ?1",
            [id],
        )?,
        RuleAction::Dislike => conn.execute(
            "UPDATE rules SET polarity = 'dislike', enabled = 1 WHERE id = ?1",
            [id],
        )?,
        RuleAction::Neutral => conn.execute("UPDATE rules SET enabled = 0 WHERE id = ?1", [id])?,
    };
    if n == 0 {
        Err(Error::RuleNotFound)
    } else {
        tracing::info!(rule_id = id, ?action, "rule action");
        Ok(())
    }
}

pub fn set_rule_enabled(conn: &Connection, id: i64, enabled: bool) -> Result<()> {
    let n = conn.execute(
        "UPDATE rules SET enabled = ?1 WHERE id = ?2",
        params![i64::from(enabled), id],
    )?;
    if n == 0 {
        Err(Error::RuleNotFound)
    } else {
        Ok(())
    }
}

/// 确认建议：启用并标成 manual，不再出现在待确认列表。
pub fn confirm_rule(conn: &Connection, id: i64) -> Result<()> {
    let n = conn.execute(
        "UPDATE rules SET enabled = 1, source = 'manual' WHERE id = ?1",
        [id],
    )?;
    if n == 0 {
        Err(Error::RuleNotFound)
    } else {
        tracing::info!(rule_id = id, "rule confirmed");
        Ok(())
    }
}

pub fn delete_rule(conn: &Connection, id: i64) -> Result<()> {
    let n = conn.execute("DELETE FROM rules WHERE id = ?1", [id])?;
    if n == 0 {
        Err(Error::RuleNotFound)
    } else {
        tracing::info!(rule_id = id, "rule deleted");
        Ok(())
    }
}

pub fn has_author_rule(conn: &Connection, author: &str) -> Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM rules WHERE dimension = 'author' AND lower(pattern) = lower(?1)",
        [author],
        |row| row.get(0),
    )?;
    Ok(n > 0)
}

impl RuleFilter {
    fn matches(&self, rule: &Rule) -> bool {
        self.polarity.is_none_or(|p| rule.polarity == p)
            && self.dimension.is_none_or(|d| rule.dimension == d)
            && self.enabled.is_none_or(|e| rule.enabled == e)
            && self.source.is_none_or(|s| rule.source == s)
            && self.scope_kind.is_none_or(|k| rule.scope_kind == k)
            && self.scope_id.is_none_or(|id| rule.scope_id == Some(id))
    }
}

fn rule_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Rule> {
    let polarity: String = row.get(1)?;
    let dimension: String = row.get(2)?;
    let scope_kind: String = row.get(4)?;
    let source: String = row.get(7)?;
    let map_err = |err: Error| {
        rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(err))
    };
    Ok(Rule {
        id: row.get(0)?,
        polarity: Polarity::parse_str(&polarity).map_err(map_err)?,
        dimension: Dimension::parse_str(&dimension).map_err(map_err)?,
        pattern: row.get(3)?,
        scope_kind: ScopeKind::parse_str(&scope_kind).map_err(map_err)?,
        scope_id: row.get(5)?,
        weight: row.get(6)?,
        source: RuleSource::parse_str(&source).map_err(map_err)?,
        enabled: row.get::<_, i64>(8)? != 0,
        pack_id: row.get(9)?,
    })
}
