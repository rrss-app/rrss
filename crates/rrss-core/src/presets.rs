//! 编译进二进制的规则预设包。日志 target：`rrss_core::presets`。

use crate::error::rules::RuleProblem;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::rules::{self, Dimension, NewRule, Polarity, RuleSource, ScopeKind};

const PACK_SOURCES: &[(&str, &str)] = &[
    (
        "ads-and-sponsored",
        include_str!("presets/ads-and-sponsored.toml"),
    ),
    ("clickbait-cn", include_str!("presets/clickbait-cn.toml")),
    ("promo-cn", include_str!("presets/promo-cn.toml")),
    ("dev-focus", include_str!("presets/dev-focus.toml")),
    ("crypto-web3", include_str!("presets/crypto-web3.toml")),
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PresetPack {
    pub id: String,
    pub name: String,
    pub description: String,
    pub polarity_default: Polarity,
    pub rules: Vec<PresetRule>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PresetRule {
    pub dimension: Dimension,
    pub pattern: String,
    pub weight: i64,
    pub polarity: Polarity,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackStatus {
    pub pack: PresetPack,
    pub installed: bool,
    pub enabled_count: i64,
}

#[derive(Debug, Deserialize)]
struct PackFile {
    id: String,
    name: String,
    description: String,
    polarity_default: String,
    rules: Vec<RuleFile>,
}

#[derive(Debug, Deserialize)]
struct RuleFile {
    dimension: String,
    pattern: String,
    #[serde(default)]
    weight: Option<i64>,
    #[serde(default)]
    polarity: Option<String>,
}

fn parse_pack(raw: &str) -> Result<PresetPack> {
    let file: PackFile =
        toml::from_str(raw).map_err(|_| Error::InvalidRule(RuleProblem::PresetInvalid))?;
    let polarity_default = Polarity::parse_str(&file.polarity_default)?;
    let mut rules = Vec::new();
    for rule in file.rules {
        let dimension = Dimension::parse_str(&rule.dimension)?;
        let polarity = match rule.polarity.as_deref() {
            Some(p) => Polarity::parse_str(p)?,
            None => polarity_default,
        };
        rules::validate_pattern(dimension, &rule.pattern)?;
        rules.push(PresetRule {
            dimension,
            pattern: rule.pattern,
            weight: rule.weight.unwrap_or(rules::DEFAULT_WEIGHT),
            polarity,
        });
    }
    Ok(PresetPack {
        id: file.id,
        name: file.name,
        description: file.description,
        polarity_default,
        rules,
    })
}

fn load_all() -> Result<Vec<PresetPack>> {
    PACK_SOURCES
        .iter()
        .map(|(_, raw)| parse_pack(raw))
        .collect()
}

pub fn list_packs() -> Result<Vec<PresetPack>> {
    load_all()
}

pub fn pack_by_id(id: &str) -> Result<PresetPack> {
    load_all()?
        .into_iter()
        .find(|p| p.id == id)
        .ok_or_else(|| Error::PresetNotFound(id.into()))
}

pub fn installed_packs(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT DISTINCT pack_id FROM rules WHERE pack_id IS NOT NULL AND pack_id != '' ORDER BY pack_id",
    )?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

pub fn pack_statuses(conn: &Connection) -> Result<Vec<PackStatus>> {
    let packs = list_packs()?;
    let mut out = Vec::with_capacity(packs.len());
    for pack in packs {
        let enabled_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM rules WHERE pack_id = ?1 AND enabled = 1",
            [&pack.id],
            |row| row.get(0),
        )?;
        let total: i64 = conn.query_row(
            "SELECT COUNT(*) FROM rules WHERE pack_id = ?1",
            [&pack.id],
            |row| row.get(0),
        )?;
        out.push(PackStatus {
            installed: total > 0,
            enabled_count,
            pack,
        });
    }
    Ok(out)
}

pub fn apply_pack(conn: &Connection, id: &str) -> Result<u32> {
    let pack = pack_by_id(id)?;
    let mut n = 0u32;
    for rule in &pack.rules {
        let exists: i64 = conn.query_row(
            "SELECT COUNT(*) FROM rules WHERE pack_id = ?1 AND dimension = ?2 AND pattern = ?3",
            rusqlite::params![pack.id, rule.dimension.as_str(), rule.pattern],
            |row| row.get(0),
        )?;
        if exists > 0 {
            continue;
        }
        rules::insert_rule(
            conn,
            NewRule {
                polarity: rule.polarity,
                dimension: rule.dimension,
                pattern: rule.pattern.clone(),
                scope_kind: ScopeKind::Global,
                scope_id: None,
                weight: rule.weight,
                source: RuleSource::Manual,
                enabled: true,
                pack_id: Some(pack.id.clone()),
            },
        )?;
        n += 1;
    }
    tracing::info!(pack_id = %pack.id, inserted = n, "applied preset pack");
    Ok(n)
}

pub fn remove_pack(conn: &Connection, id: &str) -> Result<u32> {
    let n = conn.execute("DELETE FROM rules WHERE pack_id = ?1", [id])?;
    tracing::info!(pack_id = id, deleted = n, "removed preset pack");
    Ok(n as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_at;
    use tempfile::tempdir;

    #[test]
    fn bundled_packs_parse() {
        let packs = list_packs().unwrap();
        assert_eq!(packs.len(), 5);
        let dev = packs.iter().find(|p| p.id == "dev-focus").unwrap();
        assert_eq!(dev.polarity_default, Polarity::Like);
        assert!(dev.rules.iter().any(|r| r.pattern == "Rust"));
        let ads = packs.iter().find(|p| p.id == "ads-and-sponsored").unwrap();
        assert_eq!(ads.polarity_default, Polarity::Dislike);
    }

    #[test]
    fn apply_and_remove_is_idempotent() {
        let dir = tempdir().unwrap();
        let conn = open_at(&dir.path().join("rrss.db")).unwrap();
        let n = apply_pack(&conn, "dev-focus").unwrap();
        assert!(n >= 1);
        assert_eq!(apply_pack(&conn, "dev-focus").unwrap(), 0);
        let status = pack_statuses(&conn)
            .unwrap()
            .into_iter()
            .find(|s| s.pack.id == "dev-focus")
            .unwrap();
        assert!(status.installed);
        assert!(status.enabled_count > 0);
        assert!(remove_pack(&conn, "dev-focus").unwrap() > 0);
        assert_eq!(remove_pack(&conn, "dev-focus").unwrap(), 0);
        assert!(
            !pack_statuses(&conn)
                .unwrap()
                .into_iter()
                .find(|s| s.pack.id == "dev-focus")
                .unwrap()
                .installed
        );
    }

    #[test]
    fn unknown_pack_is_error() {
        let dir = tempdir().unwrap();
        let conn = open_at(&dir.path().join("rrss.db")).unwrap();
        let err = apply_pack(&conn, "no-such-pack").unwrap_err();
        assert!(matches!(err, Error::PresetNotFound(_)));
    }
}
