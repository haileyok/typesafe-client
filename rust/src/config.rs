//! Configuration resolution: environment variables, defaults, and validation.
//!
//! Explicit options override environment variables, and environment
//! variables override defaults. Environment values are trimmed, and an empty
//! or whitespace-only value is ignored.

use std::fmt;
use std::time::Duration;

use crate::errors::Error;

/// The environment variable for the API key.
pub(crate) const API_KEY_ENV: &str = "TYPESAFE_API_KEY";
/// The environment variable for the base URL.
pub(crate) const BASE_URL_ENV: &str = "TYPESAFE_BASE_URL";
/// The environment variable for the default model.
pub(crate) const DEFAULT_MODEL_ENV: &str = "TYPESAFE_DEFAULT_MODEL";
/// The environment variable for the log level.
pub(crate) const LOG_LEVEL_ENV: &str = "TYPESAFE_LOG_LEVEL";

/// The logging verbosity of the client.
///
/// Logging is off unless configured. At [`LogLevel::Info`], the client logs
/// one line per attempt result and per scheduled retry. At
/// [`LogLevel::Debug`], it also logs headers and bodies. Credential headers
/// are redacted; bodies are not.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    /// Request/response headers and bodies, attempt lines, and retries.
    Debug,
    /// One line per attempt result and per scheduled retry.
    Info,
    /// Only warnings and errors.
    Warn,
    /// Only errors.
    Error,
    /// No logging. The default.
    #[default]
    Off,
}

impl LogLevel {
    /// Parses a log level name, case-insensitively, ignoring surrounding
    /// whitespace. `warning` is accepted as an alias for `warn`.
    pub fn parse(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "debug" => Some(Self::Debug),
            "info" => Some(Self::Info),
            "warn" | "warning" => Some(Self::Warn),
            "error" => Some(Self::Error),
            "off" => Some(Self::Off),
            _ => None,
        }
    }

    /// Resolves the level from an explicit setting or `TYPESAFE_LOG_LEVEL`.
    pub(crate) fn resolve(explicit: Option<Self>) -> Self {
        if let Some(level) = explicit {
            return level;
        }
        env_trimmed(LOG_LEVEL_ENV)
            .and_then(|value| Self::parse(&value))
            .unwrap_or_default()
    }
}

impl fmt::Display for LogLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::Debug => "debug",
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
            Self::Off => "off",
        };
        f.write_str(name)
    }
}

/// Reads an environment variable, trimmed; `None` when unset or blank.
pub(crate) fn env_trimmed(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

/// Resolves a setting: explicit value, else environment, else `default`.
fn resolve_string(value: Option<&str>, env: &str, default: &str) -> String {
    if let Some(value) = value {
        return value.trim().to_owned();
    }
    env_trimmed(env).unwrap_or_else(|| default.to_owned())
}

/// Resolves and validates the API key: explicit value or environment, trimmed.
///
/// Rejects empty keys and keys containing whitespace, control characters, or
/// non-ASCII characters. The error never echoes the key.
pub(crate) fn resolve_api_key(api_key: Option<&str>) -> Result<String, Error> {
    let key = api_key
        .map(str::trim)
        .map(str::to_owned)
        .or_else(|| env_trimmed(API_KEY_ENV))
        .unwrap_or_default();
    if key.is_empty() {
        return Err(Error::Config(format!(
            "No API key was provided. Pass api_key or set the {API_KEY_ENV} environment variable."
        )));
    }
    if !key.is_ascii() {
        return Err(Error::Config(
            "API key must contain only printable ASCII characters without whitespace.".into(),
        ));
    }
    if key.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(Error::Config(
            "API key must contain only printable ASCII characters without whitespace.".into(),
        ));
    }
    if !key.chars().all(|c| {
        let c = c as u8;
        (0x21..=0x7e).contains(&c)
    }) {
        return Err(Error::Config(
            "API key must contain only printable ASCII characters without whitespace.".into(),
        ));
    }
    Ok(key)
}

/// Resolves the base URL, stripping a trailing `/`.
pub(crate) fn resolve_base_url(base_url: Option<&str>) -> String {
    resolve_string(base_url, BASE_URL_ENV, crate::DEFAULT_BASE_URL)
        .trim_end_matches('/')
        .to_owned()
}

/// Resolves the default model.
pub(crate) fn resolve_default_model(default_model: Option<&str>) -> String {
    resolve_string(default_model, DEFAULT_MODEL_ENV, crate::DEFAULT_MODEL)
}

/// Validates a per-attempt timeout: it must be greater than zero.
pub(crate) fn validate_timeout(timeout: Duration) -> Result<Duration, Error> {
    if timeout.is_zero() {
        return Err(Error::Config(
            "timeout must be a positive duration of time.".into(),
        ));
    }
    Ok(timeout)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Environment variables are process-global; these tests serialize on a
    // shared lock. The parent `tests::env_lock` module holds it across all
    // env-mutating tests (see tests/env.rs), and unit tests here use a local
    // copy of the same discipline by only reading well-known-unset variables.
    // Mutation-based tests live in tests/env.rs.

    #[test]
    fn parse_log_level() {
        assert_eq!(LogLevel::parse("debug"), Some(LogLevel::Debug));
        assert_eq!(LogLevel::parse("INFO"), Some(LogLevel::Info));
        assert_eq!(LogLevel::parse(" warn "), Some(LogLevel::Warn));
        assert_eq!(LogLevel::parse("warning"), Some(LogLevel::Warn));
        assert_eq!(LogLevel::parse("error"), Some(LogLevel::Error));
        assert_eq!(LogLevel::parse("off"), Some(LogLevel::Off));
        assert_eq!(LogLevel::parse("nope"), None);
        assert_eq!(LogLevel::parse(""), None);
    }

    #[test]
    fn default_log_level_is_off() {
        assert_eq!(LogLevel::default(), LogLevel::Off);
    }

    #[test]
    fn api_key_validation() {
        // Missing/empty.
        let err = resolve_api_key(None).unwrap_err();
        assert!(matches!(err, Error::Config(_)));
        assert!(err.to_string().contains("No API key"));
        // Whitespace inside.
        for bad in ["key with space", "key\ttab", "key\nnewline", "ünïcödé"] {
            let err = resolve_api_key(Some(bad)).unwrap_err();
            assert!(matches!(err, Error::Config(_)));
            assert!(!err.to_string().contains(bad), "leaked key: {err}");
        }
        // Trimmed valid key.
        assert_eq!(
            resolve_api_key(Some("  sk-live-abc123  ")).unwrap(),
            "sk-live-abc123"
        );
    }

    #[test]
    fn timeout_validation() {
        assert!(validate_timeout(Duration::from_millis(1)).is_ok());
        let err = validate_timeout(Duration::ZERO).unwrap_err();
        assert!(matches!(err, Error::Config(_)));
    }

    #[test]
    fn base_url_strips_trailing_slash() {
        assert_eq!(
            resolve_base_url(Some("https://example.com/")),
            "https://example.com"
        );
        assert_eq!(
            resolve_base_url(Some("https://example.com//")),
            "https://example.com"
        );
    }
}
