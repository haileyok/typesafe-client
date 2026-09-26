//! Environment-variable configuration tests.
//!
//! Environment variables are process-global, so every test that mutates them
//! serializes on a shared mutex.

use std::sync::{Mutex, MutexGuard, OnceLock};

use typesafe_client::{Client, LogLevel};

/// The global lock serializing all env-mutating tests.
static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

/// Acquires the env lock for the duration of a test.
fn lock_env() -> MutexGuard<'static, ()> {
    ENV_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .expect("env lock poisoned")
}

/// Runs a closure with env vars set, restoring them afterwards.
fn with_env(vars: &[(&str, Option<&str>)], f: impl FnOnce()) {
    let _guard = lock_env();
    let mut saved: Vec<(&str, Option<std::ffi::OsString>)> = Vec::new();
    for (name, value) in vars {
        saved.push((name, std::env::var_os(name)));
        match value {
            Some(value) => std::env::set_var(name, value),
            None => std::env::remove_var(name),
        }
    }
    f();
    for (name, value) in saved {
        match value {
            Some(value) => std::env::set_var(name, value),
            None => std::env::remove_var(name),
        }
    }
}

#[test]
fn env_supplies_api_key() {
    with_env(&[(env_key(), Some("sk-env-key"))], || {
        let client = Client::from_env().unwrap();
        assert_eq!(client.base_url(), "https://api.typesafe.ai");
        assert_eq!(client.default_model(), "jev-latest");
        assert_eq!(client.timeout(), std::time::Duration::from_secs(10));
        assert_eq!(client.log_level(), LogLevel::Off);
    });
}

#[test]
fn explicit_overrides_env() {
    with_env(
        &[
            (env_key(), Some("sk-env-key")),
            (env_base(), Some("https://env.example.com/")),
            (env_model(), Some("jev-env")),
            (env_log(), Some("warn")),
        ],
        || {
            let client = Client::builder()
                .api_key("sk-explicit")
                .base_url("https://explicit.example.com/")
                .default_model("jev-explicit")
                .log_level(LogLevel::Debug)
                .build()
                .unwrap();
            assert_eq!(client.base_url(), "https://explicit.example.com");
            assert_eq!(client.default_model(), "jev-explicit");
            assert_eq!(client.log_level(), LogLevel::Debug);
        },
    );
}

#[test]
fn env_overrides_defaults() {
    with_env(
        &[
            (env_base(), Some("https://env.example.com")),
            (env_model(), Some("jev-env")),
            (env_log(), Some("info")),
        ],
        || {
            let client = Client::builder().api_key("sk").build().unwrap();
            assert_eq!(client.base_url(), "https://env.example.com");
            assert_eq!(client.default_model(), "jev-env");
            assert_eq!(client.log_level(), LogLevel::Info);
        },
    );
}

#[test]
fn blank_env_is_ignored() {
    with_env(
        &[
            (env_key(), Some("   ")),
            (env_base(), Some("")),
            (env_model(), Some("\t")),
            (env_log(), Some("  ")),
        ],
        || {
            let error = Client::from_env().unwrap_err();
            assert!(error.to_string().contains("No API key"), "{error}");
            let client = Client::builder().api_key("sk").build().unwrap();
            assert_eq!(client.base_url(), "https://api.typesafe.ai");
            assert_eq!(client.default_model(), "jev-latest");
            assert_eq!(client.log_level(), LogLevel::Off);
        },
    );
}

#[test]
fn env_values_are_trimmed() {
    with_env(
        &[
            (env_key(), Some("  sk-trimmed  ")),
            (env_base(), Some("  https://trim.example.com  ")),
        ],
        || {
            let client = Client::from_env().unwrap();
            assert_eq!(client.base_url(), "https://trim.example.com");
        },
    );
}

#[test]
fn invalid_env_key_rejected() {
    with_env(&[(env_key(), Some("sk with space"))], || {
        let error = Client::from_env().unwrap_err();
        assert!(error.to_string().contains("API key"), "{error}");
        // The key must not appear in the error.
        assert!(!error.to_string().contains("sk with space"), "{error}");
    });
}

#[test]
fn invalid_log_level_falls_back_to_off() {
    with_env(&[(env_log(), Some("yelling"))], || {
        let client = Client::builder().api_key("sk").build().unwrap();
        assert_eq!(client.log_level(), LogLevel::Off);
    });
}

#[test]
fn debug_shows_no_api_key() {
    let client = Client::builder()
        .api_key("sk-super-secret")
        .build()
        .unwrap();
    let debug = format!("{client:?}");
    assert!(!debug.contains("sk-super-secret"), "{debug}");
    let builder = typesafe_client::Client::builder().api_key("sk-super-secret");
    let debug = format!("{builder:?}");
    assert!(!debug.contains("sk-super-secret"), "{debug}");
}

#[test]
fn bad_settings_are_config_errors() {
    // Missing key entirely (with a clean env).
    with_env(&[(env_key(), None)], || {
        let error = Client::from_env().unwrap_err();
        assert!(matches!(error, typesafe_client::Error::Config(_)));
    });
    // Zero timeout.
    let error = Client::builder()
        .api_key("sk")
        .timeout(std::time::Duration::ZERO)
        .build()
        .unwrap_err();
    assert!(matches!(error, typesafe_client::Error::Config(_)));
    // Jitter out of range.
    let error = Client::builder()
        .api_key("sk")
        .retry_policy(typesafe_client::RetryPolicy {
            backoff_jitter: 1.5,
            ..Default::default()
        })
        .build()
        .unwrap_err();
    assert!(matches!(error, typesafe_client::Error::Config(_)));
}

// Distinct env-var names per test body would be nice, but the real names are
// what the client reads; the shared lock keeps them safe.
fn env_key() -> &'static str {
    "TYPESAFE_API_KEY"
}
fn env_base() -> &'static str {
    "TYPESAFE_BASE_URL"
}
fn env_model() -> &'static str {
    "TYPESAFE_DEFAULT_MODEL"
}
fn env_log() -> &'static str {
    "TYPESAFE_LOG_LEVEL"
}
