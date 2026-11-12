//! 对应规格：推荐系统「阅读会话：只记可靠的原始量」
//! 的「派生的投入度」，以及「接入方式」「可维护性约定」。
//!
//! 只消费调用方提供的原始摘要；不访问数据库、signals、时钟或系统时区。

use std::collections::BTreeMap;

pub const MIN_PERSONAL_SAMPLES: usize = 10;
pub const DEFAULT_CHINESE_UNITS_PER_MINUTE: f64 = 400.0;
pub const DEFAULT_ENGLISH_UNITS_PER_MINUTE: f64 = 230.0;
/// 没有词数时，英文按每词平均 5 个字母 + 1 个分隔字符近似；不按 UTF-8 字节计。
pub const ENGLISH_CHARS_PER_WORD: f64 = 6.0;
/// 中文字数或英文估算词数不超过此值，走不依赖时长的短文判定。
pub const SHORT_TEXT_UNITS: f64 = 100.0;
pub const BOUNCE_ACTIVE_MS: u64 = 5_000;
pub const BOUNCE_COVERAGE: f64 = 0.10;
pub const DEFAULT_READ_RATIO: f64 = 0.50;
pub const DEFAULT_DEEP_RATIO: f64 = 0.80;
pub const DEFAULT_SKIM_RATIO: f64 = 0.50;
pub const SPEED_QUANTILE: f64 = 0.50;
pub const READ_QUANTILE: f64 = 0.50;
pub const DEEP_QUANTILE: f64 = 0.75;
pub const SKIM_QUANTILE: f64 = 0.50;
const DAY_MS: i128 = 86_400_000;

/// 可见文本的语言；译文打开时由调用方传入译文语言。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Language {
    Chinese,
    English,
}

impl Language {
    pub fn reading_units(self, chars: u64) -> f64 {
        match self {
            Self::Chinese => chars as f64,
            Self::English => chars as f64 / ENGLISH_CHARS_PER_WORD,
        }
    }
}

/// 字符数来自同一可见文档，active_ms 已扣除暂停。
/// opened_at 是 Unix 毫秒。exposure_id 不影响判定，留在数据层。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSummary {
    pub article_id: i64,
    pub opened_at: i64,
    pub active_ms: u64,
    pub read_chars: u64,
    pub skimmed_chars: u64,
    pub total_chars: u64,
    pub reached_end: bool,
    pub translated: bool,
    pub lang: Language,
    pub summary_only: bool,
    pub opened_original: bool,
}

/// 每篇每天一个结果；不同语言 / 原文与译文的量保持独立，避免混用速度。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DailySession {
    pub article_id: i64,
    /// 本地日期相对 Unix epoch 的天数；不是时间戳。
    pub local_day: i64,
    pub contexts: Vec<SessionSummary>,
}

/// offset 为相对 UTC 的秒数（东为正）；调用方提供所需日期的偏移。
/// 同一篇、同一本地日、同一语言与译文状态内：
/// - active 累加（溢出饱和），总字数、读过 / 扫过字数取最大值；
/// - 无块集合，不能求覆盖并集，故保守取最大值，读过优先，扫过封顶为剩余字数；
/// - 到文末 / 打开原文取任一次，只有摘要取全部均为摘要；
/// - opened_at 取最早一次；诊断字段留在原始日志，不参与判定。
///
/// 输出按 article_id、本地日、语言、译文状态排序。跨日会话归入打开时的日期。
pub fn merge_daily_sessions(
    sessions: &[SessionSummary],
    utc_offset_seconds: i32,
) -> Vec<DailySession> {
    let mut days: BTreeMap<(i64, i64), BTreeMap<(Language, bool), SessionSummary>> =
        BTreeMap::new();
    let mut ordered: Vec<_> = sessions.iter().collect();
    ordered.sort_by_key(|session| session.opened_at);
    for session in ordered {
        let local_ms = i128::from(session.opened_at) + i128::from(utc_offset_seconds) * 1_000;
        let local_day = local_ms.div_euclid(DAY_MS) as i64;
        let contexts = days.entry((session.article_id, local_day)).or_default();
        let key = (session.lang, session.translated);
        if let Some(merged) = contexts.get_mut(&key) {
            merged.active_ms = merged.active_ms.saturating_add(session.active_ms);
            merged.total_chars = merged.total_chars.max(session.total_chars);
            merged.read_chars = merged.read_chars.max(session.read_chars);
            merged.skimmed_chars = merged.skimmed_chars.max(session.skimmed_chars);
            merged.reached_end |= session.reached_end;
            merged.opened_original |= session.opened_original;
            merged.summary_only &= session.summary_only;
        } else {
            contexts.insert(key, session.clone());
        }
    }
    days.into_iter()
        .map(|((article_id, local_day), contexts)| DailySession {
            article_id,
            local_day,
            contexts: contexts
                .into_values()
                .map(|mut session| {
                    session.read_chars = session.read_chars.min(session.total_chars);
                    session.skimmed_chars = session
                        .skimmed_chars
                        .min(session.total_chars - session.read_chars);
                    session
                })
                .collect(),
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReadingSpeed {
    /// 中文每字 / 英文每估算词的毫秒数。先求时间 / 单位数，再取中位数。
    pub ms_per_unit: f64,
    pub samples: usize,
    pub uses_default: bool,
}

impl ReadingSpeed {
    pub fn expected_ms(self, lang: Language, chars: u64) -> f64 {
        lang.reading_units(chars) * self.ms_per_unit
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CoverageThresholds {
    pub read_ratio: f64,
    pub deep_ratio: f64,
    pub skim_ratio: f64,
    pub samples: usize,
    pub uses_default: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ContextProfile {
    pub speed: ReadingSpeed,
    pub thresholds: CoverageThresholds,
}

/// 按语言 / 译文状态预计算的画像；同一份历史用于判定多天时可复用。
#[derive(Debug, Clone, PartialEq)]
pub struct Profiles {
    chinese: [ContextProfile; 2],
    english: [ContextProfile; 2],
}

impl Profiles {
    /// 历史已按本地日合并，且必须早于所有待判会话，避免自我校准。
    pub fn from_history(history: &[DailySession]) -> Self {
        let [chinese, english] = [Language::Chinese, Language::English].map(|lang| {
            [false, true].map(|translated| ContextProfile {
                speed: estimate_reading_speed(history, lang, translated),
                thresholds: estimate_coverage_thresholds(history, lang, translated),
            })
        });
        Self { chinese, english }
    }

    fn get(&self, lang: Language, translated: bool) -> ContextProfile {
        let contexts = match lang {
            Language::Chinese => &self.chinese,
            Language::English => &self.english,
        };
        contexts[usize::from(translated)]
    }
}

/// 历史已按本地日合并，调用方只传目标会话之前的历史，避免待判会话自我校准。
pub fn estimate_reading_speed(
    history: &[DailySession],
    lang: Language,
    translated: bool,
) -> ReadingSpeed {
    let mut samples: Vec<f64> = matching_contexts(history, lang, translated)
        .filter(|s| calibratable(s) && s.reached_end && s.active_ms > 0 && s.read_chars > 0)
        .map(|s| s.active_ms as f64 / lang.reading_units(s.read_chars.min(s.total_chars)))
        .collect();
    let uses_default = samples.len() < MIN_PERSONAL_SAMPLES;
    let default_rate = match lang {
        Language::Chinese => DEFAULT_CHINESE_UNITS_PER_MINUTE,
        Language::English => DEFAULT_ENGLISH_UNITS_PER_MINUTE,
    };
    ReadingSpeed {
        ms_per_unit: if uses_default {
            60_000.0 / default_rate
        } else {
            quantile(&mut samples, SPEED_QUANTILE)
        },
        samples: samples.len(),
        uses_default,
    }
}

/// 非短文、非摘要、有效时间非零且非跳出的历史；与速度一样按语言 / 译文分组。
pub fn estimate_coverage_thresholds(
    history: &[DailySession],
    lang: Language,
    translated: bool,
) -> CoverageThresholds {
    let (mut read, mut skim): (Vec<_>, Vec<_>) = matching_contexts(history, lang, translated)
        .filter(|s| calibratable(s) && s.active_ms > 0 && !is_bounce(s))
        .map(coverage)
        .unzip();
    let samples = read.len();
    let uses_default = samples < MIN_PERSONAL_SAMPLES;
    let (read_ratio, deep_ratio, skim_ratio) = if uses_default {
        (DEFAULT_READ_RATIO, DEFAULT_DEEP_RATIO, DEFAULT_SKIM_RATIO)
    } else {
        (
            quantile(&mut read, READ_QUANTILE),
            quantile(&mut read, DEEP_QUANTILE),
            quantile(&mut skim, SKIM_QUANTILE),
        )
    };
    CoverageThresholds {
        read_ratio,
        deep_ratio,
        skim_ratio,
        samples,
        uses_default,
    }
}

fn matching_contexts(
    history: &[DailySession],
    lang: Language,
    translated: bool,
) -> impl Iterator<Item = &SessionSummary> {
    history
        .iter()
        .flat_map(|day| &day.contexts)
        .filter(move |s| s.lang == lang && s.translated == translated)
}

fn calibratable(session: &SessionSummary) -> bool {
    !session.summary_only && session.lang.reading_units(session.total_chars) > SHORT_TEXT_UNITS
}

fn coverage(session: &SessionSummary) -> (f64, f64) {
    if session.total_chars == 0 {
        return (0.0, 0.0);
    }
    let read = session.read_chars.min(session.total_chars);
    let skim = session.skimmed_chars.min(session.total_chars - read);
    (
        read as f64 / session.total_chars as f64,
        skim as f64 / session.total_chars as f64,
    )
}

fn is_bounce(session: &SessionSummary) -> bool {
    let (read, skim) = coverage(session);
    session.total_chars > 0 && session.active_ms < BOUNCE_ACTIVE_MS && read + skim < BOUNCE_COVERAGE
}

/// 线性插值分位数；偶数样本的中位数为中间两项的均值。
fn quantile(samples: &mut [f64], fraction: f64) -> f64 {
    samples.sort_unstable_by(f64::total_cmp);
    let index = (samples.len() - 1) as f64 * fraction;
    let low = index.floor() as usize;
    let high = index.ceil() as usize;
    samples[low] + (samples[high] - samples[low]) * (index - low as f64)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Engagement {
    Bounce,
    Skim,
    Read,
    Deep,
}

/// profile 必须来自相同语言与译文状态的历史。
/// 短文 / 摘要只看读完或打开原文，未完成记中性 Skim，不因时长给负反馈。
/// 其余按 Bounce → Deep → Read → Skim 判定；不足阅读阈值的零散覆盖也记 Skim。
pub fn classify_session(session: &SessionSummary, profile: ContextProfile) -> Engagement {
    if !calibratable(session) {
        return if (session.total_chars > 0 && session.reached_end) || session.opened_original {
            Engagement::Read
        } else {
            Engagement::Skim
        };
    }
    if is_bounce(session) {
        return Engagement::Bounce;
    }
    let (read, _) = coverage(session);
    let thresholds = profile.thresholds;
    if read > 0.0
        && read >= thresholds.deep_ratio.max(thresholds.read_ratio)
        && session.active_ms as f64 > profile.speed.expected_ms(session.lang, session.total_chars)
    {
        return Engagement::Deep;
    }
    if read > 0.0 && read >= thresholds.read_ratio {
        return Engagement::Read;
    }
    // 四档没有“未定”档；未达阅读标准的部分阅读保持中性。
    Engagement::Skim
}

/// 使用预计算画像，取一天各语言 / 译文上下文中最强的证据。
pub fn classify_daily_session_with(day: &DailySession, profiles: &Profiles) -> Engagement {
    day.contexts
        .iter()
        .map(|session| classify_session(session, profiles.get(session.lang, session.translated)))
        .max()
        .unwrap_or(Engagement::Skim)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(article_id: i64) -> SessionSummary {
        SessionSummary {
            article_id,
            opened_at: 0,
            active_ms: 150_000,
            read_chars: 1_000,
            skimmed_chars: 0,
            total_chars: 1_000,
            reached_end: true,
            translated: false,
            lang: Language::Chinese,
            summary_only: false,
            opened_original: false,
        }
    }

    fn history(
        count: usize,
        lang: Language,
        translated: bool,
        ms_per_unit: f64,
    ) -> Vec<DailySession> {
        let sessions: Vec<_> = (0..count)
            .map(|i| {
                let mut s = session(i as i64);
                s.lang = lang;
                s.translated = translated;
                s.active_ms = (lang.reading_units(s.read_chars) * ms_per_unit) as u64;
                s
            })
            .collect();
        merge_daily_sessions(&sessions, 0)
    }

    fn profile(history: &[DailySession], s: &SessionSummary) -> ContextProfile {
        ContextProfile {
            speed: estimate_reading_speed(history, s.lang, s.translated),
            thresholds: estimate_coverage_thresholds(history, s.lang, s.translated),
        }
    }

    fn classify(s: &SessionSummary) -> Engagement {
        classify_session(s, profile(&[], s))
    }

    fn close(actual: f64, expected: f64) {
        assert!((actual - expected).abs() < 1e-8, "{actual} != {expected}");
    }

    #[test]
    fn insufficient_samples_use_default_speed_and_thresholds() {
        for lang in [Language::Chinese, Language::English] {
            for count in [0, 9] {
                let h = history(count, lang, false, 20.0);
                let speed = estimate_reading_speed(&h, lang, false);
                assert!(speed.uses_default);
                assert_eq!(speed.samples, count);
                let rate = match lang {
                    Language::Chinese => 400.0,
                    Language::English => 230.0,
                };
                close(speed.ms_per_unit, 60_000.0 / rate);
                let thresholds = estimate_coverage_thresholds(&h, lang, false);
                assert!(thresholds.uses_default);
                close(thresholds.read_ratio, 0.5);
                close(thresholds.deep_ratio, 0.8);
                close(thresholds.skim_ratio, 0.5);
            }
        }
    }

    #[test]
    fn language_and_translation_contexts_calibrate_separately() {
        let mut h = Vec::new();
        for (lang, translated, count, ms_per_unit) in [
            (Language::Chinese, false, 9, 100.0),
            (Language::Chinese, true, 10, 500.0),
            (Language::English, false, 10, 300.0),
            (Language::English, true, 10, 90.0),
        ] {
            h.extend(history(count, lang, translated, ms_per_unit));
        }
        for (lang, translated, count, expected) in [
            (Language::Chinese, false, 9, 150.0),
            (Language::Chinese, true, 10, 500.0),
            (Language::English, false, 10, 300.0),
            (Language::English, true, 10, 90.0),
        ] {
            let speed = estimate_reading_speed(&h, lang, translated);
            assert_eq!(speed.samples, count);
            assert_eq!(speed.uses_default, count < MIN_PERSONAL_SAMPLES);
            assert_eq!(
                estimate_coverage_thresholds(&h, lang, translated).uses_default,
                count < MIN_PERSONAL_SAMPLES
            );
            close(speed.ms_per_unit, expected);
        }
        close(
            estimate_reading_speed(&h, Language::English, false)
                .expected_ms(Language::English, 600),
            30_000.0,
        );
    }

    #[test]
    fn median_is_of_individual_time_per_unit_not_pooled_totals() {
        let sessions: Vec<_> = (0..10)
            .map(|i| {
                let mut s = session(i);
                s.read_chars = if i < 5 { 1_000 } else { 10_000 };
                s.total_chars = s.read_chars;
                s.active_ms = s.read_chars * if i < 5 { 100 } else { 300 };
                s
            })
            .collect();
        close(
            estimate_reading_speed(
                &merge_daily_sessions(&sessions, 0),
                Language::Chinese,
                false,
            )
            .ms_per_unit,
            200.0,
        );
    }

    #[test]
    fn speed_uses_only_reliable_completed_long_sessions() {
        let mut sessions = Vec::new();
        for i in 0..5 {
            let mut s = session(i);
            match i {
                0 => s.reached_end = false,
                1 => s.active_ms = 0,
                2 => s.read_chars = 0,
                3 => s.summary_only = true,
                _ => {
                    s.total_chars = 100;
                    s.read_chars = 100;
                }
            }
            sessions.push(s);
        }
        let speed = estimate_reading_speed(
            &merge_daily_sessions(&sessions, 0),
            Language::Chinese,
            false,
        );
        assert_eq!(speed.samples, 0);
        assert!(speed.uses_default);
    }

    #[test]
    fn same_day_merges_conservatively_and_preserves_earliest_open() {
        let mut early = session(1);
        early.active_ms = 10_000;
        early.read_chars = 300;
        early.skimmed_chars = 600;
        early.reached_end = false;
        let mut middle = early.clone();
        middle.opened_at = 1_000;
        middle.read_chars = 600;
        middle.skimmed_chars = 500;
        let mut late = early.clone();
        late.opened_at = 2_000;
        late.reached_end = true;
        late.opened_original = true;
        let days = merge_daily_sessions(&[late, early, middle], 0);
        assert_eq!(days.len(), 1);
        assert_eq!(days[0].contexts.len(), 1);
        let merged = &days[0].contexts[0];
        assert_eq!(merged.active_ms, 30_000);
        assert_eq!(merged.opened_at, 0);
        assert_eq!((merged.read_chars, merged.skimmed_chars), (600, 400));
        assert!(merged.reached_end && merged.opened_original);
    }

    #[test]
    fn local_date_controls_merging_including_negative_epoch() {
        let mut a = session(1);
        let mut b = a.clone();
        a.opened_at = 23 * 3_600_000;
        b.opened_at = 25 * 3_600_000;
        assert_eq!(merge_daily_sessions(&[a.clone(), b.clone()], 0).len(), 2);
        assert_eq!(
            merge_daily_sessions(&[a.clone(), b.clone()], 7_200).len(),
            1
        );
        a.opened_at = 21 * 3_600_000;
        b.opened_at = 23 * 3_600_000;
        assert_eq!(merge_daily_sessions(&[a.clone(), b.clone()], 0).len(), 1);
        assert_eq!(
            merge_daily_sessions(&[a.clone(), b.clone()], 7_200).len(),
            2
        );
        a.opened_at = 0;
        assert_eq!(merge_daily_sessions(&[a], -3_600)[0].local_day, -1);
    }

    #[test]
    fn repeated_opens_are_one_calibration_sample_and_other_articles_stay_separate() {
        let repeated = vec![session(1); 10];
        let days = merge_daily_sessions(&repeated, 0);
        assert_eq!(
            estimate_reading_speed(&days, Language::Chinese, false).samples,
            1
        );
        assert_eq!(merge_daily_sessions(&[session(1), session(2)], 0).len(), 2);
    }

    #[test]
    fn mixed_translation_day_keeps_speeds_separate_but_returns_one_grade() {
        let mut original = session(1);
        original.active_ms = 1_000;
        original.read_chars = 0;
        original.reached_end = false;
        let mut translated = session(1);
        translated.translated = true;
        let days = merge_daily_sessions(&[original, translated], 0);
        assert_eq!(days.len(), 1);
        assert_eq!(days[0].contexts.len(), 2);
        assert_eq!(
            classify_daily_session_with(&days[0], &Profiles::from_history(&[])),
            Engagement::Read
        );
        assert_eq!(
            estimate_reading_speed(&days, Language::Chinese, false).samples,
            0
        );
        assert_eq!(
            estimate_reading_speed(&days, Language::Chinese, true).samples,
            1
        );
    }

    #[test]
    fn precomputed_profiles_match_daily_classification() {
        let mut h = history(10, Language::Chinese, false, 60.0);
        h.extend(history(9, Language::Chinese, true, 300.0));
        h.extend(history(10, Language::English, false, 100.0));
        h.extend(history(10, Language::English, true, 500.0));
        for day in &mut h {
            if day.contexts[0].translated {
                day.contexts[0].total_chars = 2_000;
                day.contexts[0].skimmed_chars = 1_000;
            }
        }
        let mut sessions = Vec::new();
        for lang in [Language::Chinese, Language::English] {
            for translated in [false, true] {
                for case in 0..6 {
                    let mut s = session(case);
                    s.lang = lang;
                    s.translated = translated;
                    match case {
                        0 => {
                            s.read_chars = 0;
                            s.active_ms = 1_000;
                        }
                        1 => {
                            s.read_chars = 300;
                            s.skimmed_chars = 700;
                        }
                        2 => s.read_chars = 600,
                        3 => s.active_ms = 600_000,
                        4 => s.summary_only = true,
                        _ => s.total_chars = 0,
                    }
                    sessions.push(s);
                }
            }
        }
        let mut days = merge_daily_sessions(&sessions, 0);
        // 同时覆盖单一上下文，避免最强证据掩盖画像选错。
        days.extend(sessions.into_iter().map(|s| DailySession {
            article_id: s.article_id,
            local_day: 0,
            contexts: vec![s],
        }));
        days.push(DailySession {
            article_id: 99,
            local_day: 0,
            contexts: vec![],
        });
        for history in [&[][..], h.as_slice()] {
            let profiles = Profiles::from_history(history);
            for day in &days {
                let expected = day
                    .contexts
                    .iter()
                    .map(|s| classify_session(s, profile(history, s)))
                    .max()
                    .unwrap_or(Engagement::Skim);
                assert_eq!(classify_daily_session_with(day, &profiles), expected);
            }
        }
    }

    #[test]
    fn short_text_ignores_time_and_requires_completion_or_original() {
        for (lang, chars) in [(Language::Chinese, 100), (Language::English, 600)] {
            let mut s = session(1);
            s.lang = lang;
            s.total_chars = chars;
            s.read_chars = 0;
            s.active_ms = 0;
            assert_eq!(classify(&s), Engagement::Read);
            s.reached_end = false;
            s.active_ms = 1_800_000;
            assert_eq!(classify(&s), Engagement::Skim);
            s.opened_original = true;
            assert_eq!(classify(&s), Engagement::Read);
        }
    }

    #[test]
    fn summary_original_is_read_even_with_no_coverage_or_time() {
        let mut s = session(1);
        s.summary_only = true;
        s.read_chars = 0;
        s.active_ms = 0;
        s.reached_end = false;
        assert_eq!(classify(&s), Engagement::Skim);
        s.opened_original = true;
        assert_eq!(classify(&s), Engagement::Read);
        s.opened_original = false;
        s.reached_end = true;
        assert_eq!(classify(&s), Engagement::Read);
    }

    #[test]
    fn bounce_needs_both_low_active_time_and_low_combined_coverage() {
        let mut s = session(1);
        s.reached_end = false;
        s.active_ms = 4_999;
        s.read_chars = 20;
        s.skimmed_chars = 79;
        assert_eq!(classify(&s), Engagement::Bounce);
        s.skimmed_chars = 80;
        assert_eq!(classify(&s), Engagement::Skim);
        s.skimmed_chars = 0;
        s.active_ms = 5_000;
        assert_eq!(classify(&s), Engagement::Skim);
    }

    #[test]
    fn skim_read_and_deep_obey_coverage_and_expected_time() {
        let mut s = session(1);
        s.reached_end = false;
        s.active_ms = 50_000;
        s.read_chars = 100;
        s.skimmed_chars = 800;
        assert_eq!(classify(&s), Engagement::Skim);
        s.read_chars = 500;
        s.skimmed_chars = 0;
        assert_eq!(classify(&s), Engagement::Read);
        s.read_chars = 800;
        s.active_ms = 150_000;
        assert_eq!(classify(&s), Engagement::Read);
        s.active_ms += 1;
        assert_eq!(classify(&s), Engagement::Deep);
        s.read_chars = 799;
        assert_eq!(classify(&s), Engagement::Read);
    }

    #[test]
    fn coverage_quantiles_follow_personal_distribution() {
        let histories = [200, 600].map(|base| {
            let sessions: Vec<_> = (0..10)
                .map(|i| {
                    let mut s = session(i);
                    s.read_chars = base + i as u64 * 20;
                    s.skimmed_chars = 1_000 - s.read_chars;
                    s
                })
                .collect();
            merge_daily_sessions(&sessions, 0)
        });
        let low = estimate_coverage_thresholds(&histories[0], Language::Chinese, false);
        let high = estimate_coverage_thresholds(&histories[1], Language::Chinese, false);
        assert!(!low.uses_default && !high.uses_default);
        close(low.read_ratio, 0.29);
        close(low.deep_ratio, 0.335);
        close(low.skim_ratio, 0.71);
        close(high.read_ratio, 0.69);
        close(high.deep_ratio, 0.735);
        let mut s = session(20);
        s.read_chars = 500;
        s.active_ms = 10_000;
        s.reached_end = false;
        assert_eq!(
            classify_session(&s, profile(&histories[0], &s)),
            Engagement::Read
        );
        assert_eq!(
            classify_session(&s, profile(&histories[1], &s)),
            Engagement::Skim
        );
    }

    #[test]
    fn fast_and_slow_readers_get_different_grades_for_the_same_article() {
        let fast = history(10, Language::Chinese, false, 60.0);
        let slow = history(10, Language::Chinese, false, 300.0);
        let mut s = session(20);
        s.active_ms = 120_000;
        assert_eq!(classify_session(&s, profile(&fast, &s)), Engagement::Deep);
        assert_eq!(classify_session(&s, profile(&slow, &s)), Engagement::Read);
    }

    #[test]
    fn zero_coverage_does_not_become_read_when_personal_median_is_zero() {
        let mut sessions: Vec<_> = (0..10).map(session).collect();
        for s in &mut sessions {
            s.read_chars = 0;
            s.skimmed_chars = 1_000;
        }
        let h = merge_daily_sessions(&sessions, 0);
        let s = &sessions[0];
        close(profile(&h, s).thresholds.read_ratio, 0.0);
        assert_eq!(classify_session(s, profile(&h, s)), Engagement::Skim);
        let mut empty = session(20);
        empty.total_chars = 0;
        assert_eq!(classify(&empty), Engagement::Skim);
    }

    #[test]
    fn merge_caps_invalid_coverage_and_saturates_time() {
        let mut s = session(1);
        s.read_chars = 2_000;
        s.skimmed_chars = 2_000;
        s.active_ms = u64::MAX;
        let days = merge_daily_sessions(&[s.clone(), s], 0);
        let merged = &days[0].contexts[0];
        assert_eq!((merged.read_chars, merged.skimmed_chars), (1_000, 0));
        assert_eq!(merged.active_ms, u64::MAX);
        let mut extreme = session(2);
        extreme.opened_at = i64::MAX;
        assert_eq!(merge_daily_sessions(&[extreme], i32::MAX).len(), 1);
    }
}
