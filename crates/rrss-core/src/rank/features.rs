//! 阶段 23「B1 基础层 · 特征偏好」：衰减计数与可解释词面画像，纯函数。

use crate::{features::FeatureChip, rules::Dimension};
use std::collections::{BTreeMap, BTreeSet};

pub const HALF_LIFE_MS: i64 = 30 * 86_400_000;
pub const MIN_FEEDBACK: usize = 10;
pub const MAX_WEIGHT: f64 = 2.0;

const STOP_WORDS: &[&str] = &[
    "the", "and", "for", "with", "from", "this", "that", "are", "如何", "我们", "一个", "可以",
    "什么", "以及", "关于", "最新",
];

type Key = (&'static str, String);

fn key(chip: &FeatureChip) -> Key {
    (chip.dimension.as_str(), chip.pattern.to_lowercase())
}

fn keys(chips: &[FeatureChip]) -> BTreeSet<Key> {
    chips
        .iter()
        .map(key)
        .filter(|(dimension, pattern)| {
            *dimension != Dimension::TitleKw.as_str() || !STOP_WORDS.contains(&pattern.as_str())
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
    weights: BTreeMap<Key, f64>,
}

impl Profile {
    /// corpus 是截止学习时间的全部文章特征（含无反馈文章），每篇重复特征只算一次。
    pub fn learn(corpus: &[Vec<FeatureChip>], feedback: &[Feedback<'_>], now: i64) -> Self {
        let feedback: Vec<_> = feedback
            .iter()
            .filter(|f| f.at < now && f.weight != 0.0)
            .collect();
        if feedback.len() < MIN_FEEDBACK {
            return Self::default();
        }
        let mut frequency = BTreeMap::<Key, usize>::new();
        for article in corpus {
            for k in keys(article) {
                *frequency.entry(k).or_default() += 1;
            }
        }
        let mut counts = BTreeMap::<Key, (f64, f64)>::new();
        for f in feedback {
            let decay = 2_f64.powf(-(now.saturating_sub(f.at) as f64) / HALF_LIFE_MS as f64);
            for k in keys(f.features) {
                if frequency.get(&k).copied().unwrap_or(0) < 3 {
                    continue;
                }
                let count = counts.entry(k).or_default();
                if f.weight > 0.0 {
                    count.0 += f.weight * decay;
                } else {
                    count.1 += -f.weight * decay;
                }
            }
        }
        Self {
            weights: counts
                .into_iter()
                .map(|(k, (p, n))| {
                    (
                        k,
                        ((p + 1.0) / (n + 1.0)).ln().clamp(-MAX_WEIGHT, MAX_WEIGHT),
                    )
                })
                .collect(),
        }
    }

    /// 权重保留原维度与可读特征，供组合原因与后续规则建议读取。
    pub fn weighted(&self, chips: &[FeatureChip]) -> Vec<(FeatureChip, f64)> {
        let mut seen = BTreeSet::new();
        chips
            .iter()
            .filter_map(|c| {
                let k = key(c);
                if !seen.insert(k.clone()) {
                    return None;
                }
                self.weights
                    .get(&k)
                    .filter(|w| w.abs() > f64::EPSILON)
                    .map(|w| (c.clone(), *w))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::extract_features;

    #[test]
    fn decay_smoothing_frequency_stopwords_cold_start_and_readable_weights() {
        let chips = extract_features(Some("Ada"), &[], "Rust the rare", None, None);
        let common = extract_features(Some("Ada"), &[], "Rust the", None, None);
        let corpus = vec![chips.clone(), chips.clone(), common];
        let mut feedback: Vec<_> = (0..10)
            .map(|_| Feedback {
                features: &chips,
                weight: 1.0,
                at: 0,
            })
            .collect();
        assert!(
            Profile::learn(&corpus, &feedback[..9], HALF_LIFE_MS)
                .weighted(&chips)
                .is_empty()
        );
        let weights = Profile::learn(&corpus, &feedback, HALF_LIFE_MS).weighted(&chips);
        assert_eq!(weights.len(), 2);
        assert_eq!(weights[0].0.label, "作者：Ada");
        assert!((weights[0].1 - 6_f64.ln()).abs() < 1e-10);
        let saturated: Vec<_> = (0..100)
            .map(|_| Feedback {
                features: &chips,
                weight: 1.0,
                at: 0,
            })
            .collect();
        assert_eq!(
            Profile::learn(&corpus, &saturated, HALF_LIFE_MS).weighted(&chips)[0].1,
            MAX_WEIGHT
        );
        // 混合强正、弱负与中性反馈，手算衰减后的正负计数；中性不凑冷启动门槛。
        for f in &mut feedback[..5] {
            f.weight = 2.0;
        }
        for f in &mut feedback[5..] {
            f.weight = -0.25;
        }
        let mixed = Profile::learn(&corpus, &feedback, HALF_LIFE_MS).weighted(&chips);
        assert!((mixed[0].1 - (6.0_f64 / 1.625).ln()).abs() < 1e-10);
        feedback.iter_mut().for_each(|f| f.weight = -0.25);
        let negative = Profile::learn(&corpus, &feedback, HALF_LIFE_MS).weighted(&chips);
        assert!((negative[0].1 + 2.25_f64.ln()).abs() < 1e-10);
        assert!(
            Profile::learn(&corpus, &feedback, 0)
                .weighted(&chips)
                .is_empty()
        );
        feedback[0].weight = 0.0;
        assert!(
            Profile::learn(&corpus, &feedback, HALF_LIFE_MS)
                .weighted(&chips)
                .is_empty()
        );
    }
}
