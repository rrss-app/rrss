//! 阶段 23「M1 隐式标签」「阅读会话」：行为与会话 → 每篇每日一份证据，纯函数。

use std::collections::{BTreeMap, HashMap};

use crate::signals::engagement::{self, DailySession, Engagement, Profiles, SessionSummary};
use crate::signals::{ArticleEvent, EventKind};

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

type Vote = (usize, bool, Option<i64>, i64);
#[derive(Default)]
struct Explicit {
    vote: Option<Vote>,
    star: Option<Vote>,
}

/// One observation per article/day, or the latest active explicit state for an article.
/// No exposure is invented here: content learning may use reading from any entry point.
#[derive(Debug, Clone, Copy)]
pub struct Evidence {
    pub article_id: i64,
    pub weight: f64,
    pub at: i64,
    pub explicit: bool,
    pub exposure_id: Option<i64>,
}

pub fn evidence(
    events: &[ArticleEvent],
    sessions: &[SessionSummary],
    history: &[DailySession],
    utc_offset_seconds: i32,
) -> Vec<Evidence> {
    let mut originals = HashMap::<i64, Vec<i64>>::new();
    let mut explicit = BTreeMap::<i64, Explicit>::new();
    let mut ordered: Vec<_> = events.iter().collect();
    ordered.sort_by_key(|e| e.at);
    for (order, event) in ordered.into_iter().enumerate() {
        if event.kind == EventKind::OpenOriginal {
            originals
                .entry(event.article_id)
                .or_default()
                .push(event.at);
        }
        let entry = explicit.entry(event.article_id).or_default();
        match event.kind {
            EventKind::Star => entry.star = Some((order, true, event.exposure_id, event.at)),
            EventKind::Unstar => entry.star = None,
            EventKind::Up | EventKind::Cite | EventKind::Ask | EventKind::Down => {
                entry.vote = Some((
                    order,
                    event.kind != EventKind::Down,
                    event.exposure_id,
                    event.at,
                ));
            }
            _ => {}
        }
    }
    let profiles = Profiles::from_history(history);
    let valid_sessions: Vec<_> = sessions
        .iter()
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
    let day_of = |at: i64| {
        (i128::from(at) + i128::from(utc_offset_seconds) * 1000).div_euclid(86_400_000) as i64
    };
    let mut last_evidence = BTreeMap::<(i64, i64), i64>::new();
    for s in &valid_sessions {
        let day = day_of(s.opened_at);
        let mut at = s.opened_at;
        if s.opened_original {
            if let Some(times) = originals.get(&s.article_id) {
                if let Some(original) = times.iter().rev().find(|at| day_of(**at) == day) {
                    at = at.max(*original);
                }
            }
        }
        last_evidence
            .entry((s.article_id, day))
            .and_modify(|old| *old = (*old).max(at))
            .or_insert(at);
    }
    let mut evidence = Vec::new();
    for day in engagement::merge_daily_sessions(&valid_sessions, utc_offset_seconds) {
        let feedback_weight = match engagement::classify_daily_session_with(&day, &profiles) {
            Engagement::Deep => DEEP_FEEDBACK,
            Engagement::Read => READ_FEEDBACK,
            Engagement::Bounce => BOUNCE_FEEDBACK,
            Engagement::Skim => continue,
        };
        let at = last_evidence[&(day.article_id, day.local_day)];
        evidence.push(Evidence {
            article_id: day.article_id,
            weight: feedback_weight,
            at,
            explicit: false,
            exposure_id: None,
        });
    }
    let mut explicit_evidence = BTreeMap::new();
    for (article_id, votes) in explicit {
        if let Some((_, positive, exposure_id, at)) =
            votes.vote.into_iter().chain(votes.star).max_by_key(|v| v.0)
        {
            let feedback_weight = if positive {
                EXPLICIT_FEEDBACK
            } else {
                -EXPLICIT_FEEDBACK
            };
            explicit_evidence.insert(
                article_id,
                Evidence {
                    article_id,
                    weight: feedback_weight,
                    at,
                    explicit: true,
                    exposure_id,
                },
            );
        }
    }
    evidence.retain(|e| !explicit_evidence.contains_key(&e.article_id));
    evidence.extend(explicit_evidence.into_values());
    evidence.sort_by_key(|e| (e.at, e.article_id));
    evidence
}

#[cfg(test)]
mod tests {
    use super::*;
    use engagement::Language;

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
            let labels = evidence(&events, &[session(1)], &[], 0);
            assert_eq!(
                labels.last().map(|l| (l.weight > 0.0, l.weight)),
                expected,
                "{kinds:?}"
            );
        }
        assert!(evidence(&[event(1, Star, 1), event(1, Unstar, 2)], &[], &[], 0).is_empty());
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
        let labels = evidence(
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
        // Latest evidence per article, as the source and content learners consume it.
        let latest: BTreeMap<_, _> = labels
            .iter()
            .map(|l| (l.article_id, (l.weight > 0.0, l.weight)))
            .collect();
        assert_eq!(
            latest
                .into_iter()
                .map(|(id, (up, w))| (id, up, w))
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
    fn daily_evidence_preserves_cross_day_reading_and_explicit_retractions() {
        let mut first = session(1);
        let mut second = first.clone();
        second.opened_at += 86_400_000;
        let sessions = [first.clone(), first.clone(), second];
        let implicit = evidence(&[], &sessions, &[], 0);
        assert_eq!(implicit.len(), 2);
        assert!(implicit.iter().all(|v| !v.explicit && v.weight == 2.0));
        let events = [
            event(1, EventKind::Star, 100_000_000),
            event(1, EventKind::Unstar, 200_000_000),
        ];
        assert_eq!(evidence(&events, &sessions, &[], 0).len(), 2);
        let starred = evidence(&events[..1], &sessions, &[], 0);
        assert_eq!(starred.len(), 1);
        assert!(starred[0].explicit);
        assert_eq!(starred[0].at, 100_000_000);
        assert!(evidence(&[event(2, EventKind::Open, 1)], &[], &[], 0).is_empty());
        first.total_chars = 40;
        first.read_chars = 40;
        first.active_ms = 500;
        first.reached_end = true;
        assert_eq!(evidence(&[], &[first], &[], 0)[0].weight, 1.0);
    }
}
