//! Integration tests over wiremock: request shape, headers, decoding, errors,
//! retries, and the budget stop conditions.

use std::time::Duration;

use serde_json::{json, Value};
use wiremock::matchers::{body_partial_json, header, method, path};
use wiremock::MockServer;
use wiremock::{Mock, ResponseTemplate};

use typesafe_system_one::{
    ApiErrorKind, Choice, Client, Error, LogLevel, Noul, RequestOptions, RetryPolicy, Score,
    SystemOneRequest,
};

/// A fast retry policy: 5ms backoff, no jitter.
fn fast_retry() -> RetryPolicy {
    RetryPolicy {
        max_retries: 2,
        backoff_initial: Duration::from_millis(5),
        backoff_max: Duration::from_millis(5),
        backoff_jitter: 0.0,
        total_budget: Some(Duration::from_secs(30)),
        ..Default::default()
    }
}

async fn mock_server() -> MockServer {
    MockServer::start().await
}

async fn client_for(server: &MockServer) -> Client {
    Client::builder()
        .api_key("sk-test-key")
        .base_url(server.uri())
        .retry_policy(RetryPolicy::none())
        .log_level(LogLevel::Debug)
        .build()
        .unwrap()
}

fn ok_body() -> Value {
    json!({
        "model": "jev-1.13.0",
        "answers": {
            "spam": {"type": "noul", "noul": 0.98},
            "tone": {"type": "choice", "choice": "angry",
                     "probabilities": {"angry": 0.8, "calm": 0.2}, "confidence": 0.9},
            "urgency": {"type": "score", "score": 1.7, "confidence": 0.9,
                        "legend": {"0": "Can wait", "1": "This week", "2": "Today"},
                        "probabilities": {"0": 0.1, "1": 0.1, "2": 0.8}}
        },
        "usage": {"input_tokens": 120, "output_tokens": 12}
    })
}

/// Answers every question in the request with a well-formed answer of the
/// matching type, so request-shape tests pass the completeness check no
/// matter which question IDs they send.
struct EchoAnswers;

impl wiremock::Respond for EchoAnswers {
    fn respond(&self, request: &wiremock::Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&request.body).unwrap();
        let mut answers = serde_json::Map::new();
        for (id, question) in body["questions"].as_object().unwrap() {
            let answer = match question["type"].as_str().unwrap() {
                "noul" => json!({"type": "noul", "noul": 0.5}),
                "choice" => {
                    let first = question["criteria"]
                        .as_object()
                        .unwrap()
                        .keys()
                        .next()
                        .unwrap()
                        .clone();
                    json!({"type": "choice", "choice": first.clone(),
                           "probabilities": {first: 1.0}, "confidence": 1.0})
                }
                _ => json!({"type": "score", "score": 0.0, "confidence": 1.0,
                            "legend": {"0": "low"}, "probabilities": {"0": 1.0}}),
            };
            answers.insert(id.clone(), answer);
        }
        ResponseTemplate::new(200).set_body_json(json!({"model": "jev-1.13.0", "answers": answers}))
    }
}

fn simple_request() -> SystemOneRequest {
    SystemOneRequest::new("I was charged twice.")
        .question("spam", Noul::new("Is this spam?"))
        .question(
            "tone",
            Choice::new("What tone?")
                .option("calm", "Neutral")
                .option("angry", "Hostile"),
        )
        .question("urgency", Score::new("How urgent?", ["Can wait", "Today"]))
}

// ---------------------------------------------------------------------------
// Request body shape
// ---------------------------------------------------------------------------

#[tokio::test]
async fn request_body_shape_and_omitted_fields() {
    let server = mock_server().await;
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .and(body_partial_json(json!({
            "model": "jev-latest",
            "state": "I was charged twice.",
            "questions": {
                "n": {"type": "noul", "instructions": "Is this spam?"},
                "c": {"type": "choice", "instructions": "Tone?", "criteria": {"calm": null, "angry": "Hostile"}},
                "s": {"type": "score", "criteria": ["Low", "High"]}
            }
        })))
        .respond_with(EchoAnswers)
        .mount(&server)
        .await;

    // The body must NOT contain null instructions/criteria for unset fields:
    // verify with an exact-body check via a captured body.
    let client = client_for(&server).await;
    client
        .system_one(
            SystemOneRequest::new("I was charged twice.")
                .question("n", Noul::new("Is this spam?"))
                .question(
                    "c",
                    Choice::new("Tone?")
                        .bare_option("calm")
                        .option("angry", "Hostile"),
                )
                .question("s", Score::new("How urgent?", ["Low", "High"])),
        )
        .await
        .unwrap();

    let bodies = server.received_requests().await.unwrap();
    assert_eq!(bodies.len(), 1);
    let body: Value = serde_json::from_slice(&bodies[0].body).unwrap();
    assert_eq!(body["model"], "jev-latest");
    assert_eq!(body["state"], "I was charged twice.");
    // Omitted optional fields, never null.
    assert!(body["questions"]["n"].get("criteria").is_none());
    assert!(body["questions"]["c"].get("criteria").is_some());
    assert!(body["questions"]["s"].get("criteria").is_some());
    let noul = &body["questions"]["n"];
    assert_eq!(noul["type"], "noul");
    assert_eq!(noul["instructions"], "Is this spam?");
    assert!(serde_json::to_string(noul)
        .unwrap()
        .find("\"criteria\":null")
        .is_none());
    // Choice preserves insertion order.
    let criteria = body["questions"]["c"]["criteria"].as_object().unwrap();
    assert_eq!(
        criteria.keys().collect::<Vec<_>>(),
        vec!["calm", "angry"],
        "{criteria:?}"
    );
    assert_eq!(criteria["calm"], Value::Null);
}

#[tokio::test]
async fn structured_instructions_and_criteria() {
    let server = mock_server().await;
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .respond_with(EchoAnswers)
        .mount(&server)
        .await;
    let client = client_for(&server).await;
    client
        .system_one(
            SystemOneRequest::new(json!({"subject": "Refund"}))
                .question(
                    "n",
                    Noul::new(json!({"task": "Find billing issues."})).criteria(
                        Some(json!({"includes": ["duplicates"]})),
                        Some(json!(["otherwise"])),
                    ),
                )
                .question(
                    "s",
                    Score::new(
                        json!({"task": "Rate"}),
                        vec![json!({"label": "Low"}), json!("High")],
                    ),
                ),
        )
        .await
        .unwrap();

    let bodies = server.received_requests().await.unwrap();
    let body: Value = serde_json::from_slice(&bodies[0].body).unwrap();
    assert_eq!(
        body["questions"]["n"]["instructions"],
        json!({"task": "Find billing issues."})
    );
    assert_eq!(
        body["questions"]["n"]["criteria"],
        json!({"true": {"includes": ["duplicates"]}, "false": ["otherwise"]})
    );
    assert_eq!(
        body["questions"]["s"]["criteria"],
        json!([{"label": "Low"}, "High"])
    );
}

#[tokio::test]
async fn per_request_model_override_and_extra_body() {
    let server = mock_server().await;
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .respond_with(EchoAnswers)
        .mount(&server)
        .await;
    let client = Client::builder()
        .api_key("sk-test-key")
        .base_url(server.uri())
        .default_model("jev-default")
        .retry_policy(RetryPolicy::none())
        .build()
        .unwrap();

    client
        .system_one(simple_request().model("jev-1.13.0"))
        .await
        .unwrap();
    let bodies = server.received_requests().await.unwrap();
    let body: Value = serde_json::from_slice(&bodies[0].body).unwrap();
    assert_eq!(body["model"], "jev-1.13.0");

    client
        .system_one_with(
            SystemOneRequest::new("x").question("a", Noul::new("q")),
            RequestOptions::new().extra_body("foo", json!(1)),
        )
        .await
        .unwrap();
    let bodies = server.received_requests().await.unwrap();
    let body: Value = serde_json::from_slice(&bodies[1].body).unwrap();
    assert_eq!(body["model"], "jev-default"); // no override -> default model
    assert_eq!(body["foo"], 1);
    assert_eq!(body["state"], "x");
}

// ---------------------------------------------------------------------------
// Headers
// ---------------------------------------------------------------------------

#[tokio::test]
async fn request_headers_and_precedence() {
    let server = mock_server().await;
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .and(header("authorization", "Bearer sk-test-key"))
        .and(header("accept", "application/json"))
        .and(header("content-type", "application/json"))
        .and(header("user-agent", "typesafe-client-rust/0.1.0"))
        .and(header("x-typesafe-sdk", "typesafe-client-rust/0.1.0"))
        .and(header("x-typesafe-runtime", runtime_header()))
        .and(header("x-agent-client", "per-call-wins"))
        .and(header("x-custom", "custom-value"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(ok_body())
                .insert_header("x-typesafe-request-id", "req-1"),
        )
        .mount(&server)
        .await;

    let client = Client::builder()
        .api_key("sk-test-key")
        .base_url(server.uri())
        .retry_policy(RetryPolicy::none())
        .default_header("X-Agent-Client", "default-value")
        .default_header("X-Custom", "custom-value")
        .build()
        .unwrap();
    let response = client
        .system_one_with(
            simple_request(),
            RequestOptions::new().header("X-Agent-Client", "per-call-wins"),
        )
        .await
        .unwrap();
    assert_eq!(response.request_id.as_deref(), Some("req-1"));

    // A per-call header outranks the client default but loses to SDK-owned
    // headers (a separate catch-all mock accepts any request).
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0]
            .headers
            .get("x-agent-client")
            .and_then(|v| v.to_str().ok()),
        Some("per-call-wins")
    );
    assert_eq!(
        requests[0]
            .headers
            .get("x-custom")
            .and_then(|v| v.to_str().ok()),
        Some("custom-value")
    );
}

#[tokio::test]
async fn sdk_owned_headers_win_over_callers() {
    let server = mock_server().await;
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .and(header("authorization", "Bearer sk-test-key"))
        .and(header("user-agent", "typesafe-client-rust/0.1.0"))
        .and(header("accept", "application/json"))
        .and(header("x-typesafe-sdk", "typesafe-client-rust/0.1.0"))
        .and(header("x-typesafe-runtime", runtime_header()))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_body()))
        .mount(&server)
        .await;

    // The caller tries to override SDK-owned headers; the SDK's values win.
    let client = Client::builder()
        .api_key("sk-test-key")
        .base_url(server.uri())
        .retry_policy(RetryPolicy::none())
        .default_header("User-Agent", "my-app")
        .default_header("Authorization", "Bearer wrong")
        .default_header("Accept", "text/plain")
        .default_header("X-TypeSafe-SDK", "my-app")
        .default_header("X-TypeSafe-Runtime", "my-runtime")
        .build()
        .unwrap();
    client
        .system_one_with(
            simple_request(),
            RequestOptions::new()
                .header("User-Agent", "per-call-app")
                .header("X-Agent-Client", "attribution"),
        )
        .await
        .unwrap();

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0]
            .headers
            .get("user-agent")
            .and_then(|v| v.to_str().ok()),
        Some("typesafe-client-rust/0.1.0")
    );
    assert_eq!(
        requests[0]
            .headers
            .get("authorization")
            .and_then(|v| v.to_str().ok()),
        Some("Bearer sk-test-key")
    );
    assert_eq!(
        requests[0]
            .headers
            .get("accept")
            .and_then(|v| v.to_str().ok()),
        Some("application/json")
    );
    assert_eq!(
        requests[0]
            .headers
            .get("x-typesafe-sdk")
            .and_then(|v| v.to_str().ok()),
        Some("typesafe-client-rust/0.1.0")
    );
    assert_eq!(
        requests[0]
            .headers
            .get("x-agent-client")
            .and_then(|v| v.to_str().ok()),
        Some("attribution")
    );
}

#[tokio::test]
async fn header_precedence_is_case_insensitive() {
    // Regression: headers used to live in a case-sensitive map, so a
    // lowercase caller header sorted after the SDK's canonical-case one and
    // won in the (case-insensitive) HeaderMap, overriding Authorization.
    let server = mock_server().await;
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_body()))
        .mount(&server)
        .await;
    let client = Client::builder()
        .api_key("sk-test-key")
        .base_url(server.uri())
        .retry_policy(RetryPolicy::none())
        .default_header("authorization", "Bearer hijacked")
        .default_header("user-agent", "my-app")
        .default_header("x-agent-client", "from-default")
        .default_header("X-Other", "from-default")
        .build()
        .unwrap();
    client
        .system_one_with(
            simple_request(),
            RequestOptions::new()
                .header("AUTHORIZATION", "Bearer hijacked-too")
                .header("X-Agent-Client", "from-call")
                .header("x-other", "from-call"),
        )
        .await
        .unwrap();

    let requests = server.received_requests().await.unwrap();
    let get = |name: &str| -> Vec<String> {
        requests[0]
            .headers
            .get_all(name)
            .iter()
            .map(|v| v.to_str().unwrap().to_owned())
            .collect()
    };
    assert_eq!(get("authorization"), vec!["Bearer sk-test-key"]);
    assert_eq!(get("user-agent"), vec!["typesafe-client-rust/0.1.0"]);
    // Per-call headers beat client defaults regardless of letter case.
    assert_eq!(get("x-agent-client"), vec!["from-call"]);
    assert_eq!(get("x-other"), vec!["from-call"]);
}

#[tokio::test]
async fn get_requests_send_no_body() {
    let server = mock_server().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"models": []})))
        .mount(&server)
        .await;
    let client = client_for(&server).await;
    client
        .list_models_with(RequestOptions::new().header("content-type", "text/plain"))
        .await
        .unwrap();
    let requests = server.received_requests().await.unwrap();
    assert!(requests[0].body.is_empty());
    assert!(requests[0].headers.get("content-type").is_none());
}

#[tokio::test]
async fn rate_limit_error_exposes_retry_after_even_when_not_respected() {
    let server = mock_server().await;
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .respond_with(ResponseTemplate::new(429).insert_header("retry-after-ms", "1500"))
        .mount(&server)
        .await;
    let client = Client::builder()
        .api_key("sk-test-key")
        .base_url(server.uri())
        .retry_policy(RetryPolicy {
            respect_retry_after: false,
            ..RetryPolicy::none()
        })
        .build()
        .unwrap();
    let error = client.system_one(simple_request()).await.unwrap_err();
    let api = error.as_api().expect("api error");
    assert_eq!(api.kind, ApiErrorKind::RateLimit);
    assert_eq!(api.retry_after, Some(Duration::from_millis(1500)));
}

#[tokio::test]
async fn no_content_type_on_get() {
    let server = mock_server().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"models": [
            {"name": "jev-latest", "description": "General.", "release_date": "2026-09-15"}
        ]})))
        .mount(&server)
        .await;
    let client = client_for(&server).await;
    let models = client.list_models().await.unwrap();
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].name, "jev-latest");

    let requests = server.received_requests().await.unwrap();
    assert!(requests[0].headers.get("content-type").is_none());
    assert!(requests[0].headers.get("x-typesafe-retry-count").is_none());
}

#[tokio::test]
async fn retry_count_header_on_retries() {
    let server = mock_server().await;
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .respond_with(ResponseTemplate::new(500))
        .up_to_n_times(2)
        .expect(2)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_body()))
        .mount(&server)
        .await;

    let client = Client::builder()
        .api_key("sk-test-key")
        .base_url(server.uri())
        .retry_policy(fast_retry())
        .build()
        .unwrap();
    client.system_one(simple_request()).await.unwrap();

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 3);
    assert!(requests[0].headers.get("x-typesafe-retry-count").is_none());
    assert_eq!(
        requests[1]
            .headers
            .get("x-typesafe-retry-count")
            .and_then(|v| v.to_str().ok()),
        Some("1")
    );
    assert_eq!(
        requests[2]
            .headers
            .get("x-typesafe-retry-count")
            .and_then(|v| v.to_str().ok()),
        Some("2")
    );
}

#[tokio::test]
async fn caller_supplied_retry_count_is_dropped() {
    let server = mock_server().await;
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_body()))
        .mount(&server)
        .await;
    let client = client_for(&server).await;
    client
        .system_one_with(
            simple_request(),
            RequestOptions::new().header("X-TypeSafe-Retry-Count", "99"),
        )
        .await
        .unwrap();
    let requests = server.received_requests().await.unwrap();
    assert!(requests[0].headers.get("x-typesafe-retry-count").is_none());
}

// ---------------------------------------------------------------------------
// Client-side validation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn validation_runs_before_network() {
    let server = mock_server().await;
    let client = client_for(&server).await;

    // Empty questions.
    let error = client
        .system_one(SystemOneRequest::new("state"))
        .await
        .unwrap_err();
    assert!(matches!(error, Error::InvalidRequest(_)), "{error}");
    // Number state.
    let error = client
        .system_one(SystemOneRequest::new(42))
        .await
        .unwrap_err();
    assert!(matches!(error, Error::InvalidRequest(_)), "{error}");
    // Bool state.
    let error = client
        .system_one(SystemOneRequest::new(true))
        .await
        .unwrap_err();
    assert!(matches!(error, Error::InvalidRequest(_)), "{error}");
    // Null state.
    let error = client
        .system_one(SystemOneRequest::new(Value::Null))
        .await
        .unwrap_err();
    assert!(matches!(error, Error::InvalidRequest(_)), "{error}");
    // Empty choice.
    let error = client
        .system_one(SystemOneRequest::new("state").question("c", Choice::new("Which?")))
        .await
        .unwrap_err();
    assert!(matches!(error, Error::InvalidRequest(_)), "{error}");
    // One-level score.
    let error = client
        .system_one(SystemOneRequest::new("state").question("s", Score::new("Rate?", ["Only"])))
        .await
        .unwrap_err();
    assert!(matches!(error, Error::InvalidRequest(_)), "{error}");

    // Nothing hit the network.
    assert!(server.received_requests().await.unwrap().is_empty());
}

// ---------------------------------------------------------------------------
// Decoding
// ---------------------------------------------------------------------------

#[tokio::test]
async fn decodes_all_answer_kinds_and_accessors() {
    let server = mock_server().await;
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_body()))
        .mount(&server)
        .await;
    let client = client_for(&server).await;
    let response = client.system_one(simple_request()).await.unwrap();

    assert_eq!(response.model, "jev-1.13.0");
    assert_eq!(response.usage.input_tokens, 120);
    assert_eq!(response.usage.output_tokens, 12);
    assert_eq!(response.noul("spam").unwrap().noul, 0.98);
    let tone = response.choice("tone").unwrap();
    assert_eq!(tone.choice, "angry");
    assert_eq!(tone.confidence, 0.9);
    assert_eq!(tone.ranked()[0], ("angry", 0.8));
    let urgency = response.score("urgency").unwrap();
    assert_eq!(urgency.score, 1.7);
    assert_eq!(urgency.legend[&2], "Today");
    assert_eq!(urgency.probabilities[&2], 0.8);
    assert_eq!(response.nouls().count(), 1);
    assert_eq!(response.choices().count(), 1);
    assert_eq!(response.scores().count(), 1);
    assert!(response.choice("spam").is_none());
}

#[tokio::test]
async fn unknown_answer_type_kept_and_missing_usage_defaults() {
    let server = mock_server().await;
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "model": "jev-1.13.0",
            "answers": {
                "a": {"type": "noul", "noul": 1.0},
                "future": {"type": "vibes", "vibe": "immaculate", "confidence": 0.5}
            }
        })))
        .mount(&server)
        .await;
    let client = client_for(&server).await;
    let response = client
        .system_one(SystemOneRequest::new("x").question("a", Noul::new("q")))
        .await
        .unwrap();
    assert_eq!(response.usage.input_tokens, 0);
    assert_eq!(response.usage.output_tokens, 0);
    match response.answers.get("future").unwrap() {
        typesafe_system_one::Answer::Unknown { kind, raw } => {
            assert_eq!(kind, "vibes");
            assert_eq!(raw["vibe"], "immaculate");
        }
        other => panic!("expected Unknown, got {other:?}"),
    }
}

#[tokio::test]
async fn validation_error_field_paths() {
    let server = mock_server().await;
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "model": "jev-1.13.0",
            "answers": {"tone": {"type": "choice", "choice": "calm", "probabilities": {"calm": 1.0}}}
        })))
        .mount(&server)
        .await;
    let client = client_for(&server).await;
    let error = client.system_one(simple_request()).await.unwrap_err();
    match error {
        Error::ResponseValidation {
            field_path, status, ..
        } => {
            assert_eq!(field_path, "answers.tone.confidence");
            assert_eq!(status, 200);
        }
        other => panic!("expected ResponseValidation, got {other:?}"),
    }

    // Non-object 2xx body.
    let server2 = mock_server().await;
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .respond_with(ResponseTemplate::new(200).set_body_string("nope"))
        .mount(&server2)
        .await;
    let client = client_for(&server2).await;
    let error = client.system_one(simple_request()).await.unwrap_err();
    match error {
        Error::ResponseValidation {
            field_path, status, ..
        } => {
            assert_eq!(field_path, "");
            assert_eq!(status, 200);
        }
        other => panic!("expected ResponseValidation, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[tokio::test]
async fn api_error_kind_per_status() {
    for (status, kind) in [
        (400, ApiErrorKind::BadRequest),
        (401, ApiErrorKind::Authentication),
        (403, ApiErrorKind::PermissionDenied),
        (404, ApiErrorKind::NotFound),
        (422, ApiErrorKind::UnprocessableEntity),
        (429, ApiErrorKind::RateLimit),
        (500, ApiErrorKind::InternalServer),
        (503, ApiErrorKind::InternalServer),
        (599, ApiErrorKind::InternalServer),
        (418, ApiErrorKind::Other),
    ] {
        let server = mock_server().await;
        Mock::given(method("POST"))
            .and(path("/v1/systemone"))
            .respond_with(ResponseTemplate::new(status).set_body_string("boom"))
            .mount(&server)
            .await;
        let client = client_for(&server).await;
        let error = client.system_one(simple_request()).await.unwrap_err();
        let api = error.as_api().expect("api error");
        assert_eq!(api.kind, kind, "{status}");
        assert_eq!(api.status, status);
        assert_eq!(error.status(), Some(status));
        assert_eq!(api.message, "boom");
        // Display: METHOD URL: status message (no request id here).
        assert_eq!(
            error.to_string(),
            format!("POST {}/v1/systemone: {status} boom", server.uri())
        );
    }
}

#[tokio::test]
async fn message_extraction_paths() {
    let cases = [
        (json!("plain string body"), "plain string body"),
        (json!({"error": "error string"}), "error string"),
        (
            json!({"error": {"message": "nested message"}}),
            "nested message",
        ),
        (json!({"message": "message field"}), "message field"),
        (json!({"detail": "detail string"}), "detail string"),
        (
            json!({"detail": {"message": "detail message"}}),
            "detail message",
        ),
        (
            json!({"detail": [
                {"loc": ["body", "questions", "urgency", "score", "criteria"], "msg": "Field required", "type": "missing"}
            ]}),
            "questions.urgency.score.criteria: Field required",
        ),
    ];
    for (body, expected) in cases {
        let server = mock_server().await;
        Mock::given(method("POST"))
            .and(path("/v1/systemone"))
            .respond_with(ResponseTemplate::new(400).set_body_json(&body))
            .mount(&server)
            .await;
        let client = client_for(&server).await;
        let error = client.system_one(simple_request()).await.unwrap_err();
        assert_eq!(error.as_api().unwrap().message, expected, "{body}");
    }
}

#[tokio::test]
async fn message_truncation_and_no_body() {
    // Truncated raw body (non-extractable object).
    let server = mock_server().await;
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .respond_with(
            ResponseTemplate::new(500).set_body_json(json!({"unexpected": "x".repeat(300)})),
        )
        .mount(&server)
        .await;
    let client = client_for(&server).await;
    let error = client.system_one(simple_request()).await.unwrap_err();
    let message = error.as_api().unwrap().message.clone();
    assert_eq!(message.chars().count(), 201);
    assert!(message.ends_with('…'));

    // Empty body: no message, and "status code (no body)".
    let server = mock_server().await;
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .respond_with(ResponseTemplate::new(502))
        .mount(&server)
        .await;
    let client = client_for(&server).await;
    let error = client.system_one(simple_request()).await.unwrap_err();
    assert_eq!(error.as_api().unwrap().message, "status code (no body)");

    // Plain-string body over 200 chars is truncated too (SPEC: Unicode scalar values).
    let server = mock_server().await;
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .respond_with(ResponseTemplate::new(502).set_body_string("é".repeat(250)))
        .mount(&server)
        .await;
    let client = client_for(&server).await;
    let error = client.system_one(simple_request()).await.unwrap_err();
    let message = error.as_api().unwrap().message.clone();
    assert_eq!(message.chars().count(), 201);
    assert!(message.ends_with('…'));
    assert_eq!(message.chars().take(200).filter(|c| *c == 'é').count(), 200);
}

#[tokio::test]
async fn request_id_captured_on_errors() {
    let server = mock_server().await;
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .respond_with(
            ResponseTemplate::new(429)
                .set_body_string("slow down")
                .insert_header("x-typesafe-request-id", "req-42"),
        )
        .mount(&server)
        .await;
    let client = client_for(&server).await;
    let error = client.system_one(simple_request()).await.unwrap_err();
    assert_eq!(error.request_id(), Some("req-42"));
    assert_eq!(
        error.to_string(),
        format!(
            "POST {}/v1/systemone: 429 slow down (request_id=req-42)",
            server.uri()
        )
    );
}

// ---------------------------------------------------------------------------
// Retries
// ---------------------------------------------------------------------------

#[tokio::test]
async fn retries_retryable_statuses() {
    for status in [408u16, 429, 500, 529, 599] {
        let server = mock_server().await;
        Mock::given(method("POST"))
            .and(path("/v1/systemone"))
            .respond_with(ResponseTemplate::new(status))
            .up_to_n_times(2)
            .expect(2)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/systemone"))
            .respond_with(ResponseTemplate::new(200).set_body_json(ok_body()))
            .mount(&server)
            .await;
        let client = Client::builder()
            .api_key("sk-test-key")
            .base_url(server.uri())
            .retry_policy(fast_retry())
            .build()
            .unwrap();
        let response = client.system_one(simple_request()).await.unwrap();
        assert_eq!(response.model, "jev-1.13.0", "{status}");
    }
}

#[tokio::test]
async fn does_not_retry_non_retryable_statuses() {
    for status in [400u16, 401, 422] {
        let server = mock_server().await;
        Mock::given(method("POST"))
            .and(path("/v1/systemone"))
            .respond_with(ResponseTemplate::new(status).set_body_string("no"))
            .expect(1)
            .mount(&server)
            .await;
        let client = Client::builder()
            .api_key("sk-test-key")
            .base_url(server.uri())
            .retry_policy(fast_retry())
            .build()
            .unwrap();
        let error = client.system_one(simple_request()).await.unwrap_err();
        assert_eq!(error.status(), Some(status));
    }
}

#[tokio::test]
async fn max_retries_respected() {
    let server = mock_server().await;
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .respond_with(ResponseTemplate::new(500))
        .expect(4) // 1 attempt + 3 retries
        .mount(&server)
        .await;
    let client = Client::builder()
        .api_key("sk-test-key")
        .base_url(server.uri())
        .retry_policy(RetryPolicy {
            max_retries: 3,
            ..fast_retry()
        })
        .build()
        .unwrap();
    let error = client.system_one(simple_request()).await.unwrap_err();
    assert_eq!(error.status(), Some(500));
}

#[tokio::test]
async fn retry_after_ms_honored() {
    let server = mock_server().await;
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .respond_with(ResponseTemplate::new(429).insert_header("retry-after-ms", "50"))
        .up_to_n_times(1)
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_body()))
        .mount(&server)
        .await;
    let client = Client::builder()
        .api_key("sk-test-key")
        .base_url(server.uri())
        .retry_policy(fast_retry())
        .build()
        .unwrap();
    let started = std::time::Instant::now();
    client.system_one(simple_request()).await.unwrap();
    assert!(
        started.elapsed() >= Duration::from_millis(45),
        "retry-after-ms not honored: {:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn retry_after_seconds_honored() {
    let server = mock_server().await;
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "0.1"))
        .up_to_n_times(1)
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_body()))
        .mount(&server)
        .await;
    let client = Client::builder()
        .api_key("sk-test-key")
        .base_url(server.uri())
        .retry_policy(fast_retry())
        .build()
        .unwrap();
    let started = std::time::Instant::now();
    client.system_one(simple_request()).await.unwrap();
    assert!(
        started.elapsed() >= Duration::from_millis(95),
        "Retry-After seconds not honored: {:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn retry_after_http_date_honored() {
    let server = mock_server().await;
    // HTTP dates have 1-second resolution: formatting `now + 3s` truncates
    // the fractional second, so the honored delay lands in roughly (2s, 3s].
    // Asserting >= 1s leaves a wide margin for that truncation and for time
    // spent between formatting and parsing (the fast retry policy's backoff
    // is milliseconds, so >= 1s can only come from the header).
    let date = httpdate::fmt_http_date(std::time::SystemTime::now() + Duration::from_secs(3));
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", date))
        .up_to_n_times(1)
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_body()))
        .mount(&server)
        .await;
    let client = Client::builder()
        .api_key("sk-test-key")
        .base_url(server.uri())
        .retry_policy(fast_retry())
        .build()
        .unwrap();
    let started = std::time::Instant::now();
    client.system_one(simple_request()).await.unwrap();
    assert!(
        started.elapsed() >= Duration::from_secs(1),
        "Retry-After date not honored: {:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn retry_after_above_cap_falls_back_to_backoff() {
    let server = mock_server().await;
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "61")) // above the 60s cap
        .up_to_n_times(1)
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .respond_with(ResponseTemplate::new(200).set_body_json(ok_body()))
        .mount(&server)
        .await;
    let client = Client::builder()
        .api_key("sk-test-key")
        .base_url(server.uri())
        .retry_policy(fast_retry())
        .build()
        .unwrap();
    let started = std::time::Instant::now();
    client.system_one(simple_request()).await.unwrap();
    // Backoff is 5ms, not 61s.
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "retry-after above cap was honored: {:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn budget_stop_returns_last_api_error() {
    // The budget is small; the next delay (a big retry-after) would exceed it.
    // The result must be the 529 API error, not a timeout.
    let server = mock_server().await;
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .respond_with(
            ResponseTemplate::new(529)
                .set_body_string("overloaded")
                .insert_header("retry-after-ms", "60000"),
        )
        .expect(1) // only the initial attempt; the next delay exceeds the budget
        .mount(&server)
        .await;
    let client = Client::builder()
        .api_key("sk-test-key")
        .base_url(server.uri())
        .retry_policy(RetryPolicy {
            max_retries: 5,
            backoff_initial: Duration::from_millis(5),
            backoff_max: Duration::from_millis(5),
            backoff_jitter: 0.0,
            total_budget: Some(Duration::from_millis(500)),
            ..Default::default()
        })
        .build()
        .unwrap();
    let started = std::time::Instant::now();
    let error = client.system_one(simple_request()).await.unwrap_err();
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "budget stop retried a 60s delay: {:?}",
        started.elapsed()
    );
    match &error {
        Error::Api(api) => {
            assert_eq!(api.status, 529);
            assert_eq!(api.message, "overloaded");
            assert_eq!(api.retry_after, Some(Duration::from_secs(60)));
        }
        other => panic!("expected the last API error, got {other:?}"),
    }
    assert!(!error.is_timeout());
}

#[tokio::test]
async fn connection_errors_retried() {
    // Point at a closed localhost port.
    let client = Client::builder()
        .api_key("sk-test-key")
        .base_url("http://127.0.0.1:1")
        .retry_policy(fast_retry())
        .build()
        .unwrap();
    let error = client.system_one(simple_request()).await.unwrap_err();
    assert!(error.is_connection(), "{error:?}");
    assert!(!error.is_timeout(), "{error:?}");
}

#[tokio::test]
async fn connection_error_display_has_no_api_key() {
    let client = Client::builder()
        .api_key("sk-test-key")
        .base_url("http://127.0.0.1:1")
        .retry_policy(RetryPolicy::none())
        .build()
        .unwrap();
    let error = client.system_one(simple_request()).await.unwrap_err();
    let text = error.to_string();
    assert!(!text.contains("sk-test-key"), "{text}");
}

#[tokio::test]
async fn timeout_maps_to_timeout_error_and_retries() {
    // The server sleeps longer than the per-attempt timeout.
    let server = mock_server().await;
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(ok_body())
                .set_delay(Duration::from_millis(500)),
        )
        .expect(3) // retried: timeout errors retry by default
        .mount(&server)
        .await;
    let client = Client::builder()
        .api_key("sk-test-key")
        .base_url(server.uri())
        .timeout(Duration::from_millis(50))
        .retry_policy(fast_retry())
        .build()
        .unwrap();
    let error = client.system_one(simple_request()).await.unwrap_err();
    assert!(error.is_timeout(), "{error:?}");
    match &error {
        Error::Timeout { timeout } => assert_eq!(*timeout, Duration::from_millis(50)),
        other => panic!("expected Timeout, got {other:?}"),
    }
    // A timeout is a kind of connection error.
    assert!(error.is_connection());
}

#[tokio::test]
async fn timeouts_not_retried_when_disabled() {
    let server = mock_server().await;
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(ok_body())
                .set_delay(Duration::from_millis(500)),
        )
        .expect(1)
        .mount(&server)
        .await;
    let client = Client::builder()
        .api_key("sk-test-key")
        .base_url(server.uri())
        .timeout(Duration::from_millis(50))
        .retry_policy(RetryPolicy {
            retry_timeouts: false,
            ..fast_retry()
        })
        .build()
        .unwrap();
    let error = client.system_one(simple_request()).await.unwrap_err();
    assert!(error.is_timeout(), "{error:?}");
}

#[tokio::test]
async fn connection_errors_not_retried_when_disabled() {
    let client = Client::builder()
        .api_key("sk-test-key")
        .base_url("http://127.0.0.1:1")
        .retry_policy(RetryPolicy {
            retry_connection_errors: false,
            ..fast_retry()
        })
        .build()
        .unwrap();
    let error = client.system_one(simple_request()).await.unwrap_err();
    assert!(error.is_connection(), "{error:?}");
}

// ---------------------------------------------------------------------------
// list_models
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_models_decodes_and_retries() {
    let server = mock_server().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(503))
        .up_to_n_times(1)
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "models": [
                {"name": "jev-latest", "description": "General-purpose.", "release_date": "2026-09-15"},
                {"name": "jev-1.13.0", "description": "Pinned.", "release_date": "2026-09-10"}
            ]
        })))
        .mount(&server)
        .await;
    let client = Client::builder()
        .api_key("sk-test-key")
        .base_url(server.uri())
        .retry_policy(fast_retry())
        .build()
        .unwrap();
    let models = client.list_models().await.unwrap();
    assert_eq!(models.len(), 2);
    assert_eq!(models[0].name, "jev-latest");
    assert_eq!(models[1].release_date, "2026-09-10");
}

fn runtime_header() -> String {
    format!(
        "rust ({}; {})",
        std::env::consts::OS,
        std::env::consts::ARCH
    )
}
