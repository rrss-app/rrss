//! 阶段 23「指标与读数」：加权 AUC 与 NDCG@10，纯函数。

#[derive(Debug, Clone, Copy)]
pub struct Sample {
    pub positive: bool,
    pub weight: f64,
    pub score: f64,
}

fn valid(s: &&Sample) -> bool {
    s.weight.is_finite() && s.weight > 0.0 && s.score.is_finite()
}

/// 加权正负对的胜率，分数并列算半胜；无正例或无负例时未定义。
pub fn weighted_auc(samples: &[Sample]) -> Option<f64> {
    let mut sorted: Vec<_> = samples.iter().filter(valid).collect();
    sorted.sort_by(|a, b| a.score.total_cmp(&b.score));
    let (mut positives, mut negatives, mut wins) = (0.0, 0.0, 0.0);
    let mut i = 0;
    while i < sorted.len() {
        let mut j = i;
        let (mut p, mut n) = (0.0, 0.0);
        while j < sorted.len() && sorted[j].score == sorted[i].score {
            if sorted[j].positive {
                p += sorted[j].weight;
            } else {
                n += sorted[j].weight;
            }
            j += 1;
        }
        wins += p * (negatives + n * 0.5);
        positives += p;
        negatives += n;
        i = j;
    }
    (positives > 0.0 && negatives > 0.0).then(|| wins / (positives * negatives))
}

/// 二元相关性 × IPW 为 gain，IDCG 以加权 gain 降序排；无正例时未定义。
/// 稳定排序保留并列分数的输入次序，调用方先按焦点的时间 / id 次序排列。
pub fn ndcg_at_10(samples: &[Sample]) -> Option<f64> {
    let mut sorted: Vec<_> = samples.iter().filter(valid).collect();
    sorted.sort_by(|a, b| b.score.total_cmp(&a.score));
    let gains: Vec<_> = sorted
        .iter()
        .map(|s| if s.positive { s.weight } else { 0.0 })
        .collect();
    let mut ideal = gains.clone();
    ideal.sort_by(|a, b| b.total_cmp(a));
    let dcg = |values: &[f64]| {
        values
            .iter()
            .take(10)
            .enumerate()
            .map(|(i, gain)| gain / (i as f64 + 2.0).log2())
            .sum::<f64>()
    };
    let idcg = dcg(&ideal);
    (idcg > 0.0).then(|| dcg(&gains) / idcg)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(positive: bool, weight: f64, score: f64) -> Sample {
        Sample {
            positive,
            weight,
            score,
        }
    }
    fn close(actual: Option<f64>, expected: f64) {
        assert!((actual.unwrap() - expected).abs() < 1e-10);
    }

    #[test]
    fn auc_weighted_pairs_and_ties_match_hand_calculation() {
        let samples = [
            sample(true, 2.0, 2.0),
            sample(true, 1.0, 1.0),
            sample(false, 3.0, 1.0),
            sample(false, 1.0, 3.0),
        ];
        // 2*3 胜出，1*3 半胜，总正负对权重 (2+1)*(3+1)。
        close(weighted_auc(&samples), 7.5 / 12.0);
        close(
            weighted_auc(&[sample(true, 2.0, 0.0), sample(false, 3.0, 0.0)]),
            0.5,
        );
    }

    #[test]
    fn ndcg_weighted_gain_stable_ties_and_cutoff_match_hand_calculation() {
        let samples = [
            sample(false, 1.0, 3.0),
            sample(true, 1.0, 2.0),
            sample(true, 2.0, 2.0),
        ];
        close(
            ndcg_at_10(&samples),
            (1.0 / 3_f64.log2() + 2.0 / 2.0) / (2.0 + 1.0 / 3_f64.log2()),
        );
        let mut eleven: Vec<_> = (0..10)
            .map(|i| sample(false, 1.0, 20.0 - i as f64))
            .collect();
        eleven.push(sample(true, 1.0, 0.0));
        close(ndcg_at_10(&eleven), 0.0);
    }

    #[test]
    fn empty_single_class_and_invalid_inputs_have_defined_boundaries() {
        for samples in [
            vec![],
            vec![sample(true, 1.0, 1.0)],
            vec![sample(false, 1.0, 1.0)],
        ] {
            assert_eq!(weighted_auc(&samples), None);
            assert_eq!(
                ndcg_at_10(&samples),
                samples.first().filter(|s| s.positive).map(|_| 1.0)
            );
        }
        let invalid = [
            sample(true, f64::NAN, 1.0),
            sample(false, 1.0, f64::INFINITY),
            sample(true, 0.0, 0.0),
        ];
        assert_eq!(weighted_auc(&invalid), None);
        assert_eq!(ndcg_at_10(&invalid), None);
    }
}
