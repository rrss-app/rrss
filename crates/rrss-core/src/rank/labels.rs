//! 阶段 23「M1 隐式标签」「阅读会话」：每篇一票与位置逆倾向权重，纯函数。

use std::collections::{BTreeMap, BTreeSet, HashMap};

use super::engagement::{self, DailySession, Engagement, Profiles, SessionSummary};
use crate::signals::{ArticleEvent, EventKind};

/// 限制单个低概率曝光对指标的影响，最大相当于首位样本的 10 倍。
pub const MAX_IPW: f64 = 10.0;
/// 首位和目标位置各至少 20 次曝光且有打开，才采用经验打开率。
pub const MIN_POSITION_SAMPLES: usize = 20;

// 阶段 23「在 B1 里的权重」：跳出可能只是误点，负证据取显式踩的四分之一；
// 细读取阅读的两倍。显式证据始终覆盖分档，强度固定为一票，不叠加隐式证据。
const BOUNCE_FEEDBACK: f64 = -0.25;
const READ_FEEDBACK: f64 = 1.0;
const DEEP_FEEDBACK: f64 = 2.0;
const EXPLICIT_FEEDBACK: f64 = 1.0;

#[derive(Debug, Clone)]
pub struct LabelExposure {
    pub id: i64,
    pub article_id: i64,
    /// 零起位置，与 E0 前端一致。
    pub position: i64,
    pub shown_at: i64,
}

#[derive(Debug, Clone, Copy)]
pub struct Label {
    pub article_id: i64,
    pub positive: bool,
    /// 评测的位置逆倾向权重，与 B1 的反馈强度独立。
    pub weight: f64,
    pub feedback_weight: f64,
    /// 实际判定证据时间，供 B1 反馈衰减；不使用曝光时间替代。
    pub at: i64,
}

type Vote = (usize, bool, i64, i64);
#[derive(Default)]
struct Explicit {
    vote: Option<Vote>,
    star: Option<Vote>,
}

pub(crate) type ExposureIndex<'a> = HashMap<i64, Vec<&'a LabelExposure>>;

/// 标签与源偏好共用文章索引及稳定的曝光时间顺序。
pub(crate) fn index_exposures<'a>(
    exposures: impl IntoIterator<Item = &'a LabelExposure>,
) -> ExposureIndex<'a> {
    let mut index = ExposureIndex::new();
    for e in exposures {
        index.entry(e.article_id).or_default().push(e);
    }
    for article in index.values_mut() {
        article.sort_by_key(|e| (e.shown_at, e.id));
    }
    index
}

/// 有关联 id 时校验文章与时间；缺失 / 失效 id 退回同篇最近一次先前曝光。
pub(crate) fn exposure_for<'a>(
    exposures: &ExposureIndex<'a>,
    article_id: i64,
    at: i64,
    id: Option<i64>,
) -> Option<&'a LabelExposure> {
    let article = exposures.get(&article_id)?;
    let eligible = &article[..article.partition_point(|e| e.shown_at <= at)];
    id.and_then(|id| eligible.iter().find(|e| e.id == id).copied())
        .or_else(|| eligible.last().copied())
}

/// 位置必须来自同一 surface。只使用 window 内证据；history 必须早于 window。
/// 显式证据按时间（同时间保留输入顺序）取最后一票，unstar 只撤销 star。
/// 无显式票时，同日合并后取最后一个非 Skim 日；曝光本身不给负标签。
pub fn generate(
    exposures: &[LabelExposure],
    events: &[ArticleEvent],
    sessions: &[SessionSummary],
    history: &[DailySession],
    utc_offset_seconds: i32,
) -> Vec<Label> {
    let exposure_index = index_exposures(exposures);
    let mut exposure_ids = HashMap::new();
    for e in exposures {
        exposure_ids.entry(e.id).or_insert(e);
    }
    let mut opened = BTreeSet::new();
    let mut originals = HashMap::<i64, Vec<i64>>::new();
    let mut explicit = BTreeMap::<i64, Explicit>::new();
    let mut ordered: Vec<_> = events.iter().collect();
    ordered.sort_by_key(|e| e.at);
    for (order, event) in ordered.into_iter().enumerate() {
        let Some(exposure) = exposure_for(
            &exposure_index,
            event.article_id,
            event.at,
            event.exposure_id,
        ) else {
            continue;
        };
        if event.kind == EventKind::Open {
            opened.insert(exposure.id);
        }
        if event.kind == EventKind::OpenOriginal {
            originals
                .entry(event.article_id)
                .or_default()
                .push(event.at);
        }
        let entry = explicit.entry(event.article_id).or_default();
        match event.kind {
            EventKind::Star => entry.star = Some((order, true, exposure.id, event.at)),
            EventKind::Unstar => entry.star = None,
            EventKind::Up | EventKind::Cite | EventKind::Ask | EventKind::Down => {
                entry.vote = Some((order, event.kind != EventKind::Down, exposure.id, event.at));
            }
            _ => {}
        }
    }
    let profiles = Profiles::from_history(history);
    let valid_sessions: Vec<_> = sessions
        .iter()
        .filter(|s| exposure_for(&exposure_index, s.article_id, s.opened_at, None).is_some())
        .cloned()
        .map(|mut s| {
            // 原文打开仍交给 engagement 判定，只给同篇同日会话补原始证据。
            let day = |at: i64| {
                (i128::from(at) + i128::from(utc_offset_seconds) * 1000).div_euclid(86_400_000)
            };
            s.opened_original |= originals.get(&s.article_id).is_some_and(|times| {
                times
                    .get(times.partition_point(|at| *at < s.opened_at))
                    .is_some_and(|at| day(*at) == day(s.opened_at))
            });
            s
        })
        .collect();
    let mut evidence = BTreeMap::new();
    for day in engagement::merge_daily_sessions(&valid_sessions, utc_offset_seconds) {
        let feedback_weight = match engagement::classify_daily_session_with(&day, &profiles) {
            Engagement::Deep => DEEP_FEEDBACK,
            Engagement::Read => READ_FEEDBACK,
            Engagement::Bounce => BOUNCE_FEEDBACK,
            Engagement::Skim => continue,
        };
        let at = day.contexts.iter().map(|s| s.opened_at).max().unwrap();
        if let Some(e) = exposure_for(&exposure_index, day.article_id, at, None) {
            evidence.insert(day.article_id, (feedback_weight, e.id, at));
        }
    }
    for (article_id, votes) in explicit {
        if let Some((_, positive, id, at)) =
            votes.vote.into_iter().chain(votes.star).max_by_key(|v| v.0)
        {
            let feedback_weight = if positive {
                EXPLICIT_FEEDBACK
            } else {
                -EXPLICIT_FEEDBACK
            };
            evidence.insert(article_id, (feedback_weight, id, at));
        }
    }
    let mut positions = BTreeMap::<i64, (usize, usize)>::new();
    for e in exposures.iter().filter(|e| e.position >= 0) {
        let counts = positions.entry(e.position).or_default();
        counts.0 += 1;
        counts.1 += usize::from(opened.contains(&e.id));
    }
    let rate = |pos| {
        positions
            .get(&pos)
            .filter(|(n, o)| *n >= MIN_POSITION_SAMPLES && *o > 0)
            .map(|(n, o)| *o as f64 / *n as f64)
    };
    evidence
        .into_iter()
        .filter_map(|(article_id, (feedback_weight, id, at))| {
            let e = exposure_ids.get(&id).filter(|e| e.position >= 0)?;
            // 稀疏位置用 DCG 的对数位置折扣作为启发式先验（非已识别的因果概率）。
            let probability = rate(e.position)
                .zip(rate(0))
                .map(|(p, first)| (p / first).min(1.0))
                .unwrap_or_else(|| 1.0 / (e.position as f64 + 2.0).log2());
            Some(Label {
                article_id,
                positive: feedback_weight > 0.0,
                feedback_weight,
                at,
                weight: (1.0 / probability).min(MAX_IPW),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use engagement::Language;

    fn exposure(id: i64, position: i64) -> LabelExposure {
        LabelExposure {
            id,
            article_id: id,
            position,
            shown_at: 0,
        }
    }

    fn event(id: i64, kind: EventKind, at: i64) -> ArticleEvent {
        ArticleEvent {
            article_id: id,
            kind,
            exposure_id: None,
            at,
        }
    }

    fn session(id: i64) -> SessionSummary {
        SessionSummary {
            article_id: id,
            opened_at: 1,
            active_ms: 200_000,
            read_chars: 900,
            skimmed_chars: 0,
            total_chars: 1000,
            reached_end: true,
            translated: false,
            lang: Language::Chinese,
            summary_only: false,
            opened_original: false,
        }
    }

    #[test]
    fn latest_explicit_vote_and_star_revocation_override_reading() {
        use EventKind::*;
        for (kinds, expected) in [
            (vec![Up, Down], Some((false, -1.0))),
            (vec![Down, Up], Some((true, 1.0))),
            (vec![Star], Some((true, 1.0))),
            (vec![Cite], Some((true, 1.0))),
            (vec![Ask], Some((true, 1.0))),
            (vec![Star, Unstar], Some((true, 2.0))), // 撤销后恢复每日细读权重。
            (vec![Down, Star, Unstar], Some((false, -1.0))),
            (vec![Up, Star, Down, Unstar], Some((false, -1.0))),
        ] {
            let events: Vec<_> = kinds
                .iter()
                .enumerate()
                .rev()
                .map(|(i, k)| event(1, *k, i as i64 + 2))
                .collect();
            let labels = generate(&[exposure(1, 0)], &events, &[session(1)], &[], 0);
            assert_eq!(
                labels.first().map(|l| (l.positive, l.feedback_weight)),
                expected,
                "{kinds:?}"
            );
        }
        assert!(
            generate(
                &[exposure(1, 0)],
                &[event(1, Star, 1), event(1, Unstar, 2)],
                &[],
                &[],
                0
            )
            .is_empty()
        );
    }

    #[test]
    fn daily_engagement_and_summary_original_use_existing_classifier() {
        let mut deep = session(1);
        let mut read = session(2);
        read.read_chars = 500;
        read.active_ms = 50_000;
        let mut bounce = session(3);
        bounce.read_chars = 0;
        bounce.active_ms = 1000;
        let mut skim = session(4);
        skim.read_chars = 0;
        skim.skimmed_chars = 700;
        let mut summary = session(5);
        summary.summary_only = true;
        summary.opened_original = false;
        summary.read_chars = 0;
        summary.reached_end = false;
        summary.active_ms = 0;
        deep.opened_at = 86_400_001;
        let mut earlier_bounce = bounce.clone();
        earlier_bounce.article_id = 1;
        let mut same_day_bounce = earlier_bounce.clone();
        same_day_bounce.opened_at = deep.opened_at + 1;
        let exposures: Vec<_> = (1..=5).map(|id| exposure(id, 0)).collect();
        let labels = generate(
            &exposures,
            &[event(5, EventKind::OpenOriginal, 2)],
            &[
                deep,
                read,
                bounce,
                skim,
                summary,
                earlier_bounce,
                same_day_bounce,
            ],
            &[],
            0,
        );
        assert_eq!(
            labels
                .iter()
                .map(|l| (l.article_id, l.positive, l.feedback_weight))
                .collect::<Vec<_>>(),
            vec![
                (1, true, 2.0),
                (2, true, 1.0),
                (3, false, -0.25),
                (5, true, 1.0)
            ]
        );
    }

    #[test]
    fn empirical_position_rates_sparse_dcg_prior_and_cap() {
        let mut exposures = Vec::new();
        let mut events = Vec::new();
        for pos in 0..=1 {
            for i in 0..20 {
                let id = pos * 20 + i + 1;
                exposures.push(exposure(id, pos));
                if i < if pos == 0 { 10 } else { 5 } {
                    events.push(event(id, EventKind::Open, 1));
                }
                events.push(event(id, EventKind::Up, 2));
            }
        }
        exposures.push(exposure(41, 2));
        events.push(event(41, EventKind::Up, 2));
        exposures.push(exposure(42, 4094));
        events.push(event(42, EventKind::Down, 2));
        let labels = generate(&exposures, &events, &[], &[], 0);
        assert_eq!(labels[0].weight, 1.0);
        assert_eq!(labels[20].weight, 2.0); // (5/20)/(10/20) = 1/2。
        assert_eq!(labels[40].weight, 2.0); // log2(2+2)。
        assert_eq!(labels[41].weight, MAX_IPW);
        let mut no_opens = events.clone();
        no_opens.retain(|e| e.kind != EventKind::Open);
        let prior = generate(&exposures, &no_opens, &[], &[], 0);
        assert!((prior[20].weight - 3_f64.log2()).abs() < 1e-10);
    }
}
