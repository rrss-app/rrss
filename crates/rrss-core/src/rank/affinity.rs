//! 阶段 23「B2 增强层」：加权球面 k-means 与负反馈均值，纯内存计算。

pub const MIN_POSITIVES: usize = 10;
const MAX_ITERATIONS: usize = 20;
pub const NEGATIVE_FACTOR: f64 = 0.3;

pub struct Feedback<'a> {
    pub id: i64,
    pub vector: &'a [f32],
    /// 与 labels::Label.feedback_weight 相同，正数喜欢、负数不喜欢。
    pub weight: f64,
}

#[derive(Default)]
pub struct Profile {
    centers: Vec<Vec<f32>>,
    negative: Vec<f32>,
    representatives: Vec<Option<i64>>,
}

pub fn normalized(vector: &[f32]) -> Option<Vec<f32>> {
    normalized_owned(vector.to_vec())
}
pub(crate) fn normalized_owned(mut vector: Vec<f32>) -> Option<Vec<f32>> {
    let norm = dot(&vector, &vector).sqrt();
    if !norm.is_finite() || norm == 0.0 {
        return None;
    }
    let scale = norm.recip();
    for v in &mut vector {
        *v = (*v as f64 * scale) as f32;
    }
    Some(vector)
}
fn dot(a: &[f32], b: &[f32]) -> f64 {
    // 八条独立累加链，便于编译器向量化；保留 f64 累加精度。
    let len = a.len().min(b.len());
    let mut sums = [0.0; 8];
    let (left, left_tail) = a[..len].as_chunks::<8>();
    let (right, right_tail) = b[..len].as_chunks::<8>();
    for (left, right) in left.iter().zip(right) {
        for i in 0..8 {
            sums[i] += left[i] as f64 * right[i] as f64;
        }
    }
    sums.iter().sum::<f64>()
        + left_tail
            .iter()
            .zip(right_tail)
            .map(|(a, b)| *a as f64 * *b as f64)
            .sum::<f64>()
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
    pub fn learn(feedback: &[Feedback<'_>]) -> Self {
        let mut vectors: Vec<_> = feedback
            .iter()
            .filter_map(|f| normalized(f.vector).map(|v| (f.id, v, f.weight)))
            .collect();
        vectors.sort_by_key(|v| v.0);
        let positive: Vec<_> = vectors.iter().filter(|v| v.2 > 0.0).collect();
        if positive.len() < MIN_POSITIVES {
            return Self::default();
        }
        let dims = positive[0].1.len();
        // 纯函数也拒绝不同维度；同维度的空间隔离由调用方保证。
        if vectors.iter().any(|v| v.1.len() != dims) {
            return Self::default();
        }
        let k = (positive.len() / 5).min(8);
        let mut centers = vec![positive[0].1.clone()];
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
        Self {
            negative: mean(
                vectors
                    .iter()
                    .filter(|v| v.2 < 0.0)
                    .map(|v| (v.1.as_slice(), -v.2)),
                dims,
            ),
            centers,
            representatives,
        }
    }

    /// 输入已归一化的文章向量；返回分量与最近质心的簇内代表文章。
    /// 缺向量 / 样本不足时中性降级。
    pub fn score(&self, vector: &[f32]) -> (f64, Option<i64>) {
        if self.centers.is_empty() || vector.len() != self.centers[0].len() {
            return (0.0, None);
        }
        let (group, similarity) = nearest(vector, &self.centers);
        let score = similarity.clamp(-1.0, 1.0) - NEGATIVE_FACTOR * dot(vector, &self.negative);
        (score, self.representatives[group])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
