//! 阶段 23「B2 增强层」：加权球面 k-means 与负反馈均值，纯内存计算。

use sha2::{Digest, Sha256};

use crate::vector_math::{dot, normalized};

pub const MIN_POSITIVES: usize = 10;
const MAX_ITERATIONS: usize = 20;
pub const NEGATIVE_FACTOR: f64 = 0.3;
/// Short-lived reading can break close ties without matching an explicit choice.
pub const RECENT_FACTOR: f64 = 0.15;

pub(crate) fn combined_score(long: f64, recent: f64) -> f64 {
    (long + RECENT_FACTOR * recent).clamp(-1.0, 1.0)
}

pub struct Feedback<'a> {
    pub id: i64,
    pub vector: &'a [f32],
    /// 与 labels::Evidence.weight 相同，正数喜欢、负数不喜欢。
    pub weight: f64,
}

#[derive(Default)]
pub struct Profile {
    centers: Vec<Vec<f32>>,
    negative: Vec<f32>,
    representatives: Vec<Option<i64>>,
    semantic_representatives: Vec<Option<i64>>,
    partition: Vec<[u8; 32]>,
    strengths: Vec<f64>,
    negative_strength: f64,
    negative_profile: Option<Box<Profile>>,
}

fn nearest(vector: &[f32], centers: &[Vec<f32>]) -> (usize, f64) {
    centers
        .iter()
        .enumerate()
        .map(|(i, c)| (i, dot(vector, c)))
        .max_by(|a, b| a.1.total_cmp(&b.1).then_with(|| b.0.cmp(&a.0)))
        .unwrap()
}
fn mean<'a>(vectors: impl Iterator<Item = (&'a [f32], f64)>, dims: usize) -> Vec<f32> {
    let mut sum = vec![0.0_f64; dims];
    for (v, w) in vectors {
        for (s, x) in sum.iter_mut().zip(v) {
            *s += *x as f64 * w;
        }
    }
    normalized(&sum.into_iter().map(|x| x as f32).collect::<Vec<_>>()).unwrap_or(vec![0.0; dims])
}

impl Profile {
    pub(crate) fn signature(&self, space_id: Option<i64>) -> String {
        let mut hash = Sha256::new();
        hash.update(b"rrss-affinity-partition-v1");
        // A vector-space row is a compatibility generation: even a rebuild
        // with identical dimensions/model names receives a new ID.
        hash.update(space_id.unwrap_or(0).to_le_bytes());
        hash.update((self.partition.len() as u64).to_le_bytes());
        for group in &self.partition {
            hash.update(group);
        }
        format!("{:x}", hash.finalize())
    }

    pub(crate) fn semantic_representative(&self, representative: i64) -> Option<i64> {
        self.representatives
            .iter()
            .position(|id| *id == Some(representative))
            .and_then(|i| self.semantic_representatives[i])
    }

    pub fn learn(feedback: &[Feedback<'_>]) -> Self {
        Self::learn_inner(feedback, None)
    }

    pub(crate) fn learn_temporal(
        feedback: &[Feedback<'_>],
        weights: &[super::evidence::Weighted],
        recent: bool,
    ) -> Self {
        let positive: Vec<_> = feedback
            .iter()
            .map(|f| Feedback {
                id: f.id,
                vector: f.vector,
                weight: f.weight.max(0.0),
            })
            .collect();
        let negative: Vec<_> = feedback
            .iter()
            .map(|f| Feedback {
                id: f.id,
                vector: f.vector,
                weight: (-f.weight).max(0.0),
            })
            .collect();
        let mut profile = Self::learn_inner(&positive, Some((weights, recent)));
        profile.negative_profile = Some(Box::new(Self::learn_inner(
            &negative,
            Some((weights, recent)),
        )));
        profile
    }

    fn learn_inner(
        feedback: &[Feedback<'_>],
        temporal: Option<(&[super::evidence::Weighted], bool)>,
    ) -> Self {
        let mut vectors: Vec<_> = feedback
            .iter()
            .enumerate()
            .filter(|(_, f)| f.weight.is_finite() && f.weight != 0.0)
            .filter_map(|(i, f)| normalized(f.vector).map(|v| (f.id, v, f.weight, i)))
            .collect();
        vectors.sort_by_key(|v| v.0);
        let positive: Vec<_> = vectors.iter().filter(|v| v.2 > 0.0).collect();
        if vectors.is_empty() || (temporal.is_none() && positive.len() < MIN_POSITIVES) {
            return Self::default();
        }
        let dims = vectors[0].1.len();
        // 纯函数也拒绝不同维度；同维度的空间隔离由调用方保证。
        if vectors.iter().any(|v| v.1.len() != dims) {
            return Self::default();
        }
        let k = if temporal.is_some() {
            positive.len().min(8)
        } else {
            (positive.len() / 5).clamp(1, 8).min(positive.len())
        };
        let mut centers = positive
            .first()
            .map(|p| vec![p.1.clone()])
            .unwrap_or_default();
        // 输入按文章 id 排序；最远点初始化、平票取较早输入，无随机状态。
        while centers.len() < k {
            let next = positive
                .iter()
                .min_by(|a, b| {
                    nearest(&a.1, &centers)
                        .1
                        .total_cmp(&nearest(&b.1, &centers).1)
                })
                .unwrap();
            // Small temporal profiles must also separate unrelated topics;
            // otherwise an isolated burst could borrow another topic's dates.
            if temporal.is_some() && nearest(&next.1, &centers).1 >= 0.85 {
                break;
            }
            centers.push(next.1.clone());
        }
        let mut assignments = vec![usize::MAX; positive.len()];
        for _ in 0..MAX_ITERATIONS {
            let next: Vec<_> = positive.iter().map(|p| nearest(&p.1, &centers).0).collect();
            if next == assignments {
                break;
            }
            assignments = next;
            for (i, center) in centers.iter_mut().enumerate() {
                if assignments.contains(&i) {
                    *center = mean(
                        positive
                            .iter()
                            .zip(&assignments)
                            .filter(|(_, group)| **group == i)
                            .map(|(p, _)| (p.1.as_slice(), p.2)),
                        dims,
                    );
                }
            }
        }
        // 每簇只在学习结束时选一次代表；平票取文章 id 较小的一篇。
        let representatives = centers
            .iter()
            .enumerate()
            .map(|(i, center)| {
                positive
                    .iter()
                    .zip(&assignments)
                    .filter(|(_, group)| **group == i)
                    .map(|(p, _)| (p.0, dot(&p.1, center)))
                    .max_by(|a, b| a.1.total_cmp(&b.1).then_with(|| b.0.cmp(&a.0)))
                    .map(|(id, _)| id)
            })
            .collect();
        // Topic identity follows the actual partition, independently of the
        // temporally weighted center and its nearest display representative.
        // Include each member's input vector so re-embedding within a space
        // cannot silently merge incompatible cached topics. Repeated evidence
        // for one article affects scores, but is still one partition member.
        let mut partition = Vec::new();
        let semantic_representatives = (0..centers.len())
            .map(|i| {
                let mut members: Vec<_> = positive
                    .iter()
                    .zip(&assignments)
                    .filter(|(_, group)| **group == i)
                    .map(|(p, _)| *p)
                    .collect();
                members.dedup_by_key(|p| p.0);
                let representative = members.first()?.0;
                let mut hash = Sha256::new();
                hash.update((members.len() as u64).to_le_bytes());
                for member in members {
                    hash.update(member.0.to_le_bytes());
                    hash.update((member.1.len() as u64).to_le_bytes());
                    for value in &member.1 {
                        hash.update(value.to_le_bytes());
                    }
                }
                partition.push(hash.finalize().into());
                Some(representative)
            })
            .collect();
        partition.sort_unstable();
        // Confidence is applied after normalization: a tiny one-day cluster must
        // not regain a full-strength cosine by being normalized to unit length.
        let confidence = |members: Vec<usize>| {
            let Some((weights, recent)) = temporal else {
                return 1.0;
            };
            let mut days = std::collections::BTreeSet::new();
            let (mut explicit, mut implicit) = (0.0, 0.0);
            for i in members {
                let f = weights[i];
                if f.explicit {
                    explicit += f.long.abs();
                } else {
                    days.insert(f.day);
                    implicit += if recent { f.recent.abs() } else { f.long.abs() };
                }
            }
            let support = explicit
                + implicit
                    * if recent {
                        1.0
                    } else {
                        super::evidence::persistence(days.len())
                    };
            (support / 3.0).min(1.0)
        };
        let strengths = (0..centers.len())
            .map(|i| {
                confidence(
                    positive
                        .iter()
                        .zip(&assignments)
                        .filter(|(_, g)| **g == i)
                        .map(|(p, _)| p.3)
                        .collect(),
                )
            })
            .collect();
        let negative_strength =
            confidence(vectors.iter().filter(|v| v.2 < 0.0).map(|v| v.3).collect());
        Self {
            strengths,
            negative_strength,
            negative_profile: None,
            negative: mean(
                vectors
                    .iter()
                    .filter(|v| v.2 < 0.0)
                    .map(|v| (v.1.as_slice(), -v.2)),
                dims,
            ),
            centers,
            representatives,
            semantic_representatives,
            partition,
        }
    }

    /// 输入已归一化的文章向量；返回分量与最近质心的簇内代表文章。
    /// 缺向量 / 样本不足时中性降级。
    pub fn score(&self, vector: &[f32]) -> (f64, Option<i64>) {
        let penalty = if let Some(profile) = &self.negative_profile {
            NEGATIVE_FACTOR * profile.score(vector).0
        } else if self.negative.len() == vector.len() {
            NEGATIVE_FACTOR * self.negative_strength * dot(vector, &self.negative).max(0.0)
        } else {
            0.0
        };
        if self.centers.is_empty() || self.centers[0].len() != vector.len() {
            return (-penalty, None);
        }
        let positive = self
            .centers
            .iter()
            .enumerate()
            .map(|(i, c)| {
                (
                    dot(vector, c).clamp(0.0, 1.0) * self.strengths[i],
                    self.representatives[i],
                )
            })
            .max_by(|a, b| a.0.total_cmp(&b.0))
            .unwrap_or_default();
        (positive.0 - penalty, positive.1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temporal(vectors: &[[f32; 2]], weights: &[f64]) -> Profile {
        let weighted: Vec<_> = weights
            .iter()
            .enumerate()
            .map(|(i, &long)| super::super::evidence::Weighted {
                article_id: i as i64 + 1,
                day: i as i64,
                explicit: true,
                long,
                recent: 0.0,
            })
            .collect();
        let feedback: Vec<_> = weighted
            .iter()
            .zip(vectors)
            .map(|(w, vector)| Feedback {
                id: w.article_id,
                vector,
                weight: w.long,
            })
            .collect();
        Profile::learn_temporal(&feedback, &weighted, false)
    }

    #[test]
    fn semantic_identity_ignores_decay_and_display_representative_changes() {
        let vectors = [[1.0, 0.0], [1.0, 0.2]];
        let profile = temporal(&vectors, &[0.2, 0.4]);
        let decayed = temporal(&vectors, &[0.1, 0.2]);
        let shifted = temporal(&vectors, &[0.4, 0.2]);
        assert_eq!(profile.signature(Some(1)), decayed.signature(Some(1)));
        assert!((profile.score(&vectors[0]).0 / 2.0 - decayed.score(&vectors[0]).0).abs() < 1e-7);
        assert_ne!(profile.representatives, shifted.representatives);
        assert_eq!(profile.signature(Some(1)), shifted.signature(Some(1)));
        assert_eq!(profile.semantic_representative(2), Some(1));
        assert_eq!(shifted.semantic_representative(1), Some(1));
        assert_ne!(profile.signature(Some(1)), profile.signature(Some(2)));
        let reembedded = temporal(&[[1.0, 0.0], [1.0, 0.3]], &[0.2, 0.4]);
        assert_eq!(
            profile.semantic_representatives,
            reembedded.semantic_representatives
        );
        assert_ne!(profile.signature(Some(1)), reembedded.signature(Some(1)));
    }

    #[test]
    fn semantic_identity_distinguishes_partitions_of_identical_input_vectors() {
        let vectors: Vec<_> = [0.0_f32, 0.4, 0.6, 1.0]
            .map(|angle| [angle.cos(), angle.sin()])
            .into();
        let balanced = temporal(&vectors, &[1.0, 1.0, 1.0, 1.0]);
        let shifted = temporal(&vectors, &[1.0, 10.0, 1.0, 10.0]);
        assert_eq!(balanced.centers.len(), 2);
        assert_eq!(shifted.centers.len(), 2);
        assert_eq!(balanced.semantic_representatives, vec![Some(1), Some(3)]);
        assert_eq!(shifted.semantic_representatives, vec![Some(1), Some(4)]);
        assert_ne!(
            balanced.signature(Some(1)),
            shifted.signature(Some(1)),
            "hashing all article IDs/vectors without their groups would alias these profiles"
        );
        let weights: Vec<_> = (1..=4)
            .rev()
            .map(|id| super::super::evidence::Weighted {
                article_id: id,
                day: id,
                explicit: true,
                long: 1.0,
                recent: 0.0,
            })
            .collect();
        let feedback: Vec<_> = weights
            .iter()
            .map(|w| Feedback {
                id: w.article_id,
                vector: &vectors[w.article_id as usize - 1],
                weight: w.long,
            })
            .collect();
        assert_eq!(
            balanced.signature(Some(1)),
            Profile::learn_temporal(&feedback, &weights, false).signature(Some(1))
        );
    }

    #[test]
    fn each_positive_and_negative_cluster_requires_its_own_persistent_support() {
        let x = [1.0, 0.0];
        let y = [0.0, 1.0];
        for sign in [1.0, -1.0] {
            let weights: Vec<_> = (0..10)
                .map(|id| super::super::evidence::Weighted {
                    article_id: id,
                    day: if id < 5 { id } else { 5 },
                    explicit: false,
                    long: sign * if id < 5 { 1.0 } else { 0.4 },
                    recent: 0.0,
                })
                .collect();
            let feedback: Vec<_> = weights
                .iter()
                .map(|v| Feedback {
                    id: v.article_id,
                    vector: if v.article_id < 5 { &x } else { &y },
                    weight: v.long,
                })
                .collect();
            let profile = Profile::learn_temporal(&feedback, &weights, false);
            assert_eq!(profile.score(&y).0, 0.0);
            assert!(profile.score(&x).0.abs() > 0.2);
        }
    }

    #[test]
    fn weighted_centers_max_cosine_negative_penalty_and_small_sample_fallback() {
        let x = [1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let y = [0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let diagonal = normalized(&[1.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]).unwrap();
        let mut feedback: Vec<_> = (0..10)
            .map(|id| Feedback {
                id,
                vector: if id < 5 { &x } else { &y },
                weight: if id == 0 { 2.0 } else { 1.0 },
            })
            .collect();
        feedback.push(Feedback {
            id: 10,
            vector: &x,
            weight: -0.25,
        });
        feedback.push(Feedback {
            id: 11,
            vector: &y,
            weight: -1.0,
        });
        let model = Profile::learn(&feedback);
        assert_eq!(model.centers, vec![x.to_vec(), y.to_vec()]);
        assert_eq!(model.centers, Profile::learn(&feedback).centers);
        let (score, id) = model.score(&x);
        assert_eq!(id, Some(0));
        assert!((score - (1.0 - 0.3 / 17_f64.sqrt())).abs() < 1e-6);
        assert!(
            (model.score(&diagonal).0 - (1.0 / 2_f64.sqrt() - 0.3 * 5.0 / 34_f64.sqrt())).abs()
                < 1e-6
        );
        assert_eq!(Profile::learn(&feedback[..9]).score(&x), (0.0, None));
        // 正反馈权重也进入质心；平票的对角向量归入第一个 x 组。
        feedback[4].vector = &diagonal;
        let weighted = Profile::learn(&feedback);
        let center = weighted.centers.iter().find(|c| c[0] > 0.0).unwrap();
        let expected = normalized(&[(5.0 + 1.0 / 2_f32.sqrt()), 1.0 / 2_f32.sqrt()]).unwrap();
        assert!((center[0] - expected[0]).abs() < 1e-6);
        assert!((center[1] - expected[1]).abs() < 1e-6);
        // 对角文章自身最相近，但其最近质心的代表是更贴近质心的 x 文章。
        assert_eq!(weighted.score(&diagonal).1, Some(0));
        assert_eq!(model.score(&[]), (0.0, None));
    }
}
