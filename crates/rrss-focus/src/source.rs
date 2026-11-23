//! Recommendation source preference: engaged reading per real Focus opportunity.

use super::{
    evidence::DAY_MS,
    features,
    labels::{Evidence, LabelExposure},
};
use rrss_core::{features::FeatureChip, rules::Dimension};
use std::collections::{BTreeMap, BTreeSet};

pub const WINDOW_MS: i64 = 60 * DAY_MS;
pub const PRIOR_EXPOSURES: f64 = 20.0;
/// The frontend keeps the first exposure ID for an article throughout a launch.
/// A link alone is not proof of a later reading entry point.
pub const ATTRIBUTION_WINDOW_MS: i64 = DAY_MS;

pub struct Feedback<'a> {
    pub evidence: Evidence,
    pub exposure_id: i64,
    pub features: &'a [FeatureChip],
}

struct Topic {
    tags: BTreeSet<features::Key>,
    title: BTreeSet<features::Key>,
}

impl Topic {
    fn from_features(chips: &[FeatureChip]) -> Self {
        let (tags, title) = features::keys(chips)
            .into_iter()
            .partition(|(dimension, _)| *dimension == Dimension::Tag.as_str());
        Self { tags, title }
    }

    fn distinct_from(&self, other: &Self) -> bool {
        // Tags carry editorial topic information. Repeated title templates must
        // not erase it; when either side has no tags, compare actual title words.
        let (left, right) = if !self.tags.is_empty() && !other.tags.is_empty() {
            (&self.tags, &other.tags)
        } else {
            (&self.title, &other.title)
        };
        !left.is_empty()
            && !right.is_empty()
            && left.intersection(right).count() * 2 < left.union(right).count()
    }
}

pub fn learn(
    exposures: &[(LabelExposure, i64)],
    feedback: &[Feedback<'_>],
    now: i64,
) -> BTreeMap<i64, f64> {
    let exposures: BTreeMap<_, _> = exposures
        .iter()
        .filter(|(e, _)| e.shown_at < now && e.shown_at >= now.saturating_sub(WINDOW_MS))
        .map(|(e, f)| (e.id, (e, *f)))
        .collect();
    // One opportunity and outcome per article/day, even across launches.
    let mut opportunities = BTreeMap::<(i64, i64, i64), f64>::new();
    for (e, feed) in exposures.values() {
        opportunities
            .entry((*feed, e.shown_at.div_euclid(DAY_MS), e.article_id))
            .or_default();
    }
    let mut positive_days = BTreeMap::<i64, BTreeSet<i64>>::new();
    let mut topics = BTreeMap::<i64, Vec<Topic>>::new();
    let mut qualified = false;
    for f in feedback {
        let v = f.evidence;
        let Some((e, feed)) = exposures.get(&f.exposure_id) else {
            continue;
        };
        if e.article_id != v.article_id
            || v.at < e.shown_at
            || v.at >= now
            || v.at.saturating_sub(e.shown_at) > ATTRIBUTION_WINDOW_MS
        {
            continue;
        }
        if v.weight == 0.0 || !v.weight.is_finite() {
            continue;
        }
        qualified = true;
        let reward = if v.explicit {
            v.weight.clamp(0.0, 1.0)
        } else {
            if v.weight > 1.0 {
                1.0
            } else if v.weight > 0.0 {
                0.75
            } else {
                0.0
            }
        };
        let day = e.shown_at.div_euclid(DAY_MS);
        let outcome = opportunities.get_mut(&(*feed, day, e.article_id)).unwrap();
        *outcome = outcome.max(reward);
        if reward > 0.0 {
            positive_days
                .entry(*feed)
                .or_default()
                .insert(v.at.div_euclid(DAY_MS));
            let topic = Topic::from_features(f.features);
            let groups = topics.entry(*feed).or_default();
            if (!topic.tags.is_empty() || !topic.title.is_empty())
                && groups.len() < 2
                && groups.iter().all(|g| topic.distinct_from(g))
            {
                groups.push(topic);
            }
        }
    }
    let mut daily = BTreeMap::<(i64, i64), (f64, f64)>::new();
    for ((feed, day, _), reward) in opportunities {
        let c = daily.entry((feed, day)).or_default();
        c.0 += 1.0;
        c.1 += reward;
    }
    let mut counts = BTreeMap::<i64, (f64, f64, usize)>::new();
    for ((feed, _), (n, successes)) in daily {
        let scale = (3.0 / n).min(1.0);
        let c = counts.entry(feed).or_default();
        c.0 += n * scale;
        c.1 += successes * scale;
        c.2 += 1;
    }
    let (n, successes) = counts
        .values()
        .fold((0.0, 0.0), |(n, s), c| (n + c.0, s + c.1));
    if n == 0.0 {
        return BTreeMap::new();
    }
    // No attributable feedback means unknown, not a failed reading trial.
    if !qualified {
        return counts.into_keys().map(|feed| (feed, 0.0)).collect();
    }
    // Beta(2,2) regularizes both outcomes. Keep the reference below certainty:
    // even an all-engaged library can establish a positive preference over time.
    let mean = ((successes + 2.0) / (n + 4.0)).clamp(0.25, 0.5);
    let log_odds = |p: f64| (p / (1.0 - p)).ln();
    counts
        .into_iter()
        .map(|(feed, (n, successes, days))| {
            let posterior = (successes + PRIOR_EXPOSURES * mean) / (n + PRIOR_EXPOSURES);
            let raw = log_odds(posterior) - log_odds(mean);
            let broad = positive_days.get(&feed).is_some_and(|d| d.len() >= 3)
                && topics.get(&feed).is_some_and(|t| t.len() >= 2);
            let score = if days < 3 || (raw > 0.0 && !broad) {
                0.0
            } else {
                raw.clamp(-0.5, 0.5)
            };
            (feed, score)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn extra_daily_supply_cannot_multiply_opportunities_or_source_reward() {
        let a = rrss_core::features::extract_features(None, &[], "Orbital mechanics", None, None);
        let b = rrss_core::features::extract_features(None, &[], "Garden plants", None, None);
        let learn_count = |per_day: i64, days: i64| {
            let mut exposures = Vec::new();
            let mut feedback = Vec::new();
            for day in 0..days {
                for feed in 1..=2 {
                    for n in 0..per_day {
                        let id = (day * 2 + feed) * per_day + n;
                        exposures.push((
                            LabelExposure {
                                id,
                                article_id: id,
                                position: n,
                                shown_at: day * DAY_MS,
                            },
                            feed,
                        ));
                        if feed == 1 {
                            feedback.push(Feedback {
                                evidence: Evidence {
                                    article_id: id,
                                    weight: 2.0,
                                    at: day * DAY_MS + 1,
                                    explicit: false,
                                    exposure_id: Some(id),
                                },
                                exposure_id: id,
                                features: if n % 2 == 0 { &a } else { &b },
                            });
                        }
                    }
                }
            }
            learn(&exposures, &feedback, days * DAY_MS)
        };
        assert_eq!(learn_count(3, 10), learn_count(300, 10));
        assert_eq!(learn_count(300, 1)[&1], 0.0);
    }
}
