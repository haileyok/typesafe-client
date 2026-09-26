//! Logging helpers: redaction and formatted lines.
//!
//! Logging is off unless configured. At `info`, one line per attempt result
//! (method, path, status, duration, request ID) and per scheduled retry
//! (delay and reason). At `debug`, headers and bodies too.
//!
//! The `Authorization`, `Proxy-Authorization`, `X-Api-Key`, `Api-Key`,
//! `Cookie`, and `Set-Cookie` headers are redacted to `***` before logging.
//! Bodies are **not** redacted; document this in any integration.

use std::time::Duration;

/// Headers redacted from logs.
const REDACTED_HEADERS: [&str; 6] = [
    "authorization",
    "proxy-authorization",
    "x-api-key",
    "api-key",
    "cookie",
    "set-cookie",
];

/// Returns `true` when a header name must be redacted.
fn is_redacted(name: &str) -> bool {
    REDACTED_HEADERS.contains(&name.to_ascii_lowercase().as_str())
}

/// Redacts credential-bearing headers from a displayable set.
///
/// The set stays usable with `tracing::debug!` field values.
pub(crate) fn redact_headers(headers: &reqwest::header::HeaderMap) -> Vec<(String, String)> {
    headers
        .iter()
        .map(|(name, value)| {
            let name = name.as_str().to_owned();
            let value = if is_redacted(&name) || value_is_secret(&name) {
                "***".to_owned()
            } else {
                String::from_utf8_lossy(value.as_bytes()).into_owned()
            };
            (name, value)
        })
        .collect()
}

/// Extra defensive check: header names carrying tokens or secrets.
fn value_is_secret(name: &str) -> bool {
    let lowered = name.to_ascii_lowercase();
    lowered.contains("token") || lowered.contains("secret")
}

/// Logs one line for a completed attempt at `info` level.
pub(crate) fn log_attempt_info(
    method: &str,
    path: &str,
    status: u16,
    duration: Duration,
    request_id: Option<&str>,
) {
    let duration_ms = duration.as_millis();
    match request_id {
        Some(request_id) => tracing::info!(
            method,
            path,
            status,
            duration_ms,
            request_id,
            "attempt finished"
        ),
        None => tracing::info!(method, path, status, duration_ms, "attempt finished"),
    }
}

/// Logs one line for a failed transport attempt at `info` level.
pub(crate) fn log_attempt_error(method: &str, path: &str, message: &str) {
    tracing::info!(method, path, error = %message, "attempt failed without a response");
}

/// Logs a scheduled retry at `info` level.
pub(crate) fn log_retry_scheduled(
    method: &str,
    path: &str,
    delay: Duration,
    attempt: u32,
    reason: &str,
) {
    let delay_ms = delay.as_millis();
    tracing::info!(method, path, attempt, delay_ms, reason, "retrying");
}

/// Logs request headers and body at `debug` level with redaction applied.
pub(crate) fn log_request_debug(
    method: &str,
    path: &str,
    headers: &reqwest::header::HeaderMap,
    body: Option<&serde_json::Value>,
) {
    let headers = redact_headers(headers);
    match body {
        Some(body) => tracing::debug!(method, path, ?headers, %body, "request"),
        None => tracing::debug!(method, path, ?headers, "request"),
    }
}

/// Logs response status, headers, and body at `debug` level with redaction
/// applied.
pub(crate) fn log_response_debug(
    method: &str,
    path: &str,
    status: u16,
    headers: &reqwest::header::HeaderMap,
    body: &str,
) {
    let headers = redact_headers(headers);
    tracing::debug!(method, path, status, ?headers, body, "response");
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::{HeaderMap, HeaderName, HeaderValue};

    #[test]
    fn redacts_secret_headers() {
        let mut headers = HeaderMap::new();
        for (name, value) in [
            ("Authorization", "Bearer sk-secret"),
            ("Proxy-Authorization", "Basic xyz"),
            ("X-Api-Key", "sk-secret"),
            ("Api-Key", "sk-secret"),
            ("Cookie", "session=1"),
            ("Set-Cookie", "session=1"),
            ("X-Token-Thing", "abc"),
            ("X-Kinda-Secret", "abc"),
            ("User-Agent", "typesafe-client-rust/0.1.0"),
            ("Accept", "application/json"),
        ] {
            headers.insert(
                HeaderName::from_bytes(name.as_bytes()).unwrap(),
                HeaderValue::from_str(value).unwrap(),
            );
        }
        let redacted = redact_headers(&headers);
        let get = |name: &str| {
            redacted
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.as_str())
                .unwrap()
        };
        assert_eq!(get("authorization"), "***");
        assert_eq!(get("proxy-authorization"), "***");
        assert_eq!(get("x-api-key"), "***");
        assert_eq!(get("api-key"), "***");
        assert_eq!(get("cookie"), "***");
        assert_eq!(get("set-cookie"), "***");
        assert_eq!(get("x-token-thing"), "***");
        assert_eq!(get("x-kinda-secret"), "***");
        assert_eq!(get("user-agent"), "typesafe-client-rust/0.1.0");
        assert_eq!(get("accept"), "application/json");
    }
}
