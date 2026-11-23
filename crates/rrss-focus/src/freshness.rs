//! 阶段 23「B1 基础层 · 时效衰减」：按固定半衰期的指数衰减，纯函数。

/// 返回已损失的新鲜度 [0,1]；缺失发布时间不施加惩罚，未来时间视作刚发布。
pub fn penalty(published: Option<i64>, now: i64, half_life: i64) -> f64 {
    1.0 - value(published, now, half_life)
}

/// Remaining freshness, shared with the fitted predictor. Missing/future
/// dates are fresh; underflow keeps a strictly positive finite lower bound.
pub(super) fn value(published: Option<i64>, now: i64, half_life: i64) -> f64 {
    published
        .map(|at| {
            2_f64
                .powf(-(now.saturating_sub(at).max(0) as f64) / half_life.max(1) as f64)
                .max(f64::MIN_POSITIVE)
        })
        .unwrap_or(1.0)
}

const DAY: i64 = 86_400_000;

/// How age costs relevance. `Legacy` is the fixed 3-day half-life of the manual
/// default. `Shelf` (fitted ranking) scales a shelf life by publishing rate:
/// three days at ten posts a day, about three weeks at one a week. Inside it
/// the cost is gentle early and steep near the edge; Focus admits nothing
/// older; feeds below 0.85 posts a week never age.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Policy {
    Legacy,
    Shelf { quadratic: bool },
}

/// The policy for the current fitted-ranking switch (`Tuning::freshness` may override).
pub(crate) fn policy(fitted_enabled: bool) -> Policy {
    if let Some(p) = crate::tuning::current().freshness {
        return p;
    }
    if fitted_enabled {
        Policy::Shelf { quadratic: true }
    } else {
        Policy::Legacy
    }
}

/// Below this many posts a week a feed never ages.
pub const EXEMPT_PER_WEEK: f64 = 0.85;
/// Rate lookback; the observed span is floored so a brand-new feed with one
/// or two items does not look busy.
pub const RATE_DAYS: i64 = 90;
const RATE_FLOOR_DAYS: i64 = 28;
/// Quiet feeds stay in Focus this long.
pub const EXEMPT_LOOKBACK_DAYS: i64 = 180;

/// Posts per day: `count` in the last `RATE_DAYS`, over the time since the
/// feed's earliest article, clamped to 28–90 days.
pub fn per_day(count: usize, observed_ms: i64) -> f64 {
    let span = observed_ms.clamp(RATE_FLOOR_DAYS * DAY, RATE_DAYS * DAY) as f64 / DAY as f64;
    count as f64 / span
}

pub fn exempt(per_day: f64) -> bool {
    per_day * 7.0 < EXEMPT_PER_WEEK
}

/// Shelf life for a publishing rate: 3 days × (10 / posts a day)^0.46, 1–30 days.
pub fn window_for_rate(per_day: f64) -> i64 {
    if per_day <= 0.0 {
        return 30 * DAY;
    }
    let days = 3.0 * (10.0 / per_day).powf(0.46);
    (days.clamp(1.0, 30.0) * DAY as f64) as i64
}

/// Per-feed rate from `(feed, effective date)` over the whole library.
pub(super) fn rates(
    published: impl Iterator<Item = (i64, i64)>,
    before: i64,
) -> std::collections::BTreeMap<i64, f64> {
    let mut seen = std::collections::BTreeMap::<i64, (usize, i64)>::new();
    for (feed, at) in published.filter(|(_, at)| *at < before) {
        let e = seen.entry(feed).or_insert((0, at));
        if at >= before - RATE_DAYS * DAY {
            e.0 += 1;
        }
        e.1 = e.1.min(at);
    }
    seen.into_iter()
        .map(|(feed, (n, first))| (feed, per_day(n, before - first)))
        .collect()
}

/// Age cost in shelf-life units, unbounded: 3 at the edge (the manual weight −2
/// makes that 6 points), quadratic by default so fresh hours cost little.
pub(super) fn shelf_cost(published: Option<i64>, now: i64, window: i64, quadratic: bool) -> f64 {
    published
        .map(|at| {
            let r = now.saturating_sub(at).max(0) as f64 / window.max(1) as f64;
            3.0 * if quadratic { r * r } else { r }
        })
        .unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn half_life_missing_and_future_dates() {
        const DAY: i64 = 86_400_000;
        assert_eq!(penalty(Some(0), DAY, DAY), 0.5);
        assert_eq!(penalty(Some(0), 2 * DAY, DAY), 0.75);
        assert_eq!(penalty(None, 1, 1), 0.0);
        assert_eq!(penalty(Some(2), 1, 1), 0.0);
    }

    #[test]
    fn shelf_life_matches_the_stated_anchors_and_exempts_quiet_feeds() {
        let days = |per_day: f64| window_for_rate(per_day) as f64 / DAY as f64;
        assert!((days(10.0) - 3.0).abs() < 0.01);
        assert!((days(1.0 / 7.0) - 21.2).abs() < 0.3);
        assert!(days(50.0) < 1.5);
        assert_eq!(days(1.0 / 60.0), 30.0);
        // One a week is aged, one a month or a brand-new single item is not.
        assert!(!exempt(per_day(13, 90 * DAY)));
        assert!(exempt(per_day(3, 90 * DAY)));
        assert!(exempt(per_day(1, 2 * DAY)));
        assert!(!exempt(per_day(40, DAY)));
        let w = 4 * DAY;
        assert_eq!(shelf_cost(Some(0), w, w, true), 3.0);
        assert_eq!(shelf_cost(Some(0), w / 2, w, true), 0.75);
        assert_eq!(shelf_cost(Some(0), w / 2, w, false), 1.5);
        assert_eq!(shelf_cost(None, w, w, true), 0.0);
    }
}
