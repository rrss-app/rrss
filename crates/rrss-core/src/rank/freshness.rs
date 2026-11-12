//! 阶段 23「B1 基础层 · 时效衰减」：打开延迟中位数与指数衰减，纯函数。

use std::collections::BTreeMap;

/// 默认一天：日常 RSS 消费周期；不足 5 篇已打开文章时不推断源节奏。
pub const DEFAULT_HALF_LIFE_MS: i64 = 86_400_000;
pub const MIN_SAMPLES: usize = 5;

pub fn learn(delays: &[(i64, i64)]) -> BTreeMap<i64, i64> {
    let mut grouped = BTreeMap::<i64, Vec<i64>>::new();
    for &(feed, delay) in delays.iter().filter(|(_, d)| *d >= 0) {
        grouped.entry(feed).or_default().push(delay);
    }
    grouped
        .into_iter()
        .filter(|(_, v)| v.len() >= MIN_SAMPLES)
        .map(|(feed, mut v)| {
            v.sort_unstable();
            let middle = v.len() / 2;
            let median = if v.len() % 2 == 0 {
                v[middle - 1] + (v[middle] - v[middle - 1]) / 2
            } else {
                v[middle]
            };
            // 零延迟仍保留一小时半衰期，避免除零和瞬时清空。
            (feed, median.max(3_600_000))
        })
        .collect()
}

/// 返回已损失的新鲜度 [0,1]；缺失发布时间不施加惩罚，未来时间视作刚发布。
pub fn penalty(published: Option<i64>, now: i64, half_life: i64) -> f64 {
    published
        .map(|at| {
            1.0 - 2_f64.powf(-(now.saturating_sub(at).max(0) as f64) / half_life.max(1) as f64)
        })
        .unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_median_sparse_default_and_missing_future_dates() {
        let p = learn(&[
            (1, 1),
            (1, 2),
            (1, DEFAULT_HALF_LIFE_MS),
            (1, DEFAULT_HALF_LIFE_MS * 2),
            (1, DEFAULT_HALF_LIFE_MS * 3),
            (2, 1),
            (3, -1),
        ]);
        assert_eq!(p[&1], DEFAULT_HALF_LIFE_MS);
        assert!(!p.contains_key(&2));
        assert_eq!(penalty(Some(0), DEFAULT_HALF_LIFE_MS, p[&1]), 0.5);
        assert_eq!(penalty(None, 1, 1), 0.0);
        assert_eq!(penalty(Some(2), 1, 1), 0.0);
    }
}
