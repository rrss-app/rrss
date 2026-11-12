//! 阶段 23「共同部分」「可维护性约定」：分量 × 手调权重 × 开关。

use serde::{Deserialize, Serialize};

use crate::{features::FeatureChip, rules::Dimension};
pub const ALGO_VERSION: i64 = 7;

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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reason {
    pub component: Component,
    pub text: String,
    /// 这条原因折合的整数分；不足 1 分的分量不出原因。
    #[serde(default)]
    pub points: i64,
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
    let mut add = |component, text, value: f64, weight: f64| {
        let points = (value * weight).round() as i64;
        if points != 0 {
            reasons.push(Reason {
                component,
                text,
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
            let habit = if *w > 0.0 { "常读" } else { "常跳过" };
            let text = match chip.dimension {
                Dimension::TitleKw => format!("标题含你{habit}的『{}』", chip.pattern),
                Dimension::Author => format!("你{habit}作者『{}』", chip.pattern),
                Dimension::Tag => format!("你{habit}标签『{}』", chip.pattern),
                _ => format!("你{habit}域名『{}』", chip.pattern),
            };
            add(Component::Features, text, *w, WEIGHTS[0]);
        }
    }
    if on.source && input.source.abs() > f64::EPSILON {
        add(
            Component::Source,
            if input.source > 0.0 {
                "你常读这个源"
            } else {
                "你较少打开这个源"
            }
            .into(),
            input.source,
            WEIGHTS[1],
        );
    }
    if on.coverage && input.coverage.0 > 1 {
        add(
            Component::Coverage,
            format!("{} 个源在报道", input.coverage.0),
            input.coverage.1,
            WEIGHTS[2],
        );
    }
    if on.freshness && input.freshness > 0.0 {
        add(
            Component::Freshness,
            "按这个源的阅读节奏降低旧文优先级".into(),
            input.freshness,
            WEIGHTS[3],
        );
    }
    if on.affinity
        && input.affinity.0 * AFFINITY_WEIGHT >= 1.0
        && let Some(title) = input.affinity.1
    {
        let short: String = title.chars().take(16).collect();
        add(
            Component::Affinity,
            format!("和你常读的『{short}』相似"),
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
            ("开发", 1.5),
            ("类型", 1.5),
            ("系统", 1.5),
            ("广告", -2.0),
            ("推广", -0.25),
        ]
        .into_iter()
        .map(|(word, weight)| {
            (
                FeatureChip {
                    dimension: Dimension::TitleKw,
                    pattern: word.into(),
                    label: format!("标题：{word}"),
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
            affinity: (0.8, Some("Rust async 的运行时与任务调度原理")),
        };
        let result = combine(input(), Switches::ALL);
        assert_eq!(
            format!("{result:?}"),
            "Boost { score: 8, reasons: [Reason { component: Features, text: \"标题含你常读的『Rust』\", points: 2 }, Reason { component: Features, text: \"标题含你常跳过的『广告』\", points: -2 }, Reason { component: Source, text: \"你常读这个源\", points: 1 }, Reason { component: Coverage, text: \"3 个源在报道\", points: 2 }, Reason { component: Freshness, text: \"按这个源的阅读节奏降低旧文优先级\", points: -1 }, Reason { component: Affinity, text: \"和你常读的『Rust async 的运行时与』相似\", points: 2 }] }"
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
}
