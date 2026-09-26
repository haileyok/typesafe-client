//! Retry policy: defaults, delay calculation, retry-after parsing, and stop
//! conditions.
//!
//! The default matches both official SDKs: 2 retries, 500ms initial backoff
//! doubling to 5s with 25% jitter, retries on 408/429/5xx, honored
//! `retry-after-ms`/`Retry-After` capped at 60s, connection and timeout
//! retries, and a 30s total budget.

use std::collections::BTreeSet;
use std::time::{Duration, SystemTime};

use crate::errors::{ApiError, Error};

/// The `retry-after-ms` response header.
pub(crate) const RETRY_AFTER_MS_HEADER: &str = "retry-after-ms";
/// The `Retry-After` response header.
pub(crate) const RETRY_AFTER_HEADER: &str = "retry-after";

/// Configuration for retry behavior.
///
/// All fields are public. The [`Default`] value matches the SPEC and both
/// official SDKs.
///
/// ```
/// use std::time::Duration;
/// use typesafe_system_one::{Client, RetryPolicy};
///
/// let client = Client::builder()
///     .api_key("key")
///     .retry_policy(RetryPolicy {
///         max_retries: 3,
///         total_budget: Some(Duration::from_secs(10)),
///         ..Default::default()
///     })
///     .build()
///     .unwrap();
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct RetryPolicy {
    /// Maximum retries after the initial attempt; `0` disables retries.
    pub max_retries: u32,
    /// First backoff delay, doubled each retry up to [`Self::backoff_max`];
    /// zero disables backoff.
    pub backoff_initial: Duration,
    /// Maximum backoff delay; zero disables backoff.
    pub backoff_max: Duration,
    /// Fraction of each backoff delay randomly subtracted; between 0 and 1.
    pub backoff_jitter: f64,
    /// HTTP status codes that are retried.
    pub http_statuses: BTreeSet<u16>,
    /// Whether to honor `retry-after-ms` and `Retry-After` headers.
    pub respect_retry_after: bool,
    /// Maximum honored server retry delay; a longer delay falls back to backoff.
    pub max_retry_after: Duration,
    /// Whether to retry connection errors (no HTTP response at all).
    pub retry_connection_errors: bool,
    /// Whether to retry attempts that exceeded the per-attempt timeout.
    pub retry_timeouts: bool,
    /// Retry budget per call, measured from the first attempt; `None` or
    /// `Some(Duration::ZERO)` disables it.
    ///
    /// A retry is not started when the elapsed time plus its delay would
    /// reach or exceed the budget; the call then returns the **last real
    /// error** rather than an artificial timeout, so the caller sees the
    /// actual 529. An attempt that has started runs to its own per-attempt
    /// timeout, so a call can overrun the budget by up to one timeout (the
    /// same semantics as the official Python SDK). For a hard bound, wrap
    /// the call in `tokio::time::timeout`.
    pub total_budget: Option<Duration>,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        let mut http_statuses = BTreeSet::new();
        http_statuses.insert(408);
        http_statuses.insert(429);
        http_statuses.extend(500..=599);
        Self {
            max_retries: 2,
            backoff_initial: Duration::from_millis(500),
            backoff_max: Duration::from_secs(5),
            backoff_jitter: 0.25,
            http_statuses,
            respect_retry_after: true,
            max_retry_after: Duration::from_secs(60),
            retry_connection_errors: true,
            retry_timeouts: true,
            total_budget: Some(Duration::from_secs(30)),
        }
    }
}

impl RetryPolicy {
    /// A policy that never retries (`max_retries = 0`).
    pub fn none() -> Self {
        Self {
            max_retries: 0,
            ..Default::default()
        }
    }

    /// Validates the policy, returning `Err(Error::Config)` on a bad setting.
    pub(super) fn validate(&self) -> Result<(), Error> {
        if !(0.0..=1.0).contains(&self.backoff_jitter) || !self.backoff_jitter.is_finite() {
            return Err(Error::Config(
                "retry policy backoff_jitter must be between 0 and 1.".into(),
            ));
        }
        Ok(())
    }

    /// Whether this error should be retried under this policy.
    pub(super) fn is_retryable(&self, error: &Error) -> bool {
        match error {
            Error::Timeout { .. } => self.retry_timeouts,
            Error::Connection { .. } => self.retry_connection_errors,
            Error::Api(api) => self.is_retryable_status(api.status),
            _ => false,
        }
    }

    /// Whether an HTTP status is retried under this policy.
    pub fn is_retryable_status(&self, status: u16) -> bool {
        self.http_statuses.contains(&status)
    }

    /// Parses the retry delay from response headers, if present and valid.
    ///
    /// Prefers `retry-after-ms` (float milliseconds, ≥ 0). Otherwise
    /// `Retry-After` as float seconds ≥ 0, or as an HTTP date
    /// (delay = max(0, date − now)). Negative or unparseable values mean
    /// "absent".
    pub(super) fn parse_retry_after(
        &self,
        headers: &reqwest::header::HeaderMap,
    ) -> Option<Duration> {
        if let Some(raw) = header_str(headers, RETRY_AFTER_MS_HEADER) {
            if let Some(delay) = parse_ms(raw) {
                return Some(delay);
            }
        }
        if let Some(raw) = header_str(headers, RETRY_AFTER_HEADER) {
            if let Ok(seconds) = raw.trim().parse::<f64>() {
                if seconds.is_finite() && seconds >= 0.0 {
                    // An unrepresentably large value is treated as absent.
                    return seconds_to_duration(seconds);
                }
                // Negative or non-finite: absent.
                return None;
            }
            // Try an HTTP date.
            if let Ok(date) = httpdate::parse_http_date(raw.trim()) {
                if let Ok(now) = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH) {
                    let date = date
                        .duration_since(SystemTime::UNIX_EPOCH)
                        .unwrap_or_default();
                    let delay = date.saturating_sub(now);
                    return Some(delay);
                }
            }
        }
        None
    }

    /// The delay for retry `retry_number` (0-based), following the last error.
    ///
    /// If a retry-after is respected (enabled, parseable, and ≤
    /// [`Self::max_retry_after`]), it is used exactly. Otherwise capped
    /// exponential backoff: `min(initial * 2^n, max) * (1 - rand[0,1) * jitter)`.
    /// If initial or max is zero, the backoff delay is zero.
    pub(super) fn delay_for_retry(
        &self,
        retry_number: u32,
        api_error: Option<&ApiError>,
    ) -> Duration {
        if self.respect_retry_after {
            if let Some(api_error) = api_error {
                if let Some(delay) = api_error.retry_after {
                    if delay <= self.max_retry_after {
                        return delay;
                    }
                }
            }
        }
        self.backoff_delay(retry_number, fastrand::f64())
    }

    /// The pure backoff portion of the delay for retry `retry_number`
    /// (0-based), given a jitter random value in `[0, 1)`.
    pub fn backoff_delay(&self, retry_number: u32, random: f64) -> Duration {
        if self.backoff_initial.is_zero() || self.backoff_max.is_zero() {
            return Duration::ZERO;
        }
        let exponential = self
            .backoff_initial
            .saturating_mul(1u32 << retry_number.min(31))
            .min(self.backoff_max);
        let jittered = 1.0 - random * self.backoff_jitter;
        let scaled = exponential.as_secs_f64() * jittered;
        // Saturate instead of panicking if a huge configured backoff_max
        // rounds past Duration::MAX.
        Duration::try_from_secs_f64(scaled.max(0.0)).unwrap_or(Duration::MAX)
    }
}

fn header_str<'a>(headers: &'a reqwest::header::HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name)?.to_str().ok()
}

fn parse_ms(raw: &str) -> Option<Duration> {
    let value: f64 = raw.trim().parse().ok()?;
    if value.is_finite() && value >= 0.0 {
        // `try_` because the header is server-controlled: an overflowing
        // value must be "absent", not a panic.
        Duration::try_from_secs_f64(value / 1000.0).ok()
    } else {
        None
    }
}

fn seconds_to_duration(seconds: f64) -> Option<Duration> {
    Duration::try_from_secs_f64(seconds.max(0.0)).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::{HeaderMap, HeaderName, HeaderValue};

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(
                HeaderName::from_bytes(name.as_bytes()).unwrap(),
                HeaderValue::from_str(value).unwrap(),
            );
        }
        map
    }

    fn policy() -> RetryPolicy {
        RetryPolicy {
            backoff_jitter: 0.0,
            ..Default::default()
        }
    }

    #[test]
    fn defaults_match_spec() {
        let p = RetryPolicy::default();
        assert_eq!(p.max_retries, 2);
        assert_eq!(p.backoff_initial, Duration::from_millis(500));
        assert_eq!(p.backoff_max, Duration::from_secs(5));
        assert_eq!(p.backoff_jitter, 0.25);
        assert_eq!(
            p.http_statuses,
            [408u16, 429]
                .into_iter()
                .chain(500u16..=599)
                .collect::<BTreeSet<u16>>()
        );
        for status in 500..=599 {
            assert!(p.http_statuses.contains(&status));
        }
        assert!(!p.http_statuses.contains(&400));
        assert!(!p.http_statuses.contains(&600));
        assert!(p.respect_retry_after);
        assert_eq!(p.max_retry_after, Duration::from_secs(60));
        assert!(p.retry_connection_errors);
        assert!(p.retry_timeouts);
        assert_eq!(p.total_budget, Some(Duration::from_secs(30)));
    }

    #[test]
    fn none_policy_disables_retries() {
        assert_eq!(RetryPolicy::none().max_retries, 0);
    }

    #[test]
    fn backoff_formula_jitter_zero() {
        let p = policy();
        assert_eq!(p.backoff_delay(0, 0.0), Duration::from_millis(500));
        assert_eq!(p.backoff_delay(1, 0.0), Duration::from_millis(1000));
        assert_eq!(p.backoff_delay(2, 0.0), Duration::from_millis(2000));
        assert_eq!(p.backoff_delay(3, 0.0), Duration::from_millis(4000));
        assert_eq!(p.backoff_delay(4, 0.0), Duration::from_millis(5000)); // capped
        assert_eq!(p.backoff_delay(9, 0.0), Duration::from_millis(5000));
    }

    #[test]
    fn backoff_formula_jitter_bounds() {
        let p = RetryPolicy::default(); // jitter 0.25
                                        // random ~ 1: delay = base * 0.75 (allow float rounding).
        let ms = p.backoff_delay(0, 0.999999).as_secs_f64() * 1000.0;
        assert!((374.0..=376.0).contains(&ms), "{ms}");
        // random = 0: delay = base.
        assert_eq!(p.backoff_delay(0, 0.0), Duration::from_millis(500));
        // Halfway: 500 * 0.875 = 437.5ms.
        let ms = p.backoff_delay(0, 0.5).as_secs_f64() * 1000.0;
        assert!((436.0..=439.0).contains(&ms), "{ms}");
        // Never above the exponential base or below base * (1 - jitter).
        for random in [0.0, 0.25, 0.5, 0.75, 0.9999] {
            let delay = p.backoff_delay(1, random);
            assert!(delay <= Duration::from_millis(1000), "{delay:?}");
            assert!(delay >= Duration::from_millis(749), "{delay:?}");
        }
    }

    #[test]
    fn backoff_zero_initial_or_max_is_zero() {
        let p = RetryPolicy {
            backoff_initial: Duration::ZERO,
            ..policy()
        };
        assert_eq!(p.backoff_delay(0, 0.0), Duration::ZERO);
        let p = RetryPolicy {
            backoff_max: Duration::ZERO,
            ..policy()
        };
        assert_eq!(p.backoff_delay(3, 0.0), Duration::ZERO);
    }

    #[test]
    fn parse_retry_after_ms() {
        let p = policy();
        assert_eq!(
            p.parse_retry_after(&headers(&[(RETRY_AFTER_MS_HEADER, "250")])),
            Some(Duration::from_millis(250))
        );
        assert_eq!(
            p.parse_retry_after(&headers(&[(RETRY_AFTER_MS_HEADER, "0")])),
            Some(Duration::ZERO)
        );
        assert_eq!(
            p.parse_retry_after(&headers(&[(RETRY_AFTER_MS_HEADER, "10.5")])),
            Some(Duration::from_nanos(10_500_000))
        );
        assert_eq!(
            p.parse_retry_after(&headers(&[(RETRY_AFTER_MS_HEADER, "nope")])),
            None
        );
        assert_eq!(
            p.parse_retry_after(&headers(&[(RETRY_AFTER_MS_HEADER, "-5")])),
            None
        );
    }

    #[test]
    fn parse_retry_after_seconds() {
        let p = policy();
        assert_eq!(
            p.parse_retry_after(&headers(&[(RETRY_AFTER_HEADER, "3")])),
            Some(Duration::from_secs(3))
        );
        assert_eq!(
            p.parse_retry_after(&headers(&[(RETRY_AFTER_HEADER, "0")])),
            Some(Duration::ZERO)
        );
        assert_eq!(
            p.parse_retry_after(&headers(&[(RETRY_AFTER_HEADER, "1.5")])),
            Some(Duration::from_millis(1500))
        );
        assert_eq!(
            p.parse_retry_after(&headers(&[(RETRY_AFTER_HEADER, "-5")])),
            None
        );
        assert_eq!(
            p.parse_retry_after(&headers(&[(RETRY_AFTER_HEADER, "soon")])),
            None
        );
    }

    #[test]
    fn parse_retry_after_prefers_ms() {
        let p = policy();
        let h = headers(&[(RETRY_AFTER_MS_HEADER, "250"), (RETRY_AFTER_HEADER, "3")]);
        assert_eq!(p.parse_retry_after(&h), Some(Duration::from_millis(250)));
    }

    #[test]
    fn parse_retry_after_http_date() {
        let p = policy();
        let date = SystemTime::now() + Duration::from_secs(5);
        let date_str = httpdate::fmt_http_date(date);
        let parsed = p
            .parse_retry_after(&headers(&[(RETRY_AFTER_HEADER, date_str.as_str())]))
            .unwrap();
        assert!(parsed >= Duration::from_secs(4), "{parsed:?}");
        assert!(parsed <= Duration::from_secs(6), "{parsed:?}");

        // A date in the past clamps to zero.
        let past = httpdate::fmt_http_date(SystemTime::now() - Duration::from_secs(60));
        assert_eq!(
            p.parse_retry_after(&headers(&[(RETRY_AFTER_HEADER, past.as_str())])),
            Some(Duration::ZERO)
        );
    }

    #[test]
    fn delay_uses_retry_after_when_within_cap() {
        let p = policy();
        let error = retry_error(
            429,
            &[(RETRY_AFTER_MS_HEADER, "1500")],
            Duration::from_millis(1500),
        );
        assert_eq!(
            p.delay_for_retry(0, Some(&error)),
            Duration::from_millis(1500)
        );
    }

    #[test]
    fn delay_falls_back_to_backoff_above_cap() {
        let p = policy();
        let error = retry_error(429, &[(RETRY_AFTER_HEADER, "61")], Duration::from_secs(61));
        assert_eq!(
            p.delay_for_retry(0, Some(&error)),
            Duration::from_millis(500)
        );
        // Exactly at the cap is honored.
        let error = retry_error(429, &[(RETRY_AFTER_HEADER, "60")], Duration::from_secs(60));
        assert_eq!(p.delay_for_retry(0, Some(&error)), Duration::from_secs(60));
    }

    #[test]
    fn delay_ignores_retry_after_when_disabled() {
        let p = RetryPolicy {
            respect_retry_after: false,
            ..policy()
        };
        let error = retry_error(
            429,
            &[(RETRY_AFTER_MS_HEADER, "1500")],
            Duration::from_millis(1500),
        );
        assert_eq!(
            p.delay_for_retry(0, Some(&error)),
            Duration::from_millis(500)
        );
    }

    fn retry_error(status: u16, headers: &[(&str, &str)], retry_after: Duration) -> ApiError {
        ApiError {
            status,
            kind: crate::errors::ApiErrorKind::from_status(status),
            message: "msg".into(),
            body: None,
            headers: headers
                .iter()
                .map(|(name, value)| {
                    (
                        HeaderName::from_bytes(name.as_bytes()).unwrap(),
                        HeaderValue::from_str(value).unwrap(),
                    )
                })
                .collect(),
            request_id: None,
            endpoint: "POST https://api.typesafe.ai/v1/systemone".into(),
            retry_after: Some(retry_after),
        }
    }
}
