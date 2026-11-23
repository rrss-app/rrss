//! Fitted read prediction; weekly profiles never observe a sample's own outcome.
use super::*;
use crate::{evidence::DAY_MS, labels::Evidence, read_model};
use rrss_core::signals::engagement::SessionSummary;

const WINDOW_MS: i64 = 60 * DAY_MS;
/// Selected positives without a Focus showing receive half weight.
pub(super) const SIDEBAR_WEIGHT: f64 = 0.5;
const SKIP_WEIGHT: f64 = -0.15;
const MIN_HALF_LIFE: i64 = 3_600_000;
const MAX_HALF_LIFE: i64 = 14 * DAY_MS;
pub(super) const ABSOLUTE_HALF_LIFE: i64 = 3 * DAY_MS;

const LAMBDA: f64 = 1.0;
/// Daily source draw in standard deviations, and the cap on that deviation:
/// sources whose effects differ by less than their uncertainty take turns from
/// day to day instead of the same few always winning a tie by noise.
const SOURCE_DRAW: (f64, f64) = (1.0, f64::INFINITY);
#[cfg(any(test, feature = "evaluation-clock"))]
thread_local! {
    static SOURCE_PENALTY_OVERRIDE: std::cell::Cell<f64> = const { std::cell::Cell::new(LAMBDA) };
    static SOURCE_DRAW_OVERRIDE: std::cell::Cell<(f64, f64)> = const { std::cell::Cell::new(SOURCE_DRAW) };
    static FINE_POSITIONS_OVERRIDE: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
}

#[cfg(any(test, feature = "evaluation-clock"))]
fn with_cell<V: Copy + 'static, T>(
    cell: &'static std::thread::LocalKey<std::cell::Cell<V>>,
    value: V,
    f: impl FnOnce() -> T,
) -> T {
    struct Restore<V: Copy + 'static>(&'static std::thread::LocalKey<std::cell::Cell<V>>, V);
    impl<V: Copy + 'static> Drop for Restore<V> {
        fn drop(&mut self) {
            self.0.set(self.1);
        }
    }
    let _restore = Restore(cell, cell.replace(value));
    f()
}

/// Evaluation only: the daily source draw as `scale` standard deviations capped
/// at `cap` (0 turns it off) on this thread.
#[cfg(any(test, feature = "evaluation-clock"))]
pub fn with_source_draw<T>(scale: f64, cap: f64, f: impl FnOnce() -> T) -> T {
    with_cell(&SOURCE_DRAW_OVERRIDE, (scale, cap), f)
}

/// Evaluation only: `false` restores the single 0–9 training position bucket.
#[cfg(any(test, feature = "evaluation-clock"))]
pub fn with_fine_positions<T>(enabled: bool, f: impl FnOnce() -> T) -> T {
    with_cell(&FINE_POSITIONS_OVERRIDE, enabled, f)
}

/// Fixed per feed and UTC day, like the daily exploration trial, so one day's
/// pages and cache refreshes agree.
pub(super) fn source_draw(fit: &read_model::Fit<5>, feed: i64, at: i64) -> f64 {
    #[cfg(any(test, feature = "evaluation-clock"))]
    let (scale, cap) = SOURCE_DRAW_OVERRIDE.get();
    #[cfg(not(any(test, feature = "evaluation-clock")))]
    let (scale, cap) = SOURCE_DRAW;
    if scale == 0.0 {
        return 0.0;
    }
    let sd = fit.source_sd.get(&feed).copied().unwrap_or(0.0).min(cap);
    scale * sd * read_model::daily_normal(feed, at.div_euclid(DAY_MS))
}

/// Evaluation only: shrinkage of per-source effects toward their prior on this thread.
#[cfg(any(test, feature = "evaluation-clock"))]
pub fn with_source_penalty<T>(lambda: f64, f: impl FnOnce() -> T) -> T {
    with_cell(&SOURCE_PENALTY_OVERRIDE, lambda, f)
}

fn source_penalty() -> f64 {
    #[cfg(any(test, feature = "evaluation-clock"))]
    return SOURCE_PENALTY_OVERRIDE.get();
    #[cfg(not(any(test, feature = "evaluation-clock")))]
    LAMBDA
}
/// Subscribing is an explicit choice: a new feed starts as if liked (log odds),
/// fading to neutral over the window; its own Focus outcomes move it either way.
const SUBSCRIPTION_PRIOR: f64 = 1.5;
pub(super) const SUBSCRIPTION_WINDOW: i64 = 21 * DAY_MS;
const HAND_PRIOR: [f64; 5] = [
    combine::WEIGHTS[0],
    combine::WEIGHTS[4],
    -combine::WEIGHTS[3],
    combine::WEIGHTS[2],
    -combine::WEIGHTS[3],
];

pub(super) fn profiles(
    articles: &BTreeMap<i64, Article>,
    vectors: &BTreeMap<i64, Vec<f32>>,
    votes: &[labels::Evidence],
    before: i64,
) -> (Profile, affinity::Profile, affinity::Profile) {
    let weighted = evidence::weight(
        &votes
            .iter()
            .filter_map(|v| articles.get(&v.article_id).map(|a| (*v, a.feed)))
            .collect::<Vec<_>>(),
        before,
    );
    let feedback: Vec<_> = weighted
        .iter()
        .map(|v| (articles[&v.article_id].chips.as_slice(), *v))
        .collect();
    let vector_weights: Vec<_> = weighted
        .iter()
        .filter(|v| vectors.contains_key(&v.article_id))
        .copied()
        .collect();
    let vector_profile = |recent| {
        let feedback: Vec<_> = vector_weights
            .iter()
            .map(|v| affinity::Feedback {
                id: v.article_id,
                vector: &vectors[&v.article_id],
                weight: if recent { v.recent } else { v.long },
            })
            .collect();
        affinity::Profile::learn_temporal(&feedback, &vector_weights, recent)
    };
    (
        Profile::learn_weighted(&feedback),
        vector_profile(false),
        vector_profile(true),
    )
}

pub(super) fn intervals(articles: &BTreeMap<i64, Article>, before: i64) -> BTreeMap<i64, i64> {
    let mut published = BTreeMap::<i64, Vec<i64>>::new();
    for a in articles.values().filter(|a| a.created < before) {
        if let Some(p) = a
            .published
            .filter(|p| *p < before && *p >= before - WINDOW_MS)
        {
            published.entry(a.feed).or_default().push(p);
        }
    }
    published
        .iter()
        .map(|(feed, times)| (*feed, interval_half_life(times)))
        .collect()
}

pub(super) fn train(
    articles: &BTreeMap<i64, Article>,
    vectors: &BTreeMap<i64, Vec<f32>>,
    exposures: &[(LabelExposure, i64)],
    sessions: &[SessionSummary],
    events: &[ArticleEvent],
    subscribed: &BTreeMap<i64, i64>,
    before: i64,
    policy: freshness::Policy,
) -> Option<read_model::Fit<5>> {
    let votes = labels::evidence(events, sessions, &[], 0);
    // Sessions supply eligibility/time even when explicit state revokes a read.
    // evidence() already removes implicit votes for explicitly labelled articles.
    let mut eligibility: Vec<_> = sessions
        .iter()
        .map(|s| labels::Evidence {
            article_id: s.article_id,
            at: s.opened_at,
            weight: 0.0,
            explicit: false,
            exposure_id: None,
        })
        .collect();
    eligibility.extend(votes.into_iter().filter(|v| v.weight > 0.0));
    eligibility.sort_by_key(|v| v.at);
    let eligibility: Vec<_> = eligibility
        .iter()
        .filter_map(|v| articles.get(&v.article_id).map(|a| (*v, a.feed)))
        .collect();
    let mut ops = opportunities(exposures, &eligibility, before);
    ops.retain(|o| articles.contains_key(&o.article));
    let positives = ops.iter().filter(|o| o.reward > 0.0).count();
    if positives < 50 || ops.iter().map(|o| o.feed).collect::<BTreeSet<_>>().len() < 2 {
        return None;
    }
    let mut weeks = BTreeMap::new();
    for o in &ops {
        let week = o.at.div_euclid(7 * evidence::DAY_MS) * 7 * evidence::DAY_MS;
        weeks.entry(week).or_insert_with(|| {
            let events: Vec<_> = events.iter().filter(|e| e.at < week).cloned().collect();
            let sessions: Vec<_> = sessions
                .iter()
                .filter(|s| s.opened_at < week)
                .cloned()
                .collect();
            let mut votes = labels::evidence(&events, &sessions, &[], 0);
            let sources = source_prior(&opportunities(
                exposures,
                &votes
                    .iter()
                    .filter_map(|v| articles.get(&v.article_id).map(|a| (*v, a.feed)))
                    .collect::<Vec<_>>(),
                week,
            ));
            let exposures: Vec<_> = exposures
                .iter()
                .filter(|(e, _)| e.shown_at < week)
                .cloned()
                .collect();
            add_skips(&mut votes, &exposures, &sessions, &events);
            let p = profiles(articles, vectors, &votes, week);
            (p, sources)
        });
    }
    let mut publications = BTreeMap::<i64, Vec<(i64, i64)>>::new();
    // Count only cluster members already ingested at the opportunity time.
    let mut clusters = BTreeMap::<i64, BTreeMap<i64, i64>>::new();
    for a in articles.values() {
        if let Some(p) = a.published {
            publications.entry(a.feed).or_default().push((a.created, p));
        }
        if let Some(c) = a.cluster {
            clusters
                .entry(c)
                .or_default()
                .entry(a.feed)
                .and_modify(|t| *t = (*t).min(a.created))
                .or_insert(a.created);
        }
    }
    let mut hand_scores = Vec::with_capacity(ops.len());
    let samples: Vec<_> = ops
        .iter()
        .zip(weights(&ops))
        .map(|(o, weight)| {
            let a = &articles[&o.article];
            let ((lexical, long, recent), sources) =
                &weeks[&(o.at.div_euclid(7 * evidence::DAY_MS) * 7 * evidence::DAY_MS)];
            let vector = vectors.get(&o.article);
            let times: Vec<_> = publications
                .get(&a.feed)
                .into_iter()
                .flatten()
                .filter(|(created, p)| *created <= o.at && *p < o.at && *p >= o.at - WINDOW_MS)
                .map(|(_, p)| *p)
                .collect();
            let interval = interval_half_life(&times);
            // Shelf life from the rate observable at the sample time (the
            // training lookback is 60 days, within the 28–90 day rate span).
            let shelf = match policy {
                freshness::Policy::Shelf { quadratic } => {
                    let rate =
                        freshness::per_day(times.len(), times.iter().min().map_or(0, |m| o.at - m));
                    Some(
                        (!freshness::exempt(rate))
                            .then(|| (freshness::window_for_rate(rate), quadratic)),
                    )
                }
                freshness::Policy::Legacy => None,
            };
            let published = Some(a.published.unwrap_or(a.created));
            let count = a
                .cluster
                .and_then(|c| clusters.get(&c))
                .map(|feeds| feeds.values().filter(|t| **t <= o.at).count())
                .unwrap_or(1);
            let sample = read_model::Sample {
                feed: o.feed,
                position: o.position,
                label: o.reward,
                weight: weight
                    * if o.position.is_none() && o.reward > 0.0 {
                        SIDEBAR_WEIGHT
                    } else {
                        1.0
                    },
                features: [
                    lexical.raw_score(&a.chips),
                    vector
                        .map(|v| affinity::combined_score(long.score(v).0, recent.score(v).0))
                        .unwrap_or(0.0),
                    match shelf {
                        Some(Some((window, quadratic))) => {
                            -freshness::shelf_cost(published, o.at, window, quadratic)
                        }
                        Some(None) => 0.0,
                        None => freshness::value(published, o.at, interval),
                    },
                    (count.max(1) as f64).ln(),
                    if shelf.is_some() {
                        0.0
                    } else {
                        freshness::value(published, o.at, ABSOLUTE_HALF_LIFE)
                    },
                ],
            };
            // Same manual combiner, with profiles/source evidence preceding the
            // sample's week. Never calibrate against an outcome's own score.
            hand_scores.push(
                combine::combine(
                    Input {
                        features: &lexical.weighted(&a.chips),
                        source: sources.get(&o.feed).copied().unwrap_or(0.0),
                        coverage: (count, sample.features[3]),
                        freshness: if shelf.is_some() {
                            -sample.features[2]
                        } else {
                            1.0 - sample.features[4]
                        },
                        affinity: (sample.features[1], None),
                    },
                    Switches::ALL,
                )
                .raw_score,
            );
            sample
        })
        .collect();
    let k = read_model::point_scale(&samples, &hand_scores);
    // Raw-unit slopes: convert hand points to log odds without SD scaling.
    read_model::fit(
        &samples,
        read_model::Penalty {
            beta: LAMBDA,
            source: source_penalty(),
            prior: HAND_PRIOR.map(|w| k * w),
            source_prior: subscribed
                .iter()
                .map(|(&feed, &at)| {
                    let left = 1.0 - (before - at) as f64 / SUBSCRIPTION_WINDOW as f64;
                    (feed, SUBSCRIPTION_PRIOR * left.clamp(0.0, 1.0))
                })
                .filter(|(_, center)| *center > 0.0)
                .collect(),
        },
    )
}

/// Median gap between consecutive publications in the window.
fn interval_half_life(times: &[i64]) -> i64 {
    let mut t = times.to_vec();
    t.sort_unstable();
    let mut gaps: Vec<i64> = t.windows(2).map(|w| w[1] - w[0]).collect();
    let hl = if gaps.len() < 2 {
        MAX_HALF_LIFE
    } else {
        gaps.sort_unstable();
        gaps[gaps.len() / 2]
    };
    hl.clamp(MIN_HALF_LIFE, MAX_HALF_LIFE)
}

#[derive(Debug, Clone)]
struct Opportunity {
    feed: i64,
    article: i64,
    day: i64,
    at: i64,
    position: Option<u8>,
    reward: f64,
    positive_day: Option<i64>,
}

/// One opportunity per article: shown in Focus, or read from any entry point.
/// A later read of an earlier-shown article is a success, not a failed showing.
fn opportunities(
    exposures: &[(LabelExposure, i64)],
    votes: &[(Evidence, i64)],
    now: i64,
) -> Vec<Opportunity> {
    let start = now - WINDOW_MS;
    let mut map = BTreeMap::<i64, Opportunity>::new();
    for (e, feed) in exposures
        .iter()
        .filter(|(e, _)| e.shown_at >= start && e.shown_at < now)
    {
        let day = e.shown_at.div_euclid(DAY_MS);
        let o = map.entry(e.article_id).or_insert(Opportunity {
            feed: *feed,
            article: e.article_id,
            day,
            at: e.shown_at,
            position: Some(position_dummy(e.position)),
            reward: 0.0,
            positive_day: None,
        });
        o.day = o.day.min(day);
    }
    for (v, feed) in votes.iter().filter(|(v, _)| v.at >= start && v.at < now) {
        let reward = if v.explicit {
            v.weight.clamp(0.0, 1.0)
        } else if v.weight > 1.0 {
            1.0
        } else if v.weight > 0.0 {
            0.75
        } else {
            0.0
        };
        let day = v.at.div_euclid(DAY_MS);
        let o = map.entry(v.article_id).or_insert(Opportunity {
            feed: *feed,
            article: v.article_id,
            day,
            at: v.at,
            position: None,
            reward: 0.0,
            positive_day: None,
        });
        if reward > o.reward {
            o.reward = reward;
            o.positive_day = Some(day);
        }
    }
    map.into_values().collect()
}

/// Same per-source/day opportunity budget as the production source model.
fn weights(ops: &[Opportunity]) -> Vec<f64> {
    let mut n = BTreeMap::<(i64, i64), f64>::new();
    for o in ops {
        *n.entry((o.feed, o.day)).or_default() += 1.0;
    }
    ops.iter()
        .map(|o| (3.0 / n[&(o.feed, o.day)]).min(1.0))
        .collect()
}

/// Fixed hand-scored source prior for point calibration.
/// This is not an alternative fitted source model.
fn source_prior(ops: &[Opportunity]) -> BTreeMap<i64, f64> {
    let prior = source::PRIOR_EXPOSURES;
    let w = weights(ops);
    let mut c = BTreeMap::<i64, (f64, f64)>::new();
    for (o, w) in ops.iter().zip(&w) {
        let e = c.entry(o.feed).or_default();
        e.0 += w;
        e.1 += w * o.reward;
    }
    let (n, s) = c.values().fold((0.0, 0.0), |(n, s), c| (n + c.0, s + c.1));
    if n == 0.0 {
        return BTreeMap::new();
    }
    let mut days = BTreeMap::<i64, BTreeSet<i64>>::new();
    for o in ops {
        if let Some(day) = o.positive_day {
            days.entry(o.feed).or_default().insert(day);
        }
    }
    let mean = ((s + 2.0) / (n + 4.0)).clamp(0.25, 0.5);
    let lo = |p: f64| (p / (1.0 - p)).ln();
    c.into_iter()
        .map(|(f, (n, s))| {
            let raw = lo((s + prior * mean) / (n + prior)) - lo(mean);
            let enough = days.get(&f).is_some_and(|d| d.len() >= 3);
            (
                f,
                if raw > 0.0 && !enough {
                    0.0
                } else {
                    raw.clamp(-0.5, 0.5)
                },
            )
        })
        .collect()
}

/// Training position bucket (see `read_model::Sample::position`): the top
/// three apart from the rest of the first ten, whose reads differ too much to
/// share one correction.
fn position_dummy(position: i64) -> u8 {
    #[cfg(any(test, feature = "evaluation-clock"))]
    if !FINE_POSITIONS_OVERRIDE.get() {
        return position_bucket(position);
    }
    match position {
        ..3 => 0,
        3..10 => 1,
        10..30 => 2,
        _ => 3,
    }
}

fn position_bucket(position: i64) -> u8 {
    if position < 10 {
        0
    } else if position < 30 {
        1
    } else {
        2
    }
}

/// Recompute at each cutoff: future sessions/events must not erase past skips.
pub(super) fn add_skips(
    votes: &mut Vec<Evidence>,
    exposures: &[(LabelExposure, i64)],
    sessions: &[SessionSummary],
    events: &[rrss_core::signals::ArticleEvent],
) {
    let touched: BTreeSet<_> = sessions
        .iter()
        .map(|s| s.article_id)
        .chain(events.iter().map(|e| e.article_id))
        .collect();
    let mut seen = BTreeSet::new();
    for (e, _) in exposures {
        if !touched.contains(&e.article_id) && seen.insert(e.article_id) {
            let weight = SKIP_WEIGHT * [1.0, 0.5, 0.0][position_bucket(e.position) as usize];
            if weight != 0.0 {
                votes.push(Evidence {
                    article_id: e.article_id,
                    weight,
                    at: e.shown_at + 1,
                    explicit: false,
                    exposure_id: Some(e.id),
                });
            }
        }
    }
}
