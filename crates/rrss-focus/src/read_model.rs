//! Weighted logistic read model. No database or environment access.
use std::collections::{BTreeMap, BTreeSet};

const MAX_ITERATIONS: usize = 50;
const STEP_TOLERANCE: f64 = 1e-7;
const WEAK_PENALTY: f64 = 0.01;
/// L2 penalty on per-period (weekly) baseline offsets that absorb cross-week CTR drift.
const PERIOD_PENALTY: f64 = 1.0;
/// Position dummies after the first bucket.
pub const POSITIONS: usize = 3;

#[derive(Clone, Debug)]
pub struct Sample<const K: usize> {
    pub feed: i64,
    pub features: [f64; K],
    /// Bucket of the first showing; 0 (or None) is the reference: 0 positions
    /// 0–2, 1 3–9, 2 10–29, 3 30 and later.
    pub position: Option<u8>,
    /// Optional time bucket (e.g. week index) for training-only baseline offsets
    /// that de-confound early cold-start CTR drift from feature slopes.
    pub period: Option<i64>,
    pub label: f64,
    pub weight: f64,
}

#[derive(Clone)]
pub struct Penalty<const K: usize> {
    /// Absolute L2 curvature: loss adds beta / 2 * sum((gamma - prior)^2).
    pub beta: f64,
    pub source: f64,
    /// Prior slopes in original feature units.
    pub prior: [f64; K],
    /// Per-source prior centers (log odds); absent sources center on 0. Sources
    /// without samples keep their center as their effect.
    pub source_prior: BTreeMap<i64, f64>,
}
impl<const K: usize> Default for Penalty<K> {
    fn default() -> Self {
        Self {
            beta: 1.0,
            source: 1.0,
            prior: [0.0; K],
            source_prior: BTreeMap::new(),
        }
    }
}

#[derive(Debug)]
pub struct Report<const K: usize> {
    pub samples: usize,
    pub positives: usize,
    /// Slopes in original feature units. Standard deviations are diagnostics only.
    pub beta: [f64; K],
    pub prior: [f64; K],
    pub stddev: [f64; K],
    /// Min, 25th percentile, median, 75th percentile, max (nearest rank).
    pub source_quantiles: [f64; 5],
    pub iterations: usize,
}

pub struct Fit<const K: usize> {
    pub mean: [f64; K],
    pub intercept: f64,
    pub position: [f64; POSITIONS],
    pub sources: BTreeMap<i64, f64>,
    /// Standard deviation of each source effect given the other coefficients
    /// (1 / sqrt of its Hessian diagonal at the optimum): the spread relative
    /// to other sources, without the common shift absorbed by the intercept.
    /// Prior-only sources have the prior's.
    pub source_sd: BTreeMap<i64, f64>,
    pub report: Report<K>,
}

impl<const K: usize> Fit<K> {
    pub fn contributions(&self, feed: i64, features: [f64; K]) -> ([f64; K], f64) {
        (
            std::array::from_fn(|j| self.report.beta[j] * (features[j] - self.mean[j])),
            self.sources.get(&feed).copied().unwrap_or(0.0),
        )
    }
}

/// Simultaneous Newton updates with dense Cholesky and backtracking. Each row has
/// only K+3 nonzero entries, so Hessian accumulation does not scale as feeds².
/// Centering preserves raw slopes; means and cross entropy use opportunity weights.
pub fn fit<const K: usize>(samples: &[Sample<K>], penalty: Penalty<K>) -> Option<Fit<K>> {
    if samples.is_empty()
        || !penalty.beta.is_finite()
        || penalty.beta <= 0.0
        || !penalty.source.is_finite()
        || penalty.source <= 0.0
        || penalty.prior.iter().any(|v| !v.is_finite())
        || samples.iter().any(|s| {
            !s.weight.is_finite()
                || s.weight <= 0.0
                || !(0.0..=1.0).contains(&s.label)
                || s.features.iter().any(|x| !x.is_finite())
                || s.position.is_some_and(|p| p as usize > POSITIONS)
        })
    {
        return None;
    }
    let total: f64 = samples.iter().map(|s| s.weight).sum();
    let mean = std::array::from_fn(|j| {
        samples
            .iter()
            .map(|s| s.weight * s.features[j])
            .sum::<f64>()
            / total
    });
    let feeds: BTreeMap<_, _> = samples
        .iter()
        .map(|s| s.feed)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .enumerate()
        .map(|(i, f)| (f, K + 1 + POSITIONS + i))
        .collect();
    let period_start = K + 1 + POSITIONS + feeds.len();
    let distinct_periods: Vec<i64> = samples
        .iter()
        .filter_map(|s| s.period)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    // Latest period is the reference (0); earlier periods get regularized offsets
    // so cross-week CTR drift (cold-start Simpson's paradox) does not invert slopes.
    let periods: BTreeMap<i64, usize> = if distinct_periods.len() > 1 {
        distinct_periods[..distinct_periods.len() - 1]
            .iter()
            .enumerate()
            .map(|(i, &p)| (p, period_start + i))
            .collect()
    } else {
        BTreeMap::new()
    };
    let n = period_start + periods.len();
    let rows: Vec<Vec<(usize, f64)>> = samples
        .iter()
        .map(|s| {
            let mut x = vec![(0, 1.0)];
            x.extend((0..K).map(|j| (j + 1, s.features[j] - mean[j])));
            if let Some(p) = s.position.filter(|p| *p > 0) {
                x.push((K + p as usize, 1.0));
            }
            x.push((feeds[&s.feed], 1.0));
            if let Some(col) = s.period.and_then(|p| periods.get(&p).copied()) {
                x.push((col, 1.0));
            }
            x
        })
        .collect();
    let mut centers = vec![0.0; n];
    let mut lambdas = vec![WEAK_PENALTY; n];
    for j in 0..K {
        centers[1 + j] = penalty.prior[j];
        lambdas[1 + j] = penalty.beta;
    }
    lambdas[K + 1 + POSITIONS..period_start].fill(penalty.source);
    lambdas[period_start..].fill(PERIOD_PENALTY);
    for (feed, &j) in &feeds {
        centers[j] = penalty.source_prior.get(feed).copied().unwrap_or(0.0);
    }
    let loss = |b: &[f64]| {
        let regularizer: f64 = (0..n)
            .map(|j| 0.5 * lambdas[j] * (b[j] - centers[j]).powi(2))
            .sum();
        regularizer
            + rows
                .iter()
                .zip(samples)
                .map(|(x, s)| {
                    let z: f64 = x.iter().map(|&(j, v)| b[j] * v).sum();
                    s.weight * (z.max(0.0) + (-z.abs()).exp().ln_1p() - s.label * z)
                })
                .sum::<f64>()
    };
    let mut b = centers.clone();
    for iteration in 1..=MAX_ITERATIONS {
        let mut g: Vec<_> = (0..n).map(|j| lambdas[j] * (b[j] - centers[j])).collect();
        let mut h = vec![0.0; n * n];
        for j in 0..n {
            h[j * n + j] = lambdas[j];
        }
        for (x, s) in rows.iter().zip(samples) {
            let z: f64 = x.iter().map(|&(j, v)| b[j] * v).sum();
            let p = 1.0 / (1.0 + (-z).exp());
            for (a, &(j, v)) in x.iter().enumerate() {
                g[j] += s.weight * (p - s.label) * v;
                for &(k, w) in &x[..=a] {
                    h[j * n + k] += s.weight * p * (1.0 - p) * v * w;
                }
            }
        }
        let precision: Vec<_> = (0..n).map(|j| h[j * n + j]).collect();
        let step = cholesky(h, g)?;
        if step.iter().all(|v| v.abs() < STEP_TOLERANCE) {
            let mut source_sd: BTreeMap<_, _> = feeds
                .iter()
                .map(|(&f, &j)| (f, precision[j].recip().sqrt()))
                .collect();
            for feed in penalty.source_prior.keys() {
                source_sd
                    .entry(*feed)
                    .or_insert(penalty.source.recip().sqrt());
            }
            let mut sources: BTreeMap<_, _> = feeds.iter().map(|(&f, &j)| (f, b[j])).collect();
            let mut sorted: Vec<_> = sources.values().copied().collect();
            sorted.sort_by(f64::total_cmp);
            for (&feed, &center) in &penalty.source_prior {
                sources.entry(feed).or_insert(center);
            }
            return Some(Fit {
                mean,
                intercept: b[0],
                position: std::array::from_fn(|i| b[K + 1 + i]),
                sources,
                source_sd,
                report: Report {
                    samples: samples.len(),
                    positives: samples.iter().filter(|s| s.label > 0.0).count(),
                    beta: std::array::from_fn(|j| b[j + 1]),
                    prior: penalty.prior,
                    stddev: std::array::from_fn(|j| {
                        (samples
                            .iter()
                            .map(|s| s.weight * (s.features[j] - mean[j]).powi(2))
                            .sum::<f64>()
                            / total)
                            .sqrt()
                    }),
                    source_quantiles: std::array::from_fn(|q| sorted[q * (sorted.len() - 1) / 4]),
                    iterations: iteration,
                },
            });
        }
        let old_loss = loss(&b);
        // Allow roundoff at the optimum, so a tiny full step cannot stall.
        b = (0..25).find_map(|backtrack| {
            let rate = 0.5_f64.powi(backtrack);
            let candidate: Vec<_> = b.iter().zip(&step).map(|(b, d)| b - rate * d).collect();
            (loss(&candidate) <= old_loss + 1e-12 * (1.0 + old_loss.abs())).then_some(candidate)
        })?;
    }
    None
}

/// Calibrate continuous hand-scored points to log odds with one predictor.
/// Constant scores, absent outcomes, or fewer than 50 samples keep unit scale.
pub fn point_scale<const K: usize>(samples: &[Sample<K>], scores: &[f64]) -> f64 {
    if samples.len() < 50
        || !scores.windows(2).any(|s| s[0] != s[1])
        || !samples.iter().any(|s| s.label > 0.0)
        || !samples.iter().any(|s| s.label < 1.0)
    {
        return 1.0;
    }
    let rows: Vec<_> = samples
        .iter()
        .zip(scores)
        .map(|(s, &score)| Sample {
            feed: 0,
            features: [score],
            position: None,
            period: s.period,
            label: s.label,
            weight: s.weight,
        })
        .collect();
    fit(
        &rows,
        Penalty {
            beta: WEAK_PENALTY,
            ..Default::default()
        },
    )
    .map(|f| f.report.beta[0])
    .filter(|k| *k > 0.0)
    .unwrap_or(1.0)
}

/// Standard normal draw fixed for one feed on one UTC day (SplitMix64, Box–Muller).
pub fn daily_normal(feed: i64, day: i64) -> f64 {
    let mut state = (feed as u64).wrapping_mul(0x9e3779b97f4a7c15)
        ^ (day as u64).wrapping_mul(0xbf58476d1ce4e5b9);
    let mut uniform = || {
        state = state.wrapping_add(0x9e3779b97f4a7c15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
        ((z ^ (z >> 31)) >> 11) as f64 / (1_u64 << 53) as f64
    };
    let (u, v) = (1.0 - uniform(), uniform());
    (-2.0 * u.ln()).sqrt() * (std::f64::consts::TAU * v).cos()
}

fn cholesky(mut h: Vec<f64>, mut rhs: Vec<f64>) -> Option<Vec<f64>> {
    let n = rhs.len();
    for i in 0..n {
        for j in 0..=i {
            let v = h[i * n + j] - (0..j).map(|k| h[i * n + k] * h[j * n + k]).sum::<f64>();
            h[i * n + j] = if i == j {
                if !v.is_finite() || v <= 0.0 {
                    return None;
                }
                v.sqrt()
            } else {
                v / h[j * n + j]
            };
        }
    }
    for i in 0..n {
        rhs[i] = (rhs[i] - (0..i).map(|j| h[i * n + j] * rhs[j]).sum::<f64>()) / h[i * n + i];
    }
    for i in (0..n).rev() {
        rhs[i] = (rhs[i] - (i + 1..n).map(|j| h[j * n + i] * rhs[j]).sum::<f64>()) / h[i * n + i];
    }
    rhs.iter().all(|v| v.is_finite()).then_some(rhs)
}

#[cfg(test)]
#[path = "read_model_tests.rs"]
mod tests;
