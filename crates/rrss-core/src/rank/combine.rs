//! 阶段 23「共同部分」「可维护性约定」：分量 × 手调权重 × 开关。

use serde::{Deserialize, Serialize};

use super::reasons::{Habit, ReasonMessage, ReasonPrefix, safe_text};
use crate::{features::FeatureChip, rules::Dimension};
pub const ALGO_VERSION: i64 = 8;

/// 文章级特征总和最多 ±4：长中文标题的二元组不能叠加压过一条 ±10 的明确规则。
pub const MAX_FEATURE_SCORE: f64 = 4.0;

/// 规则默认每条 ±10，兴趣默认每项 +2、可累加（rules/interests::DEFAULT_WEIGHT）。
/// 特征每个对数单位 1 分；源 2 分；三源约 +2；一天时效约 -1。
/// 使学习信号可打破同规则分的并列，同时一般不压过一条明确规则。
/// 向量相似满分约 +2，与源偏好、多源报道同量级，不压过 ±10 规则。
pub const AFFINITY_WEIGHT: f64 = 2.0;
pub const WEIGHTS: [f64; 5] = [1.0, 2.0, 2.0, -2.0, AFFINITY_WEIGHT];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Switches {
    pub features: bool,
    pub source: bool,
    pub coverage: bool,
    pub freshness: bool,
    pub affinity: bool,
}

impl Switches {
    pub const ALL: Self = Self {
        features: true,
        source: true,
        coverage: true,
        freshness: true,
        affinity: true,
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Component {
    Features,
    Source,
    Coverage,
    Freshness,
    Affinity,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Reason {
    pub component: Component,
    #[serde(flatten)]
    pub message: ReasonMessage,
    /// 这条原因折合的整数分；不足 1 分的分量不出原因。
    #[serde(default)]
    pub points: i64,
}

impl<'de> Deserialize<'de> for Reason {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Stored {
            component: Component,
            #[serde(default)]
            points: i64,
            #[serde(flatten)]
            message: serde_json::Value,
        }
        let stored = Stored::deserialize(deserializer)?;
        Ok(Self {
            component: stored.component,
            message: ReasonMessage::from_stored(&stored.message, "", None),
            points: stored.points,
        })
    }
}

#[derive(Debug, PartialEq)]
pub struct Boost {
    pub score: i64,
    pub reasons: Vec<Reason>,
}

pub struct Input<'a> {
    pub features: &'a [(FeatureChip, f64)],
    pub source: f64,
    pub coverage: (usize, f64),
    pub freshness: f64,
    pub affinity: (f64, Option<&'a str>),
}

/// 学来的加分永远不参与隐藏判定；调用方先只用规则 score 过滤，再加此整数排序。
/// 各项先相乘相加，最后统一四舍五入，避免小分量逐项取整丢失。
pub fn combine(input: Input<'_>, on: Switches) -> Boost {
    let enabled = [
        on.features,
        on.source,
        on.coverage,
        on.freshness,
        on.affinity,
    ];
    let values = [
        input
            .features
            .iter()
            .map(|(_, w)| w)
            .sum::<f64>()
            .clamp(-MAX_FEATURE_SCORE, MAX_FEATURE_SCORE),
        input.source,
        input.coverage.1,
        input.freshness,
        input.affinity.0,
    ];
    let score = values
        .into_iter()
        .zip(WEIGHTS)
        .zip(enabled)
        .filter(|(_, on)| *on)
        .map(|((v, w), _)| v * w)
        .sum::<f64>()
        .round() as i64;
    let mut reasons = Vec::new();
    let mut add = |component, message, value: f64, weight: f64| {
        let points = (value * weight).round() as i64;
        if points != 0 {
            reasons.push(Reason {
                component,
                message,
                points,
            });
        }
    };
    if on.features {
        // 正负各保留绝对权重最大的一条，标签数量不随标题长度增长。
        let strongest = |positive| {
            input
                .features
                .iter()
                .filter(|(_, w)| {
                    if positive {
                        *w > f64::EPSILON
                    } else {
                        *w < -f64::EPSILON
                    }
                })
                .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
        };
        for (chip, w) in [strongest(true), strongest(false)].into_iter().flatten() {
            let habit = if *w > 0.0 { Habit::Read } else { Habit::Skip };
            let pattern = safe_text(&chip.pattern);
            let message = if pattern != chip.pattern {
                // Preserve displayed content when a safe host projection would
                // change it; legacy content never enters diagnostics.
                ReasonMessage::LegacyText {
                    text: chip.pattern.clone(),
                    prefix: Some(ReasonPrefix::for_feature(chip.dimension, habit)),
                }
            } else {
                match chip.dimension {
                    Dimension::TitleKw => ReasonMessage::RankTitleKw { habit, pattern },
                    Dimension::Author => ReasonMessage::RankAuthor { habit, pattern },
                    Dimension::Tag => ReasonMessage::RankTag { habit, pattern },
                    _ => ReasonMessage::RankDomain { habit, pattern },
                }
            };
            add(Component::Features, message, *w, WEIGHTS[0]);
        }
    }
    if on.source && input.source.abs() > f64::EPSILON {
        add(
            Component::Source,
            ReasonMessage::RankSource {
                habit: if input.source > 0.0 {
                    Habit::Read
                } else {
                    Habit::Skip
                },
            },
            input.source,
            WEIGHTS[1],
        );
    }
    if on.coverage && input.coverage.0 > 1 {
        add(
            Component::Coverage,
            ReasonMessage::RankCoverage {
                count: input.coverage.0 as u64,
            },
            input.coverage.1,
            WEIGHTS[2],
        );
    }
    if on.freshness && input.freshness > 0.0 {
        add(
            Component::Freshness,
            ReasonMessage::RankFreshness {},
            input.freshness,
            WEIGHTS[3],
        );
    }
    if on.affinity
        && input.affinity.0 * AFFINITY_WEIGHT >= 1.0
        && let Some(title) = input.affinity.1
    {
        let short: String = title.chars().take(16).collect();
        let pattern = safe_text(&short);
        add(
            Component::Affinity,
            if pattern == short {
                ReasonMessage::RankAffinity { pattern }
            } else {
                ReasonMessage::LegacyText {
                    text: short,
                    prefix: Some(ReasonPrefix::RankAffinity),
                }
            },
            input.affinity.0,
            AFFINITY_WEIGHT,
        );
    }
    Boost { score, reasons }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_input_snapshot_and_independent_switches() {
        let features: Vec<_> = [
            ("Rust", 2.0),
            ("development", 1.5),
            ("types", 1.5),
            ("systems", 1.5),
            ("spam", -2.0),
            ("ads", -0.25),
        ]
        .into_iter()
        .map(|(word, weight)| {
            (
                FeatureChip {
                    dimension: Dimension::TitleKw,
                    pattern: word.into(),
                    label: word.into(),
                },
                weight,
            )
        })
        .collect();
        let input = || Input {
            features: &features,
            source: 0.5,
            coverage: (3, 3_f64.ln()),
            freshness: 0.5,
            affinity: (0.8, Some("Rust async runtime and task scheduling")),
        };
        let result = combine(input(), Switches::ALL);
        assert_eq!(result.score, 8);
        assert_eq!(
            serde_json::to_value(&result.reasons).unwrap(),
            serde_json::json!([
                {"component":"features","code":"rank_title_kw","params":{"habit":"read","pattern":"Rust"},"points":2},
                {"component":"features","code":"rank_title_kw","params":{"habit":"skip","pattern":"spam"},"points":-2},
                {"component":"source","code":"rank_source","params":{"habit":"read"},"points":1},
                {"component":"coverage","code":"rank_coverage","params":{"count":3},"points":2},
                {"component":"freshness","code":"rank_freshness","params":{},"points":-1},
                {"component":"affinity","code":"rank_affinity","params":{"pattern":"Rust async runti"},"points":2},
            ])
        );
        // 平滑后的源偏好几乎总不为 0；不足 1 分时不出原因。
        let faint = combine(
            Input {
                source: 0.1,
                ..input()
            },
            Switches::ALL,
        );
        assert!(
            faint
                .reasons
                .iter()
                .all(|r| r.component != Component::Source)
        );
        let negative: Vec<_> = features
            .iter()
            .map(|(chip, w)| (chip.clone(), -*w))
            .collect();
        assert_eq!(
            combine(
                Input {
                    features: &negative,
                    source: 0.0,
                    coverage: (0, 0.0),
                    freshness: 0.0,
                    affinity: (0.0, None),
                },
                Switches::ALL
            )
            .score,
            -4
        );
        for i in 0..5 {
            let mut on = Switches {
                features: false,
                source: false,
                coverage: false,
                freshness: false,
                affinity: false,
            };
            match i {
                0 => on.features = true,
                1 => on.source = true,
                2 => on.coverage = true,
                3 => on.freshness = true,
                _ => on.affinity = true,
            }
            let result = combine(input(), on);
            assert_eq!(result.reasons.len(), if i == 0 { 2 } else { 1 });
            assert_eq!(result.score, [4, 1, 2, -1, 2][i]);
        }
    }

    #[test]
    fn feature_dimensions_and_skipped_sources_keep_typed_habits() {
        for (dimension, code) in [
            (Dimension::Author, "rank_author"),
            (Dimension::Tag, "rank_tag"),
            (Dimension::UrlPhrase, "rank_domain"),
        ] {
            for (weight, habit) in [(2.0, "read"), (-2.0, "skip")] {
                let features = [(
                    FeatureChip {
                        dimension,
                        pattern: "Example".into(),
                        label: String::new(),
                    },
                    weight,
                )];
                let boost = combine(
                    Input {
                        features: &features,
                        source: -0.5,
                        coverage: (0, 0.0),
                        freshness: 0.0,
                        affinity: (0.0, None),
                    },
                    Switches::ALL,
                );
                let wire = serde_json::to_value(boost.reasons).unwrap();
                assert_eq!(wire[0]["code"], code);
                assert_eq!(
                    wire[0]["params"],
                    serde_json::json!({"pattern":"Example","habit":habit})
                );
                assert_eq!(wire[1]["code"], "rank_source");
                assert_eq!(wire[1]["params"], serde_json::json!({"habit":"skip"}));
            }
        }
    }

    #[test]
    fn url_feature_content_uses_legacy_prefixes_instead_of_changing_the_display() {
        let pattern = "https://author.example/notes?q=topic";
        for (dimension, prefixes) in [
            (
                Dimension::TitleKw,
                ["rank_title_kw_read", "rank_title_kw_skip"],
            ),
            (Dimension::Author, ["rank_author_read", "rank_author_skip"]),
            (Dimension::Tag, ["rank_tag_read", "rank_tag_skip"]),
            (
                Dimension::UrlPhrase,
                ["rank_domain_read", "rank_domain_skip"],
            ),
        ] {
            for (i, weight) in [2.0, -2.0].into_iter().enumerate() {
                let features = [(
                    FeatureChip {
                        dimension,
                        pattern: pattern.into(),
                        label: String::new(),
                    },
                    weight,
                )];
                let boost = combine(
                    Input {
                        features: &features,
                        source: 0.0,
                        coverage: (0, 0.0),
                        freshness: 0.0,
                        affinity: (0.0, None),
                    },
                    Switches::ALL,
                );
                let wire = serde_json::to_value(boost.reasons).unwrap();
                assert_eq!(wire[0]["code"], "legacy_text");
                assert_eq!(
                    wire[0]["params"],
                    serde_json::json!({"text":pattern,"prefix":prefixes[i]})
                );
            }
        }
        let boost = combine(
            Input {
                features: &[],
                source: 0.0,
                coverage: (0, 0.0),
                freshness: 0.0,
                affinity: (0.8, Some("https://reading.example/notes")),
            },
            Switches::ALL,
        );
        let wire = serde_json::to_value(boost.reasons).unwrap();
        assert_eq!(wire[0]["code"], "legacy_text");
        assert_eq!(
            wire[0]["params"],
            serde_json::json!({"text":"https://reading.","prefix":"rank_affinity"})
        );
    }
}
