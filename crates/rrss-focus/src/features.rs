//! 题材与作者词面特征分量：衰减计数与可解释词面画像，纯函数。

use rrss_core::{features::FeatureChip, rules::Dimension};
use std::collections::{BTreeMap, BTreeSet};

pub const HALF_LIFE_MS: i64 = super::evidence::LONG_HALF_LIFE_MS;
pub const MAX_WEIGHT: f64 = 2.0;
/// A transient topic stays below one explicit matching feature, even after
/// adding its optional vector contribution and rounding the combined score.
pub const RECENT_SCORE_LIMIT: f64 = 0.25;

const STOP_WORDS: &[&str] = &[
    // English function words (2+ letters)
    "the", "and", "for", "with", "from", "this", "that", "are", "to", "in", "of", "on", "is", "at",
    "by", "an", "as", "be", "it", "or", "we", "my", "no", "up", "do", "so", "if", "not", "but",
    "all", "can", "has", "was", "its", "you", "your", "how", "why", "new", "out", "one", "two",
    "more", "what", "when", "will", "have", "been", "into", "over", "than", "they", "them",
    "their", "about", "after", "could", "would", "should", "there", "where", "which", "while",
    // Chinese structural bigrams
    "如何", "我们", "一个", "可以", "什么", "以及", "关于", "最新", "你的", "这个", "不是", "已经",
    "正在", "开始", "使用", "怎么", "那些", "这些", "自己", "他们", "通过", "进行", "成为", "需要",
    "可能", "没有", "如果", "因为", "所以", "为什", "什么",
];

/// Same-source explicit dislike amplification: net explicit dislikes within the
/// same feed add extra negative weight without leaking to other feeds.
const SAME_FEED_DISLIKE_SCALE: f64 = 1.5;
const SAME_FEED_DISLIKE_MAX: f64 = 3.0;

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
                && !pattern.chars().all(|c| c.is_ascii_digit())
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
    /// Per-feed explicit [positive, negative] totals for same-source dislike focus.
    feed_explicit: BTreeMap<(i64, Key), [f64; 2]>,
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
        let with_feeds: Vec<_> = feedback
            .iter()
            .map(|&(chips, w)| (chips, None, w))
            .collect();
        Self::learn_weighted_with_feeds(&with_feeds)
    }

    pub(crate) fn learn_weighted_with_feeds(
        feedback: &[(&[FeatureChip], Option<i64>, super::evidence::Weighted)],
    ) -> Self {
        let mut counts = BTreeMap::<Key, Counts>::new();
        let mut feed_explicit = BTreeMap::<(i64, Key), [f64; 2]>::new();
        for (chips, feed, f) in feedback {
            if f.long == 0.0 {
                continue;
            }
            let sign = usize::from(f.long < 0.0);
            for k in keys(chips) {
                let count = counts.entry(k.clone()).or_default();
                if f.explicit {
                    count.explicit[sign] += f.long.abs();
                    if let Some(feed_id) = *feed {
                        feed_explicit.entry((feed_id, k)).or_default()[sign] += f.long.abs();
                    }
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
            feed_explicit,
        }
    }

    /// Article-level scaled global lexical score clamped to ±MAX_FEATURE_SCORE,
    /// matching the bounded units of the manual combiner and avoiding long-title blowup.
    pub(crate) fn bounded_score(&self, chips: &[FeatureChip]) -> f64 {
        self.weighted(chips)
            .iter()
            .map(|(_, w)| w)
            .sum::<f64>()
            .clamp(
                -super::combine::MAX_FEATURE_SCORE,
                super::combine::MAX_FEATURE_SCORE,
            )
    }

    /// Extra negative score from net explicit dislikes on this exact feed (<= 0.0).
    /// Kept separate from global `bounded_score` so fitted `beta[0]` noise cannot mute
    /// explicit thumbs-down, while never leaking to other feeds.
    pub(crate) fn same_feed_dislike_penalty(&self, feed: i64, chips: &[FeatureChip]) -> f64 {
        let mut seen = BTreeSet::new();
        let mut penalties = Vec::new();
        for c in chips {
            let k = key(c);
            if !seen.insert(k.clone()) {
                continue;
            }
            if let Some(&[pos, neg]) = self.feed_explicit.get(&(feed, k)) {
                let excess_neg = (neg - pos).max(0.0);
                if excess_neg > 0.0 {
                    penalties.push(
                        (SAME_FEED_DISLIKE_SCALE * (1.0 + excess_neg).ln())
                            .min(SAME_FEED_DISLIKE_MAX),
                    );
                }
            }
        }
        let sum: f64 = penalties.iter().sum();
        let max = penalties.iter().copied().fold(0.0, f64::max);
        if sum <= 0.0 {
            return 0.0;
        }
        let scale = (2.0 * max / sum).min(1.0);
        -(sum * scale).min(super::combine::MAX_FEATURE_SCORE)
    }

    /// Only title and tag evidence transfers. Author/domain identity is never a
    /// shortcut for source preference. A long title cannot multiply one vote.
    pub fn weighted(&self, chips: &[FeatureChip]) -> Vec<(FeatureChip, f64)> {
        self.weighted_inner(None, chips)
    }

    /// Same as [`weighted`], additionally amplifying net explicit dislikes that
    /// occurred on the same feed without leaking that extra penalty to other feeds.
    pub(crate) fn weighted_for_feed(
        &self,
        feed: i64,
        chips: &[FeatureChip],
    ) -> Vec<(FeatureChip, f64)> {
        self.weighted_inner(Some(feed), chips)
    }

    fn weighted_inner(&self, feed: Option<i64>, chips: &[FeatureChip]) -> Vec<(FeatureChip, f64)> {
        let mut seen = BTreeSet::new();
        let matches: Vec<_> = chips
            .iter()
            .filter_map(|c| {
                let k = key(c);
                if !seen.insert(k.clone()) {
                    return None;
                }
                let (mut long, recent) = *self.weights.get(&k)?;
                if let Some(feed_id) = feed {
                    if let Some(&[pos, neg]) = self.feed_explicit.get(&(feed_id, k)) {
                        let excess_neg = (neg - pos).max(0.0);
                        if excess_neg > 0.0 {
                            long -= (SAME_FEED_DISLIKE_SCALE * (1.0 + excess_neg).ln())
                                .min(SAME_FEED_DISLIKE_MAX);
                        }
                    }
                }
                Some((c, (long, recent)))
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

    #[test]
    fn numbers_stopwords_and_brackets_do_not_leak_and_same_feed_dislikes_stay_scoped() {
        let disliked = rrss_core::features::extract_features_with_bigrams(
            None,
            &[],
            "【开发日志】#12 in 2026: Tidewater refactor",
            None,
            None,
            true,
        );
        let unrelated_other_feed = rrss_core::features::extract_features_with_bigrams(
            None,
            &[],
            "Garden notes in 2026 #12",
            None,
            None,
            true,
        );
        let same_series = rrss_core::features::extract_features_with_bigrams(
            None,
            &[],
            "【开发日志】#13 Tidewater parser",
            None,
            None,
            true,
        );
        let weighted = super::super::evidence::Weighted {
            article_id: 1,
            day: 0,
            explicit: true,
            long: -1.0,
            recent: 0.0,
        };
        let profile = Profile::learn_weighted_with_feeds(&[(&disliked, Some(1), weighted)]);
        // Pure digits ("12", "2026") and English stopwords ("in") must not leak.
        assert!(
            profile
                .weighted_for_feed(2, &unrelated_other_feed)
                .is_empty()
        );
        // Same-feed explicit dislike is stronger on feed 1 than on feed 2.
        let sum = |v: &[(FeatureChip, f64)]| v.iter().map(|(_, w)| w).sum::<f64>();
        assert!(
            sum(&profile.weighted_for_feed(1, &same_series))
                < sum(&profile.weighted_for_feed(2, &same_series))
        );
        assert!(profile.same_feed_dislike_penalty(1, &same_series) < -0.5);
        assert_eq!(profile.same_feed_dislike_penalty(2, &same_series), 0.0);
    }
}
