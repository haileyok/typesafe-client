# Client behavior spec

Both clients in this repo (`go/` and `rust/`) implement this one contract. It
is derived from, in priority order:

1. The live OpenAPI spec, `https://api.typesafe.ai/openapi.json` (snapshot in
   [`spec/openapi.json`](spec/openapi.json), API version 0.2.0). This is the wire
   contract.
2. The TypeSafe docs at <https://docs.typesafe.ai> (`/api`, `/models`, `/primitives/*`,
   `/confidence`, `/concepts/state`).
3. The official SDKs, used as the reference for client behavior (retries,
   errors, configuration): [`typesafe-sdk-python`](https://github.com/typesafe-ai/typesafe-sdk-python)
   0.7.1 and [`typesafe-sdk-js`](https://github.com/typesafe-ai/typesafe-sdk-js) 0.6.0.

Where the sources disagree, the resolution is recorded in [Resolved ambiguities](#resolved-ambiguities).

## Endpoints

| Method | Path | Body | Response |
|---|---|---|---|
| `POST` | `/v1/systemone` | `{state, model, questions}` | `{model, answers, usage}` |
| `GET` | `/v1/models` | – | `{models: [{name, description, release_date}]}` |

## Configuration

Explicit options override environment variables, and environment variables override defaults.
Environment values are trimmed, and an empty or whitespace-only value is ignored.

| Setting | Env var | Default |
|---|---|---|
| API key (required) | `TYPESAFE_API_KEY` | none. Construction fails without one |
| Base URL | `TYPESAFE_BASE_URL` | `https://api.typesafe.ai` (trailing `/` stripped) |
| Default model | `TYPESAFE_DEFAULT_MODEL` | `jev-latest` |
| Log level | `TYPESAFE_LOG_LEVEL` | off (`debug`, `info`, `warn`, `error`, `off`) |
| Per-attempt timeout | – | 10s (covers connect through reading the full body) |

- **API key validation (at construction):** trim surrounding whitespace. Reject empty keys and
  keys containing whitespace, control characters, or non-ASCII characters. The error
  must not echo the key.
- **Gateways:** any base URL that implements the TypeSafe OpenAPI spec works, for example
  OpenRouter (`https://openrouter.ai/api`, model `~typesafe/jev-latest`) or Vercel
  AI Gateway (`https://ai-gateway.vercel.sh/typesafe`, model `typesafe-ai/jev`).
  Extra default headers are supported for gateway attribution.

## Request headers

Set on every attempt. SDK-owned headers take precedence over caller headers.

```
Authorization: Bearer <key>
Accept: application/json
Content-Type: application/json          (only when there is a body)
User-Agent: typesafe-client-<lang>/<version>
X-TypeSafe-SDK: typesafe-client-<lang>/<version>
X-TypeSafe-Runtime: <lang>/<version> (<os>; <arch>)
X-TypeSafe-Retry-Count: <n>             (retries only, n >= 1)
```

Header precedence, lowest to highest: client default headers, then per-call headers,
then SDK-owned headers. A caller-supplied `X-TypeSafe-Retry-Count` is dropped.

## Questions

A question is exactly one of three kinds, discriminated by `type`.

| Kind | `instructions` | `criteria` |
|---|---|---|
| `noul` | optional; string, object, array | optional `{true?, false?}`; each value is string, object, array, or null |
| `choice` | optional | **required** map `option -> description`; description is string, object, array, or **null** |
| `score` | optional | **required** ordered list of level descriptions; each is string, object, or array |

- Omit unset optional fields from the wire (don't send `null` for an unset field). Nulls
  the caller placed *inside* criteria are preserved.
- Serialize Choice options **in the order the caller supplied them**. Clients must
  offer an order-preserving constructor.
- Question IDs are the map keys. They are for code only and are not sent to the model.

### Client-side validation (before any network I/O)

Each failure is an "invalid request" error that names the offending question:

- `questions` must be non-empty.
- A `choice` must have at least one option.
- A `score` must have at least two levels, and no level may be null.
- `state` must serialize to a JSON string, object, or array. Numbers, booleans, and null are rejected.

The client does **not** enforce the documented upper limits (255 Choice options, 10 Score
levels, context length). The server enforces them with a 422, and they are expected to change.

## Answers

| Kind | Fields (all required on the wire) |
|---|---|
| `noul` | `noul: number` (0 to 1, probability of yes). There is no confidence |
| `choice` | `choice: string`, `probabilities: {option: number}`, `confidence: number` |
| `score` | `score: number`, `legend: {"<level>": string\|object\|array}`, `probabilities: {"<level>": number}`, `confidence: number` |

- Score `legend` and `probabilities` keys are decimal level indexes. Clients expose them
  as integers.
- **Validation:** a 2xx body must be a JSON object with `model` (string) and `answers`
  (object). Each answer must be an object with a string `type`, and every field required
  for a known `type` must be present and correctly typed. A failure is a *response
  validation error* carrying a dotted field path (for example `answers.tone.confidence`)
  and the HTTP status.
- **Completeness:** every requested question ID must have an answer. A missing answer is
  a response validation error at `answers.<id>`. An answer whose `type` is a *known*
  kind different from its question's type is an error at `answers.<id>.type`. Extra
  answer IDs are ignored. This goes beyond the official SDKs: a typed accessor on a
  missing answer would otherwise return a zero value that reads as a real "no".
- **Forward compatibility:** an answer with an unrecognized `type` is not an error. It is
  kept as an opaque "unknown" answer that carries its raw JSON.
- `usage.input_tokens` and `usage.output_tokens` default to 0 when absent, because some
  gateways omit usage. The response `model` is the versioned ID that actually answered
  (for example `jev-1.13.0`) even when the request used an alias.
- Expose the `x-typesafe-request-id` response header on responses and errors.

## Errors

| Condition | Error |
|---|---|
| Missing or invalid configuration | config error |
| Client-side validation failure | invalid-request error |
| Non-2xx response | API error: status, headers, parsed body, message, request ID, method + URL |
| No HTTP response (DNS, TLS, reset, body read failure) | connection error |
| Attempt exceeded the per-attempt timeout | timeout error (a kind of connection error) |
| Caller cancelled | the language's native cancellation (Go: `ctx.Err()`; Rust: dropping the future) |
| 2xx body not matching the schema | response validation error |

API error kinds by status: 400 bad request, 401 authentication, 403 permission denied,
404 not found, 422 unprocessable entity, 429 rate limit (also exposes the parsed
retry-after), ≥500 internal server. Any other status is a generic API error.

**Message extraction** (first match wins): body is a non-empty string (truncated to
200 characters plus `…`; the official SDKs don't truncate here, but gateway HTML
error pages can be huge), then `error`
(string), then `error.message`, then `message`, then `detail` (string), then
`detail.message`, then `detail[]` as FastAPI validation errors rendered as
`"<loc without 'body' joined by '.'>: <msg>"` joined by `"; "`. Otherwise use the raw
body, truncated to 200 characters plus `…`. An empty body gives `"status code (no body)"`.
The error string is `"<METHOD> <URL>: <status> <message> (request_id=<id>)"`, where the
request-ID suffix appears only when the header is present. Never include the API key.

## Retries

Default policy (identical to both official SDKs):

| Field | Default |
|---|---|
| max retries (after the first attempt) | 2 |
| backoff initial | 500ms |
| backoff max | 5s |
| backoff jitter | 0.25 |
| retryable HTTP statuses | 408, 429, 500–599 |
| respect `retry-after-ms` / `Retry-After` | true |
| max honored retry-after | 60s (a longer server delay falls back to backoff) |
| retry connection errors | true |
| retry timeout errors | true |
| total budget | 30s (0 = unlimited) |

- **Delay for retry n (0-based):** if a retry-after is respected, parseable, and ≤ max,
  use it exactly. Otherwise `min(initial * 2^n, max) * (1 - rand[0,1) * jitter)`. If
  initial or max is 0, the delay is 0.
- **Retry-after parsing:** prefer `retry-after-ms` (float milliseconds, ≥ 0). Otherwise
  `Retry-After` as float seconds ≥ 0, or as an HTTP date (delay = max(0, date − now)).
  Negative or unparseable values mean "absent".
- **Stop conditions:** retries are exhausted; the error isn't retryable; the caller
  cancelled; or the next delay would reach or exceed the remaining total budget *or*
  the caller's deadline. In the budget and deadline cases, return the **last real error**
  rather than a timeout, so the caller sees the 529 and not an artificial deadline error.
- The budget gates *starting* a retry. An attempt already in flight is not cut short,
  so a call can overrun the budget by up to one per-attempt timeout, which matches
  Python's `stop_before_delay`. Callers who need a hard bound use their language's
  deadline mechanism.
- Per-call options (timeout, retry policy) get the same validation as client options,
  reported as an invalid-request error.
- A per-call retry policy fully replaces the client policy for that call.
- A retry sets `X-TypeSafe-Retry-Count`.

## Logging

Logging is off unless configured. At `info`, log one line per attempt result (method,
path, status, duration, request ID) and per scheduled retry (delay and reason). At
`debug`, also log headers and bodies. Redact the `Authorization`, `Proxy-Authorization`,
`X-Api-Key`, `Api-Key`, `Cookie`, and `Set-Cookie` headers. Bodies are not redacted;
document this.

## Resolved ambiguities

| Topic | Sources | Resolution |
|---|---|---|
| Score min levels | OpenAPI `minItems: 1`; Python ≥1; docs "should have at least two"; JS ≥2 | **≥2**. A one-level score is always degenerate |
| Null Score levels | docs/advanced allows null; OpenAPI disallows; JS allows | **reject** (wire contract) |
| Null state | JS allows; OpenAPI disallows | **reject** (wire contract) |
| Retry-after above cap | JS falls back to backoff above 60s; Python honors it within budget | JS behavior, plus the budget check |
| Total retry budget | Python 30s; JS none | 30s default, configurable, 0 disables |
| Missing usage | OpenAPI required; Python optional | tolerate, default 0 |
| Unknown answer types | Python drops them with a warning | keep as opaque "unknown" answers (lossless) |
