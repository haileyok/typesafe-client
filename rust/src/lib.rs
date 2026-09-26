//! Unofficial async Rust client for the TypeSafe AI System One API (Jev).
//!
//! System One answers *noul* (yes/no), *choice* (one-of-many), and *score*
//! (rubric rating) questions about arbitrary JSON content ("state") in a single
//! request, with calibrated probabilities and a confidence value you can gate
//! on. This crate is a thin, well-behaved async client for it.
//!
//! # Quick start
//!
//! ```no_run
//! # async fn run() -> Result<(), typesafe_client::Error> {
//! use typesafe_client::{Client, Noul, Choice, Score, SystemOneRequest};
//!
//! let client = Client::from_env()?;
//!
//! let response = client
//!     .system_one(
//!         SystemOneRequest::new("I was charged twice.")
//!             .question("billing", Noul::new("Is this about billing?"))
//!             .question(
//!                 "tone",
//!                 Choice::new("What is the tone?")
//!                     .option("calm", "A neutral or polite message")
//!                     .option("angry", "An upset or hostile message"),
//!             )
//!             .question(
//!                 "urgency",
//!                 Score::new("How urgent is this?", ["Can wait", "Needs attention today"]),
//!             ),
//!     )
//!     .await?;
//!
//! println!("model: {}", response.model);
//! println!("billing noul: {:?}", response.noul("billing").map(|a| a.noul));
//! if let Some(choice) = response.choice("tone") {
//!     if choice.confidence >= 0.7 {
//!         println!("tone: {}", choice.choice);
//!     }
//! }
//! # Ok(())
//! # }
//! ```
//!
//! The client is async only: every method returns a future, and there is no
//! blocking variant. Run it from an async runtime such as
//! [`tokio`](https://crates.io/crates/tokio).
//!
//! # Overview
//!
//! - [`Client`] — construct with [`Client::builder`] or [`Client::from_env`].
//! - [`SystemOneRequest`] — build a request with [`Noul`], [`Choice`], and
//!   [`Score`] questions.
//! - [`SystemOneResponse`] — typed answers plus usage and request metadata.
//! - [`RetryPolicy`] — retries, backoff, and retry-after handling.
//! - [`Error`] — everything that can go wrong.
//!
//! See `README.md` for a longer guide, and the repository's `SPEC.md` for the
//! authoritative behavior contract both clients in this repo implement.
//!
//! This crate is unofficial and not affiliated with TypeSafe.

#![deny(missing_docs)]
#![forbid(unsafe_code)]
#![doc = include_str!("../README.md")]

mod answers;
mod call_options;
mod client;
mod config;
mod errors;
mod logging;
mod questions;
mod request;
mod retry;

pub use answers::{
    Answer, ChoiceAnswer, ModelMetadata, NoulAnswer, ScoreAnswer, SystemOneResponse, Usage,
};
pub use client::{Client, ClientBuilder, RequestOptions};
pub use config::LogLevel;
pub use errors::{ApiError, ApiErrorKind, Error};
pub use questions::{Choice, Noul, NoulCriteria, Question, Score};
pub use request::SystemOneRequest;
pub use retry::RetryPolicy;

/// The base URL used when neither the builder nor `TYPESAFE_BASE_URL` set one.
pub const DEFAULT_BASE_URL: &str = "https://api.typesafe.ai";

/// The model used when neither the builder nor `TYPESAFE_DEFAULT_MODEL` set one.
pub const DEFAULT_MODEL: &str = "jev-latest";

/// The default per-attempt timeout (covers connect through reading the full body).
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

/// The `x-typesafe-request-id` response header, exposed on responses and errors.
pub const REQUEST_ID_HEADER: &str = "x-typesafe-request-id";

use std::time::Duration;
