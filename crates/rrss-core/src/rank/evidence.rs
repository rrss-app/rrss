//! Recommendation learning: shared temporal weights for lexical and vector profiles.

use super::labels::Evidence;
use std::collections::BTreeMap;

pub const DAY_MS: i64 = 86_400_000;
pub const LONG_HALF_LIFE_MS: i64 = 30 * DAY_MS;
pub const RECENT_HALF_LIFE_MS: i64 = 2 * DAY_MS;
pub const RECENT_WINDOW_MS: i64 = 7 * DAY_MS;
/// Absolute implicit evidence per source/day/sign; explicit state has its own channel.
pub const DAILY_BUDGET: f64 = 2.0;

#[derive(Clone, Copy)]
pub struct Weighted {
    pub article_id: i64,
    pub day: i64,
    pub explicit: bool,
    pub long: f64,
    pub recent: f64,
}

pub fn weight(votes: &[(Evidence, i64)], now: i64) -> Vec<Weighted> {
    let mut totals = BTreeMap::<(i64, i64, bool), f64>::new();
    for (v, feed) in votes.iter().filter(|(v, _)| v.at < now && !v.explicit) {
        *totals
            .entry((*feed, v.at.div_euclid(DAY_MS), v.weight > 0.0))
            .or_default() += v.weight.abs();
    }
    votes
        .iter()
        .filter(|(v, _)| v.at < now)
        .map(|(v, feed)| {
            let day = v.at.div_euclid(DAY_MS);
            let base = if v.explicit {
                v.weight
            } else {
                v.weight * (DAILY_BUDGET / totals[&(*feed, day, v.weight > 0.0)]).min(1.0)
            };
            let age = now.saturating_sub(v.at);
            Weighted {
                article_id: v.article_id,
                day,
                explicit: v.explicit,
                long: base * 2_f64.powf(-age as f64 / LONG_HALF_LIFE_MS as f64),
                recent: if v.explicit || age >= RECENT_WINDOW_MS {
                    0.0
                } else {
                    base * 2_f64.powf(-age as f64 / RECENT_HALF_LIFE_MS as f64)
                },
            }
        })
        .collect()
}

/// One day is transient; repeated days gradually establish a durable topic.
pub fn persistence(days: usize) -> f64 {
    (days.saturating_sub(1) as f64 / 3.0).min(1.0)
}
