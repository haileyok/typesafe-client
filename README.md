# typesafe-client

Unofficial **Go** and **Rust** clients for the [TypeSafe AI](https://typesafe.ai)
System One API. System One models, including TypeSafe's flagship model **Jev**,
evaluate text or JSON *state* against named, typed questions and return structured
answers instead of generated text:

| Question | Answers | Returns |
|---|---|---|
| **Noul** | Is this true? | probability of yes |
| **Choice** | Which of these options? | chosen option, a probability per option, confidence |
| **Score** | Which level on this scale? | probability-weighted level, a probability per level, confidence |

| Language | Directory | Install |
|---|---|---|
| Go (1.22+, stdlib only) | [`go/`](go) | `go get github.com/haileyok/typesafe-client/go` |
| Rust (async, reqwest, 1.88+) | [`rust/`](rust) | `cargo add typesafe-system-one` ([crates.io](https://crates.io/crates/typesafe-system-one), [docs.rs](https://docs.rs/typesafe-system-one)) |

TypeSafe publishes official SDKs for [Python](https://github.com/typesafe-ai/typesafe-sdk-python)
and [JavaScript/TypeScript](https://github.com/typesafe-ai/typesafe-sdk-js). These
clients bring the same behavior to Go and Rust:

- Typed questions and answers. Structured (JSON) instructions and criteria work,
  and Choice options keep the order you give them
- Configuration from `TYPESAFE_API_KEY`, `TYPESAFE_BASE_URL`, `TYPESAFE_DEFAULT_MODEL`,
  and `TYPESAFE_LOG_LEVEL`
- The official SDKs' retry policy: 408/429/5xx (including 529 Overloaded), connection
  failures, and timeouts, retried with exponential backoff and jitter. `Retry-After`
  is honored, and a total budget returns the last *real* error rather than a synthetic
  timeout
- Typed errors per status, with server messages extracted (including FastAPI 422 field
  paths) and the `x-typesafe-request-id` attached
- Response validation with dotted field paths. Future answer types are passed
  through instead of breaking the response
- `GET /v1/models`, gateway support (OpenRouter, Vercel AI Gateway, your own proxy), and
  logging with credentials redacted

## Go

```go
client, err := typesafe.NewClient() // reads TYPESAFE_API_KEY
resp, err := client.SystemOne(ctx, typesafe.Request{
	State: "Help! My payouts have been failing for 3 days.",
	Questions: typesafe.Questions{
		"is_urgent":  typesafe.Noul("Does this convey urgency?"),
		"department": typesafe.ChoiceNames("Which team should handle this?", "billing", "technical", "sales"),
		"frustration": typesafe.Score("How frustrated is the customer?", "Calm", "Frustrated", "Very angry"),
	},
})
dept, _ := resp.Choice("department")
fmt.Println(dept.Choice, dept.Confidence)
```

## Rust

```rust
let client = Client::from_env()?; // reads TYPESAFE_API_KEY
let resp = client
    .system_one(
        SystemOneRequest::new("Help! My payouts have been failing for 3 days.")
            .question("is_urgent", Noul::new("Does this convey urgency?"))
            .question("department", Choice::from_options("Which team should handle this?", ["billing", "technical", "sales"]))
            .question("frustration", Score::new("How frustrated is the customer?", ["Calm", "Frustrated", "Very angry"])),
    )
    .await?;
let dept = resp.choice("department").unwrap();
println!("{} {}", dept.choice, dept.confidence);
```

## Behavior contract

[`SPEC.md`](SPEC.md) defines what both clients do: configuration, headers,
validation, decoding, errors, retries, and logging. It also records how
disagreements between the docs, the OpenAPI spec, and the official SDKs were
resolved. [`spec/openapi.json`](spec/openapi.json) is a snapshot of TypeSafe's
published OpenAPI document (API v0.2.0) that the clients are tested against.

Tips for using the API well, from TypeSafe's docs:

- **Ask every question about a state in one request.** Questions run in parallel and
  independently, so extra (even speculative) questions barely change latency.
- **Question IDs aren't sent to the model.** Write the full question in the instructions.
- **Use `confidence` to decide when to act**, and tune thresholds on your own data.
  Pin a versioned model (e.g. `jev-1.13.0`) once you do, because aliases move.
- **Keep math, counting, and date arithmetic in code.** See
  [Jev jaggedness](https://docs.typesafe.ai/model-jaggedness/jev-1.13).

## Releases

Each client is versioned independently: `go/vX.Y.Z` tags release the Go
module and `rust/vX.Y.Z` tags release the crate. See [RELEASING.md](RELEASING.md).

## Status

These are unofficial clients, not affiliated with or endorsed by TypeSafe AI.
Released under the [MIT License](LICENSE).
