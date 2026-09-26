# typesafe-system-one

Unofficial **async** Rust client for the [TypeSafe](https://typesafe.ai) AI
**System One** API (Jev).

> This crate is unofficial and not affiliated with TypeSafe.

System One answers *noul* (yes/no), *choice* (one-of-many), and *score*
(rubric rating) questions about arbitrary JSON content ("state") in a single
request, with calibrated probabilities and a confidence value you can gate on.
The behavior contract both clients in this repository implement is
[`SPEC.md`](https://github.com/haileyok/typesafe-client/blob/main/SPEC.md); this README is a guide to the Rust crate.

## Install

The client is async, so your program also needs an async runtime. It's built on
[tokio](https://crates.io/crates/tokio) (via reqwest), so add both:

```sh
cargo add typesafe-system-one
cargo add tokio --features macros,rt-multi-thread
```

which gives you, in `Cargo.toml`:

```toml
[dependencies]
typesafe-system-one = "0.1"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

If you build structured (JSON) instructions or state with the `json!` macro,
also `cargo add serde_json`.

In code, the crate is `typesafe_system_one`:

```rust
use typesafe_system_one::{Choice, Client, Noul, Score, SystemOneRequest};
```

You need an API key from the [TypeSafe console](https://console.typesafe.ai/keys).
`Client::from_env()` reads it from `TYPESAFE_API_KEY`, or pass it with
`Client::builder().api_key(...)`.

The minimum supported Rust version is **1.88**, set by the dependency tree.
There is no blocking client. (The crate lives in the
[`haileyok/typesafe-client`](https://github.com/haileyok/typesafe-client)
repository alongside a Go client. The `typesafe-client` name on crates.io
belongs to an unrelated project.)

The crate uses rustls for TLS. The `native-tls` cargo feature additionally
compiles in reqwest's system TLS backend. To use it, build your own
`reqwest::Client` configured for native TLS and pass it via
`ClientBuilder::http_client`:

```toml
typesafe-system-one = { version = "0.1", features = ["native-tls"] }
```

## Quick start

A complete program. Put it in `src/main.rs`:

```rust,no_run
use typesafe_system_one::{Choice, Client, Error, Noul, Score, SystemOneRequest};

#[tokio::main]
async fn main() -> Result<(), Error> {
    let client = Client::from_env()?; // reads TYPESAFE_API_KEY

    let response = client
        .system_one(
            SystemOneRequest::new("Help! My payouts have been failing for 3 days.")
                .question("is_urgent", Noul::new("Does this convey urgency?"))
                .question(
                    "department",
                    Choice::new("Which team should handle this?")
                        .option("billing", "Payments, invoicing, refunds")
                        .option("technical", "Bugs, outages, integrations")
                        .option("sales", "Pricing, upgrades, new accounts"),
                )
                .question(
                    "frustration",
                    Score::new(
                        "How frustrated is the customer?",
                        ["Calm", "Frustrated", "Very angry"],
                    ),
                ),
        )
        .await?;

    // A successful response always has an answer for every question asked,
    // so these lookups only fail on a typo in the question ID.
    let urgent = response.noul("is_urgent").expect("asked is_urgent");
    let department = response.choice("department").expect("asked department");
    let frustration = response.score("frustration").expect("asked frustration");

    println!("urgent:      P(yes) = {:.2}", urgent.noul);
    println!(
        "department:  {} (confidence {:.2})",
        department.choice, department.confidence
    );
    println!("frustration: {:.2} on a 0–2 scale", frustration.score);
    println!(
        "answered by {} (request {})",
        response.model,
        response.request_id.as_deref().unwrap_or("-")
    );
    Ok(())
}
```

Starting from an empty directory:

```sh
cargo new triage && cd triage
cargo add typesafe-system-one
cargo add tokio --features macros,rt-multi-thread
# replace src/main.rs with the program above
export TYPESAFE_API_KEY=...
cargo run
```

More complete programs are in [`examples/`](https://github.com/haileyok/typesafe-client/tree/main/rust/examples): `quickstart.rs`, and
`triage.rs`, which shows speculative fan-out and confidence-gated routing. The
API reference is on [docs.rs](https://docs.rs/typesafe-system-one).

## Configuration

Construct with [`Client::builder`](https://docs.rs/typesafe-system-one) or
`Client::from_env()`. Explicit options override environment variables, and
environment variables override defaults. Environment values are trimmed; a
blank value is ignored.

| Setting | Builder method | Env var | Default |
|---|---|---|---|
| API key (required) | `.api_key(k)` | `TYPESAFE_API_KEY` | none — construction fails |
| Base URL | `.base_url(u)` | `TYPESAFE_BASE_URL` | `https://api.typesafe.ai` (trailing `/` stripped) |
| Default model | `.default_model(m)` | `TYPESAFE_DEFAULT_MODEL` | `jev-latest` |
| Per-attempt timeout | `.timeout(d)` | — | 10s (covers connect through reading the full body) |
| Log level | `.log_level(l)` | `TYPESAFE_LOG_LEVEL` | off (`debug`, `info`, `warn`, `error`, `off`) |
| Retry policy | `.retry_policy(p)` | — | see [Retries](#retries) |

The API key is trimmed and validated at construction: empty keys, and keys
containing whitespace, control characters, or non-ASCII characters, are
rejected with a configuration error. The key never appears in `Debug` output
or in any error.

```rust
# use std::time::Duration;
# use typesafe_system_one::{Client, LogLevel, RetryPolicy};
let client = Client::builder()
    .api_key("sk-live-...")                      // else TYPESAFE_API_KEY
    .base_url("https://api.typesafe.ai")         // else TYPESAFE_BASE_URL
    .default_model("jev-1.13.0")                 // else TYPESAFE_DEFAULT_MODEL, else "jev-latest"
    .timeout(Duration::from_secs(10))            // per attempt; must be > 0
    .retry_policy(RetryPolicy { max_retries: 3, ..Default::default() })
    .default_header("X-Agent-Client", "my-app")  // gateway attribution
    .log_level(LogLevel::Info)                   // else TYPESAFE_LOG_LEVEL, else off
    .build()?;
# Ok::<(), typesafe_system_one::Error>(())
```

## Questions

A request carries a `state` (the content to evaluate — anything that
serializes to a JSON string, object, or array) and named questions. Question
IDs are the map keys: they are for your code only and are not sent to the
model; the response keys its answers by the same IDs.

```rust
# use typesafe_system_one::{Choice, Noul, Score, SystemOneRequest};
# use serde_json::json;
let request = SystemOneRequest::new("I was charged twice.")
    .model("jev-1.13.0") // optional per-request override
    .question("billing", Noul::new("Is this about billing?"))
    .question(
        "urgent",
        Noul::new("Is this urgent?")
            // optional yes/no criteria; None omits that side
            .criteria(Some("Time-sensitive"), None::<&str>),
    )
    .question(
        "tone",
        Choice::new("What is the tone?")
            .option("calm", "Neutral or polite")
            .option("angry", "Upset or hostile")
            .bare_option("excited"), // null description: interpreted by name
    )
    .question(
        "dept",
        Choice::from_options("Which team?", ["billing", "tech"]), // all-null descriptions
    )
    .question(
        "frustration",
        Score::new("How frustrated?", ["Calm", "Frustrated", "Very angry"]),
    );
# let _ = request;
```

### Ordered choice options

Choice options serialize **in the order you supply them**; `Choice::option`
appends, and `Choice::from_options` / `Choice::from_pairs` preserve iteration
order.

### Advanced structure

`instructions` and criteria accept any `impl Into<serde_json::Value>`, so
structured instructions work:

```rust
# use typesafe_system_one::{Noul, Score, SystemOneRequest};
# use serde_json::json;
let request = SystemOneRequest::new(json!({
    "subject": "Charged twice",
    "body": "Please refund the duplicate charge.",
}))
.question("billing", Noul::new(json!({
    "task": "Identify billing problems.",
    "include": ["duplicates", "refunds"],
})))
.question(
    "urgency",
    Score::new(json!({"task": "Rate urgency.", "scale": "calendar"}), [
        json!({"label": "Low", "hint": "no deadline"}),
        json!({"label": "High", "hint": "today"}),
    ]),
);
# let _ = request;
```

Any `Serialize` type works as state via
`SystemOneRequest::with_state_serialize(&my_struct)?`.

### Client-side validation

Before any network I/O, the client rejects: an empty question set; a choice
with no options; a score with fewer than two levels or any null level; and a
state that is not a JSON string, object, or array (numbers, booleans, and
null are rejected). Each failure is an `Error::InvalidRequest` naming the
offending question. Upper limits (255 choice options, 10 score levels) are
enforced server-side with a 422 and are not checked client-side.

## Reading answers

Answers are keyed by your question IDs. `response.noul(id)`, `.choice(id)`,
and `.score(id)` return typed answers; `response.nouls()`, `.choices()`, and
`.scores()` iterate all of a kind. An answer whose `type` this client doesn't
know is kept losslessly as an `Answer::Unknown { kind, raw }` rather than
dropped or errored — forward compatibility for when the API adds kinds.

Score `legend` and `probabilities` use decimal level-index keys on the wire;
the client exposes them as integer keys (`BTreeMap<u32, _>`).

### Confidence gating

Confidence (0 to 1) tells you when to route on an answer and when to fall
back to a human. A common pattern:

```rust,no_run
# use typesafe_system_one::{Choice, Client, SystemOneRequest};
# async fn demo(client: Client) -> Result<(), typesafe_system_one::Error> {
let response = client
    .system_one(
        SystemOneRequest::new("I was charged twice.").question(
            "tone",
            Choice::new("What is the tone?")
                .option("calm", "Neutral or polite")
                .option("angry", "Upset or hostile"),
        ),
    )
    .await?;

let route = match response.choice("tone") {
    Some(choice) if choice.confidence >= 0.7 => choice.choice.clone(),
    _ => "human-triage".to_owned(), // low confidence: don't trust it
};
# let _ = route;
# Ok(())
# }
```

## Errors

`Error` is `#[non_exhaustive]`:

- `Config` — missing or invalid configuration (never echoes the API key).
- `InvalidRequest` — client-side validation failed.
- `Api` — a non-2xx response. `ApiError` carries the status, kind
  (`ApiErrorKind`), extracted message, parsed body, headers, request ID,
  endpoint, and parsed retry-after.
- `Connection` — no HTTP response (DNS, TLS, reset, body read failure).
- `Timeout` — the attempt exceeded the per-attempt timeout (a kind of
  connection error; `is_connection()` returns true for it).
- `ResponseValidation` — a 2xx body didn't match the schema, with a dotted
  field path like `answers.tone.confidence` and the HTTP status. This also
  covers a response that omits an answer for a question you asked
  (`answers.<id>`) or answers it with the wrong type (`answers.<id>.type`). A
  successful response therefore always has an answer for every question, so
  `resp.noul("id")` returning `None` never silently means "no".

Helpers: `is_timeout()`, `is_connection()`, `status()`, `request_id()`,
`as_api()`.

The API error `Display` format is exactly
`"<METHOD> <URL>: <status> <message> (request_id=<id>)"`.

The message is extracted from the body (first match wins): non-empty string
body (truncated to 200 characters + `…`), `error` (string), `error.message`,
`message`, `detail` (string), `detail.message`, or `detail[]` as FastAPI
validation errors (`"questions.urgency.score.criteria: Field required"`).
Otherwise the raw body truncated to 200 characters + `…`; an empty body gives
`"status code (no body)"`. The API key never appears in any error.

```rust,no_run
# use typesafe_system_one::Error;
# fn demo(error: Error) {
match &error {
    Error::Api(api) if api.kind == typesafe_system_one::ApiErrorKind::RateLimit => {
        eprintln!("rate limited; retry after {:?}", api.retry_after);
    }
    Error::Timeout { timeout } => eprintln!("timed out after {timeout:?}"),
    _ => eprintln!("{error}"),
}
# }
```

## Retries

The default policy matches both official SDKs: 2 retries after the first
attempt, 500ms initial backoff doubling to a 5s cap with 25% jitter, retries
on 408/429/5xx (and connection and timeout errors), honored
`retry-after-ms`/`Retry-After` capped at 60s, and a 30s total budget.

Every field is public on `RetryPolicy`. A per-call policy fully replaces the
client's policy for that call:

```rust,no_run
# use std::time::Duration;
# use typesafe_system_one::{Client, RequestOptions, RetryPolicy, SystemOneRequest, Noul};
# async fn demo(client: Client) -> Result<(), typesafe_system_one::Error> {
let response = client
    .system_one_with(
        SystemOneRequest::new("text").question("a", Noul::new("q")),
        RequestOptions::new()
            .timeout(Duration::from_secs(30))
            .retry_policy(RetryPolicy { max_retries: 5, ..Default::default() }),
    )
    .await?;
# let _ = response;
# Ok(())
# }
```

- **Delay for retry n (0-based):** an honored retry-after is used exactly;
  otherwise `min(initial * 2^n, max) * (1 - rand[0,1) * jitter)`. If initial
  or max is 0, the delay is 0.
- **Stop conditions:** retries exhausted; the error isn't retryable; the next
  delay would reach or exceed the remaining total budget — in which case the
  **last real error** is returned (you see the 529, not an artificial
  timeout).
- A retry sets the `X-TypeSafe-Retry-Count` header (n ≥ 1).

## Timeouts and cancellation

The timeout (default 10s) is **per attempt** and covers connecting through
reading the full response body. Timeouts are retried by default like
connection errors; exhaustion surfaces as `Error::Timeout`.

Cancellation is native Rust: **drop the future**. Nothing runs after the
future is dropped, and no cancellation error is synthesized.

## Gateways

Any base URL implementing the TypeSafe OpenAPI spec works. Add attribution
headers with `.default_header(...)` (SDK-owned headers always win):

```rust,no_run
# use typesafe_system_one::Client;
# fn demo() -> Result<(), typesafe_system_one::Error> {
// OpenRouter
let openrouter = Client::builder()
    .api_key("sk-or-...")
    .base_url("https://openrouter.ai/api")
    .default_model("~typesafe/jev-latest")
    .default_header("X-Agent-Client", "my-app")
    .build()?;
# let _ = openrouter;
# Ok(())
# }
```

Vercel AI Gateway works the same way with
`https://ai-gateway.vercel.sh/typesafe` and model `typesafe-ai/jev`.

## Logging

Off unless configured (`LogLevel` or `TYPESAFE_LOG_LEVEL`).

- **info**: one line per attempt result (method, path, status, duration,
  request ID) and per scheduled retry (delay and reason).
- **debug**: also request/response headers and bodies.

`Authorization`, `Proxy-Authorization`, `X-Api-Key`, `Api-Key`, `Cookie`, and
`Set-Cookie` headers are redacted to `***`. **Bodies are not redacted** —
configure `debug` accordingly. Logs are emitted through the
[`tracing`](https://crates.io/crates/tracing) crate; install a subscriber
(like `tracing-subscriber`) to see them.

## Environment variables

| Variable | Meaning |
|---|---|
| `TYPESAFE_API_KEY` | API key (required if not passed to the builder) |
| `TYPESAFE_BASE_URL` | Base URL override |
| `TYPESAFE_DEFAULT_MODEL` | Default model override |
| `TYPESAFE_LOG_LEVEL` | `debug` / `info` / `warn` / `error` / `off` |

## Examples

- `examples/quickstart.rs` — one of each question kind.
- `examples/triage.rs` — speculative fan-out: choice + score + noul in one
  request, then route in code.

## License

MIT.
