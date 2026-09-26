# typesafe-client/go

An unofficial Go client for the [TypeSafe AI](https://typesafe.ai) System One API
(Jev). It has no dependencies outside the standard library and needs Go 1.22 or later.

```sh
go get github.com/haileyok/typesafe-client/go
```

```go
import typesafe "github.com/haileyok/typesafe-client/go"
```

> Not affiliated with or endorsed by TypeSafe. The client follows the behavior of the
> official [Python](https://github.com/typesafe-ai/typesafe-sdk-python) and
> [JavaScript](https://github.com/typesafe-ai/typesafe-sdk-js) SDKs. See
> [SPEC.md](../SPEC.md) for the full contract.

## Quick start

```go
client, err := typesafe.NewClient() // reads TYPESAFE_API_KEY
if err != nil {
	log.Fatal(err)
}

resp, err := client.SystemOne(ctx, typesafe.Request{
	State: "Help! My payouts have been failing for 3 days.",
	Questions: typesafe.Questions{
		"is_urgent":   typesafe.Noul("Does this convey urgency?"),
		"department":  typesafe.ChoiceNames("Which team should handle this?", "billing", "technical", "sales"),
		"frustration": typesafe.Score("How frustrated is the customer?", "Calm", "Frustrated", "Very angry"),
	},
})
if err != nil {
	log.Fatal(err)
}

urgent, _ := resp.Noul("is_urgent")       // urgent.Noul: P(yes), 0..1
dept, _ := resp.Choice("department")      // dept.Choice, dept.Probabilities, dept.Confidence
mood, _ := resp.Score("frustration")      // mood.Score (0..2, may fall between levels), mood.Confidence
fmt.Println(resp.Model, resp.RequestID)   // "jev-1.13.0": the versioned model that answered
```

Runnable programs: [`examples/quickstart`](examples/quickstart) and
[`examples/triage`](examples/triage), which shows speculative fan-out and
confidence-gated routing.

## Questions

Put every question about one state in **one request**. Questions are evaluated in
parallel and independently, so extra questions cost little latency and only a few
tokens. Question IDs are not sent to the model, so write the complete question in
the instructions.

| Constructor | Wire type | Answer |
|---|---|---|
| `Noul(instr)` / `.WithCriteria(yes, no)` | `noul` | `NoulAnswer{Noul}`: probability of yes |
| `Choice(instr, Opt(name, desc), Bare(name), ...)` | `choice` | `ChoiceAnswer{Choice, Probabilities, Confidence}` |
| `ChoiceNames(instr, names...)` | `choice` | options described by name alone |
| `ChoiceMap(instr, map[string]V)` | `choice` | options sorted by name (Go maps are unordered) |
| `Score(instr, levels...)` | `score` | `ScoreAnswer{Score, Probabilities, Legend, Confidence}` |

`Choice` sends options **in the order you give them**.

Instructions, option descriptions, Score levels, and Noul criteria may be strings
**or structured values** (maps, structs, slices), as described in
[Advanced: structure](https://docs.typesafe.ai/primitives/advanced). Refer to parts of
an object state by backticked path:

```go
resp, err := client.SystemOne(ctx, typesafe.Request{
	State: map[string]any{"ticket": ticket, "refund_policy": policy},
	Questions: typesafe.Questions{
		"supported": typesafe.Noul("Does `refund_policy` support the refund requested in `ticket`?"),
		"team": typesafe.Choice("Which team should handle `ticket`?",
			typesafe.Opt("billing", map[string]any{
				"what":    "Charges, invoices, refunds",
				"not_for": "Order tracking or account access",
			}),
			typesafe.Opt("orders", "Order status, delivery, cancellation"),
			typesafe.Bare("other"),
		),
	},
})
```

The client checks these before sending any request and returns `ErrInvalidRequest`
if one fails: at least one question; Choice has ≥1 option and no duplicates; Score
has ≥2 levels and none is nil; state is a string, object, or array. The
documented upper limits (255 options, 10 levels, context length) are left to the server.

## Reading answers

```go
dept, _ := resp.Choice("department")
switch {
case dept.Confidence < 0.5:
	routeToHuman()               // the model is unsure: don't guess
case dept.Choice == "billing":
	sendToBilling()
}
for _, o := range dept.Ranked() { // options by descending probability
	fmt.Println(o.Option, o.Probability)
}
```

A Noul has no separate confidence; its value *is* the probability, and 0.5 means
uncertain, not "medium". Tune thresholds on your own data, and pin a versioned model
(`WithModel("jev-1.13.0")`) once you have. Aliases such as `jev-latest` move
when new releases ship. See [Confidence](https://docs.typesafe.ai/confidence).

`resp.Nouls()`, `resp.Choices()`, and `resp.Scores()` return typed maps. An answer
type this client version doesn't know decodes as `UnknownAnswer` (with its raw JSON)
instead of failing the response.

## Errors

| Error | Meaning |
|---|---|
| `ErrConfig` | Missing or invalid API key or option (from `NewClient`) |
| `ErrInvalidRequest` | Rejected before sending |
| `*APIError` | Non-2xx after retries. Match with `errors.Is(err, ErrRateLimit)`, `ErrAuthentication`, `ErrPermissionDenied`, `ErrNotFound`, `ErrUnprocessableEntity`, `ErrBadRequest`, `ErrInternalServer` (≥500, incl. 529) |
| `*ConnectionError` | No HTTP response: DNS, TLS, reset (`errors.Is(err, ErrConnection)`) |
| `*TimeoutError` | An attempt exceeded the per-attempt timeout (`ErrTimeout`, also `ErrConnection`) |
| `*ResponseValidationError` | A 2xx body is missing or has a mistyped field (`FieldPath` e.g. `answers.tone.confidence`), or omits or mistypes the answer to a question you asked (`answers.<id>`, `answers.<id>.type`). A successful response therefore has an answer for every question |
| `ctx.Err()` | Your context was cancelled or its deadline passed |

`APIError` carries `StatusCode`, `Message` (extracted from the body, including FastAPI
422 details such as `questions.urgency.criteria: Field required`), `Body`, `Header`,
`RequestID` (from `x-typesafe-request-id`, useful when contacting TypeSafe), and
`RetryAfter()`.

## Retries and timeouts

The defaults match the official SDKs (`DefaultRetryPolicy()`):

- 2 retries with backoff from 500ms, doubling to a 5s cap, minus up to 25% jitter
- Retries 408, 429, and 5xx responses (including 529 Overloaded), connection failures, and per-attempt timeouts
- Honors `retry-after-ms` / `Retry-After` up to 60s. A longer requested delay falls back to backoff
- A 30s total budget. A retry whose delay would reach the budget, or your context
  deadline, isn't attempted, and **the last real error is returned** (you see the
  529, not an artificial timeout)
- Sends `X-TypeSafe-Retry-Count` on retries

```go
p := typesafe.DefaultRetryPolicy()
p.MaxRetries = 4
client, _ := typesafe.NewClient(typesafe.WithRetryPolicy(p), typesafe.WithTimeout(5*time.Second))

// Per call:
resp, err := client.SystemOne(ctx, req,
	typesafe.RequestRetryPolicy(typesafe.NoRetries()),
	typesafe.RequestTimeout(2*time.Second),
	typesafe.RequestHeader("X-Trace", id))
```

`WithTimeout` (default 10s) applies to **each attempt**, from connect through reading
the body. Bound a whole call with a context deadline.

## Configuration

| Option | Env var | Default |
|---|---|---|
| `WithAPIKey` | `TYPESAFE_API_KEY` | required |
| `WithBaseURL` | `TYPESAFE_BASE_URL` | `https://api.typesafe.ai` |
| `WithModel` | `TYPESAFE_DEFAULT_MODEL` | `jev-latest` |
| `WithLogger` | `TYPESAFE_LOG_LEVEL` (`debug`, `info`, `warn`, `error`, `off`) | silent |
| `WithTimeout` | | 10s per attempt |
| `WithRetryPolicy` | | `DefaultRetryPolicy()` |
| `WithHeader` | | none |
| `WithHTTPClient` | | `&http.Client{}` |

Explicit options take precedence over environment variables. Blank environment values are ignored.

**Gateways.** Any endpoint that implements the
[TypeSafe OpenAPI spec](https://api.typesafe.ai/docs/) works:

```go
// OpenRouter
typesafe.NewClient(typesafe.WithAPIKey(orKey), typesafe.WithBaseURL("https://openrouter.ai/api"), typesafe.WithModel("~typesafe/jev-latest"))
// Vercel AI Gateway
typesafe.NewClient(typesafe.WithAPIKey(vercelKey), typesafe.WithBaseURL("https://ai-gateway.vercel.sh/typesafe"), typesafe.WithModel("typesafe-ai/jev"))
// Your own proxy, with an attribution header
typesafe.NewClient(typesafe.WithBaseURL("https://gateway.internal"), typesafe.WithHeader("X-Client", "my-service"))
```

**Logging** uses `log/slog`. At `info` the client logs each attempt (status,
duration, request ID) and each retry. At `debug` it also logs headers (credentials
redacted) and bodies (**not** redacted).

## Models

```go
list, err := client.ListModels(ctx)
for _, m := range list.Models {
	fmt.Println(m.Name, m.ReleaseDate, m.Description)
}
```

## Development

```sh
go test -race ./...
golangci-lint run ./...
TYPESAFE_API_KEY=... TYPESAFE_LIVE_TEST=1 go test -run TestLive -v ./...   # spends a few tokens
```
