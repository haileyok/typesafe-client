//! Internal module: keeps a single definition of the fields below so the
//! builder/`Client`/`RequestOptions` stay consistent without a public trait.
//!
//! (In a future refactor these may become public; for now they are private
//! plumbing, not part of the crate's API surface.)

use std::collections::BTreeMap;
use std::time::Duration;

/// Per-attempt and per-call knobs shared by the client and per-call options.
#[derive(Debug, Clone, Default)]
pub(crate) struct CallOptions {
    /// Per-attempt timeout. Must be > 0.
    pub timeout: Option<Duration>,
    /// Per-call retry policy that fully replaces the client's policy.
    pub retry_policy: Option<RetryPolicy>,
    /// Per-call headers that outrank client defaults but not SDK-owned headers.
    pub headers: BTreeMap<String, String>,
    /// Per-call extra body fields to merge into the JSON body.
    pub extra_body: Option<serde_json::Map<String, serde_json::Value>>,
}

/// The full, resolved runtime configuration of a call.
#[derive(Debug, Clone)]
pub(crate) struct ResolvedCall {
    /// Resolved per-attempt timeout.
    pub timeout: Duration,
    /// The retry policy that governs this call.
    pub retry_policy: RetryPolicy,
    /// Headers to set on every attempt (client defaults ⊕ per-call).
    pub headers: BTreeMap<String, String>,
    /// Extra body fields merged into the JSON body.
    pub extra_body: Option<serde_json::Map<String, serde_json::Value>>,
}

pub(crate) use crate::retry::RetryPolicy;
