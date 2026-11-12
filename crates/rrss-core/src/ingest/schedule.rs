//! Persisted poll deadlines. A random sample belongs to one completed fetch,
//! never to a due scan; callers can inject the sample and reuse it on write retry.

const MAX_SLOW_INTERVAL_SECS: i64 = 4 * 60 * 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureClass {
    Fast,
    Slow,
}

/// Also used by server cooldowns without a usable Retry-After value.
pub fn slow_retry_interval(interval_secs: i64, failures: i64) -> i64 {
    interval_secs
        .max(0)
        .saturating_mul(1_i64 << failures.saturating_sub(1).clamp(0, 62))
        .min(MAX_SLOW_INTERVAL_SECS)
        .max(interval_secs.max(0))
}

#[derive(Clone, Copy)]
pub struct FetchSchedule {
    sample: u64,
    pub(super) failure: FailureClass,
    not_before: i64,
    pub(super) error_kind: crate::network::ErrorKind,
    pub(super) status: Option<u16>,
}

impl Default for FetchSchedule {
    fn default() -> Self {
        Self::with_random(|| uuid::Uuid::new_v4().as_u128() as u64)
    }
}

impl FetchSchedule {
    pub fn with_random(random: impl FnOnce() -> u64) -> Self {
        Self {
            sample: random(),
            failure: FailureClass::Fast,
            not_before: i64::MIN,
            error_kind: crate::network::ErrorKind::Other,
            status: None,
        }
    }

    pub fn with_failure(mut self, failure: FailureClass) -> Self {
        self.failure = failure;
        self
    }

    /// Diagnostic metadata travels with the completed request, including retries.
    pub fn with_error(mut self, kind: crate::network::ErrorKind, status: Option<u16>) -> Self {
        self.error_kind = kind;
        self.status = status;
        self
    }

    pub fn not_before(mut self, until: i64) -> Self {
        self.not_before = self.not_before.max(until);
        self
    }

    pub(super) fn next_at(self, now: i64, interval_secs: i64, slow_failures: i64) -> i64 {
        let interval = if slow_failures > 0 {
            slow_retry_interval(interval_secs, slow_failures)
        } else {
            interval_secs.max(0)
        }
        .saturating_mul(1000);
        let max_offset = interval / 10;
        let offset = (self.sample % (max_offset as u64 + 1)) as i64;
        let delay = interval.saturating_add(offset);
        // A configured base above the backoff cap keeps its normal interval
        // and jitter; backoff must never schedule it earlier than a success.
        let delay = if slow_failures > 0 && interval_secs <= MAX_SLOW_INTERVAL_SECS {
            delay.min(MAX_SLOW_INTERVAL_SECS * 1000)
        } else {
            delay
        };
        now.saturating_add(delay).max(self.not_before)
    }
}

/// Expired dates are unusable. Digit-only deltas saturate at the largest UTC
/// millisecond instead of wrapping or shortening a very long server deadline.
pub fn retry_after_deadline(value: &str, now: i64) -> Option<i64> {
    let value = value.trim();
    if !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()) {
        let seconds = value.bytes().fold(0_u64, |seconds, digit| {
            seconds
                .saturating_mul(10)
                .saturating_add(u64::from(digit - b'0'))
        });
        let millis = seconds.saturating_mul(1000).min(i64::MAX as u64) as i64;
        return Some(now.saturating_add(millis));
    }
    let date = httpdate::parse_http_date(value).ok()?;
    let millis = date
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_millis()
        .min(i64::MAX as u128) as i64;
    (millis > now).then_some(millis)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slow_backoff_never_shortens_a_base_interval_above_the_cap() {
        for interval in [6 * 3600, 24 * 3600, i64::MAX] {
            for failures in [1, 2, 8, i64::MAX] {
                assert!(slow_retry_interval(interval, failures) >= interval);
                for sample in [0, 360_000, u64::MAX] {
                    let schedule = FetchSchedule::with_random(|| sample);
                    assert!(
                        schedule.next_at(500, interval, failures)
                            >= schedule.next_at(500, interval, 0)
                    );
                }
            }
        }
    }

    #[test]
    fn retry_after_handles_deltas_http_date_variants_invalid_expired_and_overflow() {
        let now = 784_111_777_000;
        assert_eq!(retry_after_deadline(" 60 ", now), Some(now + 60_000));
        assert_eq!(retry_after_deadline("0", now), Some(now));
        for value in [
            "Sun, 06 Nov 1994 08:50:37 GMT",
            "Sunday, 06-Nov-94 08:50:37 GMT",
            "Sun Nov  6 08:50:37 1994",
        ] {
            assert_eq!(retry_after_deadline(value, now), Some(now + 60_000));
        }
        for value in [
            "",
            "-1",
            "+1",
            "1.5",
            "tomorrow",
            "Sun, 06 Nov 1994 08:49:37 GMT",
        ] {
            assert_eq!(retry_after_deadline(value, now), None, "{value}");
        }
        assert_eq!(
            retry_after_deadline("1844674407370955161600000", now),
            Some(i64::MAX)
        );
        assert_eq!(retry_after_deadline("60", i64::MAX - 1), Some(i64::MAX));
        let server = now + 24 * 3_600_000;
        assert_eq!(
            FetchSchedule::with_random(|| 0)
                .not_before(server)
                .next_at(now, 3600, 3),
            server
        );
    }

    #[test]
    fn injected_offset_is_bounded_and_deadlines_saturate() {
        assert_eq!(
            FetchSchedule::with_random(|| 0).next_at(500, 3600, 0),
            3_600_500
        );
        assert_eq!(
            FetchSchedule::with_random(|| 360_000).next_at(500, 3600, 0),
            3_960_500
        );
        for sample in [360_001, u64::MAX] {
            let next = FetchSchedule::with_random(|| sample).next_at(500, 3600, 0);
            assert!((3_600_500..=3_960_500).contains(&next));
        }
        assert_eq!(
            FetchSchedule::with_random(|| u64::MAX).next_at(i64::MAX - 1, i64::MAX, 0),
            i64::MAX
        );
    }
}
