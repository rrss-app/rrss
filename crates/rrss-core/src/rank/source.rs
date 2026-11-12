//! 阶段 23「B1 基础层 · 源偏好」：近 60 天曝光打开率，纯函数。

use super::labels::{self, LabelExposure};
use crate::signals::{ArticleEvent, EventKind};
use std::collections::{BTreeMap, BTreeSet};
pub const WINDOW_MS: i64 = 60 * 86_400_000;

/// 20 次曝光的先验强度，稀疏源先接近全局均值，避免一次点击主导。
pub const PRIOR_EXPOSURES: f64 = 20.0;

pub fn learn(
    exposures: &[(LabelExposure, i64)],
    events: &[ArticleEvent],
    now: i64,
) -> BTreeMap<i64, f64> {
    let window = exposures
        .iter()
        .filter(|(e, _)| e.shown_at < now && e.shown_at >= now.saturating_sub(WINDOW_MS));
    let by_article = labels::index_exposures(window.clone().map(|(e, _)| e));
    let mut opened = BTreeSet::new();
    for event in events
        .iter()
        .filter(|e| e.kind == EventKind::Open && e.at < now)
    {
        if let Some(e) =
            labels::exposure_for(&by_article, event.article_id, event.at, event.exposure_id)
        {
            opened.insert(e.id);
        }
    }
    let mut counts = BTreeMap::<i64, (f64, f64)>::new();
    for (e, feed) in window {
        let c = counts.entry(*feed).or_default();
        c.0 += 1.0;
        c.1 += f64::from(opened.contains(&e.id));
    }
    let (n, o) = counts
        .values()
        .fold((0.0, 0.0), |(n, o), c| (n + c.0, o + c.1));
    if n == 0.0 {
        return BTreeMap::new();
    }
    // Jeffreys 半计数使全零 / 全开历史仍可取对数。
    let mean = (o + 0.5) / (n + 1.0);
    counts
        .into_iter()
        .map(|(feed, (n, o))| {
            (
                feed,
                (((o + PRIOR_EXPOSURES * mean) / (n + PRIOR_EXPOSURES)) / mean).ln(),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn beta_prior_window_and_duplicate_or_unlinked_opens() {
        let exposures: Vec<_> = (1..=40)
            .map(|id| {
                (
                    LabelExposure {
                        id,
                        article_id: id,
                        position: 0,
                        shown_at: 1,
                    },
                    if id <= 20 { 1 } else { 2 },
                )
            })
            .collect();
        let events: Vec<_> = (1..=20)
            .flat_map(|id| {
                vec![
                    ArticleEvent {
                        article_id: id,
                        kind: EventKind::Open,
                        exposure_id: None,
                        at: 2,
                    };
                    2
                ]
            })
            .collect();
        let p = learn(&exposures, &events, 3);
        assert!(p[&1] > 0.0 && p[&2] < 0.0);
        let mean: f64 = 20.5 / 41.0;
        assert!((p[&1] - ((20.0 + 20.0 * mean) / (40.0 * mean)).ln()).abs() < 1e-10);
        assert!(learn(&exposures, &events, WINDOW_MS + 2).is_empty());
        let events: Vec<_> = events
            .into_iter()
            .map(|mut e| {
                e.exposure_id = Some(40);
                e
            })
            .collect();
        assert_eq!(learn(&exposures, &events, 3), p); // 错误文章 id 退回本篇曝光。
        assert!(learn(&exposures, &events, 1).is_empty()); // 不关联未来曝光。
    }
}
