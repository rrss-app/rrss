//! Filter rule, preset and feature contracts.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct RuleStatsView {
    pub rule_id: i64,
    pub hits_30d: i64,
    pub last_hit_at: Option<i64>,
    pub hidden_count: i64,
    pub advice: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct SpotCheckStatsView {
    pub exposed: i64,
    pub liked: i64,
    pub disliked: i64,
    pub insufficient: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct RuleStatsPage {
    pub rules: Vec<RuleStatsView>,
    pub spot_check: SpotCheckStatsView,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct FeatureView {
    pub dimension: String,
    pub pattern: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum RuleEffect {
    Boost,
    Hide,
    Lower,
    None,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "code", content = "params", rename_all = "snake_case")]
pub enum RuleSummaryView {
    RuleSummary { effect: RuleEffect },
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct RuleView {
    pub id: i64,
    pub polarity: String,
    pub dimension_id: String,
    pub pattern: String,
    pub summary: RuleSummaryView,
    // Current source or folder name is content data.
    pub scope_name: String,
    pub scope_kind: String,
    pub scope_id: i64,
    pub weight: i64,
    pub source: String,
    pub enabled: bool,
    pub pending: bool,
    pub pack_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct PresetView {
    pub id: String,
    pub name: String,
    pub description: String,
    pub rule_count: i64,
    pub enabled_count: i64,
    pub installed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ScopeOption {
    pub id: i64,
    // Current source or folder name is content data.
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct RulesPage {
    pub rules: Vec<RuleView>,
    pub presets: Vec<PresetView>,
    pub folders: Vec<ScopeOption>,
    pub feeds: Vec<ScopeOption>,
    pub hide_threshold: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct NewRuleSpec {
    pub polarity: String,
    pub dimension: String,
    pub pattern: String,
    pub weight: i64,
    pub scope_kind: String,
    pub scope_id: i64,
}
