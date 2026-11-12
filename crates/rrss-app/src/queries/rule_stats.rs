use rusqlite::Connection;

use crate::views::*;
use rrss_core::{feeds, rule_stats};

pub fn rule_stats(conn: &Connection) -> rrss_core::Result<RuleStatsPage> {
    let now = feeds::now_ms();
    let rules = rule_stats::list_rule_stats(conn, now)?
        .into_iter()
        .map(|rule| RuleStatsView {
            rule_id: rule.rule_id,
            hits_30d: rule.hits_30d,
            last_hit_at: rule.last_hit_at,
            hidden_count: rule.hidden_count,
            advice: rule.advice,
        })
        .collect();
    let spot = rule_stats::spot_check_stats(conn, now)?;
    Ok(RuleStatsPage {
        rules,
        spot_check: SpotCheckStatsView {
            exposed: spot.exposed,
            liked: spot.liked,
            disliked: spot.disliked,
            insufficient: spot.insufficient,
        },
    })
}
