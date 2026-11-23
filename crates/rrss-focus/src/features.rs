//! 阶段 23「B1 基础层 · 特征偏好」：衰减计数与可解释词面画像，纯函数。

use rrss_core::{features::FeatureChip, rules::Dimension};
use std::collections::{BTreeMap, BTreeSet};

pub const HALF_LIFE_MS: i64 = super::evidence::LONG_HALF_LIFE_MS;
pub const MAX_WEIGHT: f64 = 2.0;
/// A transient topic stays below one explicit matching feature, even after
/// adding its optional vector contribution and rounding the combined score.
pub const RECENT_SCORE_LIMIT: f64 = 0.25;

const STOP_WORDS: &[&str] = &[
    "the", "and", "for", "with", "from", "this", "that", "are", "如何", "我们", "一个", "可以",
    "什么", "以及", "关于", "最新",
];

pub(crate) type Key = (&'static str, String);

fn key(chip: &FeatureChip) -> Key {
    (chip.dimension.as_str(), chip.pattern.to_lowercase())
}

pub(crate) fn keys(chips: &[FeatureChip]) -> BTreeSet<Key> {
    chips
        .iter()
        .map(key)
        .filter(|(dimension, pattern)| {
            (*dimension == Dimension::Tag.as_str() || *dimension == Dimension::TitleKw.as_str())
                && !STOP_WORDS.contains(&pattern.as_str())
        })
        .collect()
}

pub struct Feedback<'a> {
    pub features: &'a [FeatureChip],
    pub weight: f64,
    pub at: i64,
}

#[derive(Default)]
pub struct Profile {
    weights: BTreeMap<Key, (f64, f64)>,
}

#[derive(Default)]
struct Counts {
    explicit: [f64; 2],
    implicit: [f64; 2],
    recent: [f64; 2],
    days: [BTreeSet<i64>; 2],
}

impl Profile {
    pub(crate) fn reliable_keys(&self, weighted: &[(FeatureChip, f64)]) -> Vec<(String, f64)> {
        let mut keys: Vec<_> = weighted
            .iter()
            .filter_map(|(chip, w)| {
                let k = key(chip);
                let long = self.weights.get(&k)?.0;
                (long >= super::topics::MIN_SUPPORT && *w > 0.0)
                    .then(|| (format!("{}:{}", k.0, k.1), *w))
            })
            .collect();
        keys.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        keys.truncate(super::topics::MAX_KEYS);
        keys
    }

    /// Compatibility entry for callers supplying independent explicit observations.
    pub fn learn(_corpus: &[Vec<FeatureChip>], feedback: &[Feedback<'_>], now: i64) -> Self {
        let weighted: Vec<_> = feedback
            .iter()
            .enumerate()
            .filter(|(_, f)| f.at < now)
            .map(|(id, f)| {
                (
                    f.features,
                    super::evidence::Weighted {
                        article_id: id as i64,
                        day: f.at.div_euclid(super::evidence::DAY_MS),
                        explicit: true,
                        long: f.weight
                            * 2_f64.powf(-now.saturating_sub(f.at) as f64 / HALF_LIFE_MS as f64),
                        recent: 0.0,
                    },
                )
            })
            .collect();
        Self::learn_weighted(&weighted)
    }

    pub(crate) fn learn_weighted(feedback: &[(&[FeatureChip], super::evidence::Weighted)]) -> Self {
        let mut counts = BTreeMap::<Key, Counts>::new();
        for (chips, f) in feedback {
            if f.long == 0.0 {
                continue;
            }
            let sign = usize::from(f.long < 0.0);
            for k in keys(chips) {
                let count = counts.entry(k).or_default();
                if f.explicit {
                    count.explicit[sign] += f.long.abs();
                } else {
                    count.implicit[sign] += f.long.abs();
                    count.recent[sign] += f.recent.abs();
                    count.days[sign].insert(f.day);
                }
            }
        }
        Self {
            weights: counts
                .into_iter()
                .map(|(key, c)| {
                    let long = [0, 1].map(|s| {
                        c.explicit[s]
                            + c.implicit[s] * super::evidence::persistence(c.days[s].len())
                    });
                    let long = ((1.0 + long[0]) / (1.0 + long[1]))
                        .ln()
                        .clamp(-MAX_WEIGHT, MAX_WEIGHT);
                    let recent = 0.6 * ((1.0 + c.recent[0]) / (1.0 + c.recent[1])).ln();
                    (
                        key,
                        (long, recent.clamp(-RECENT_SCORE_LIMIT, RECENT_SCORE_LIMIT)),
                    )
                })
                .collect(),
        }
    }

    /// Experimental predictor: sum feature weights before article-level scaling.
    pub(crate) fn raw_score(&self, chips: &[FeatureChip]) -> f64 {
        keys(chips)
            .iter()
            .filter_map(|k| self.weights.get(k))
            .map(|(long, recent)| long + recent)
            .sum()
    }

    /// Only title and tag evidence transfers. Author/domain identity is never a
    /// shortcut for source preference. A long title cannot multiply one vote.
    pub fn weighted(&self, chips: &[FeatureChip]) -> Vec<(FeatureChip, f64)> {
        let mut seen = BTreeSet::new();
        let matches: Vec<_> = chips
            .iter()
            .filter_map(|c| {
                let k = key(c);
                if !seen.insert(k.clone()) {
                    return None;
                }
                self.weights.get(&k).map(|w| (c, *w))
            })
            .collect();
        let long_sum: f64 = matches.iter().map(|(_, w)| w.0.abs()).sum();
        let long_max = matches.iter().map(|(_, w)| w.0.abs()).fold(0.0, f64::max);
        let recent_sum: f64 = matches.iter().map(|(_, w)| w.1.abs()).sum();
        let long_scale = if long_sum > 0.0 {
            (2.0 * long_max / long_sum).min(1.0)
        } else {
            0.0
        };
        let recent_scale = if recent_sum > 0.0 {
            (RECENT_SCORE_LIMIT / recent_sum).min(1.0)
        } else {
            0.0
        };
        matches
            .into_iter()
            .filter_map(|(c, (long, recent))| {
                let w = long * long_scale + recent * recent_scale;
                (w.abs() > f64::EPSILON).then(|| (c.clone(), w))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sparse_explicit_topic_transfers_but_long_titles_stopwords_and_identity_do_not_multiply_it() {
        let chips = rrss_core::features::extract_features_with_bigrams(
            Some("Author"),
            &["orbit".into()],
            "Orbital mechanics spacecraft navigation propulsion landing research satellite the",
            Some("https://journal.example/a"),
            None,
            true,
        );
        let profile = Profile::learn(
            &[],
            &[Feedback {
                features: &chips,
                weight: 1.0,
                at: 0,
            }],
            1,
        );
        let weighted = profile.weighted(&chips);
        assert!(weighted.iter().all(|(c, _)| matches!(
            c.dimension,
            Dimension::Tag | Dimension::TitleKw
        ) && c.pattern != "the"));
        assert!(weighted.iter().map(|(_, w)| w).sum::<f64>() <= 2.0 * 2_f64.ln());
        let other = rrss_core::features::extract_features(
            Some("Author"),
            &[],
            "Cooking",
            Some("https://journal.example/b"),
            None,
        );
        assert!(profile.weighted(&other).is_empty());
        assert!(
            Profile::learn(
                &[],
                &[Feedback {
                    features: &chips,
                    weight: 1.0,
                    at: 1
                }],
                1
            )
            .weighted(&chips)
            .is_empty()
        );
    }
}
