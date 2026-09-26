//! Regression tests for issues found in adversarial review: per-call option
//! validation, answer completeness, and model field paths.

use std::time::Duration;

use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use typesafe_client::{Choice, Client, Error, Noul, RequestOptions, RetryPolicy, SystemOneRequest};

async fn setup(body: serde_json::Value) -> (MockServer, Client) {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body.clone()))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&server)
        .await;
    let client = Client::builder()
        .api_key("sk-test-key")
        .base_url(server.uri())
        .retry_policy(RetryPolicy::none())
        .build()
        .unwrap();
    (server, client)
}

fn two_questions() -> SystemOneRequest {
    SystemOneRequest::new("x")
        .question("a", Noul::new("A?"))
        .question("b", Choice::from_options("B?", ["x", "y"]))
}

fn noul_a() -> serde_json::Value {
    json!({"type": "noul", "noul": 0.2})
}

fn choice_b() -> serde_json::Value {
    json!({"type": "choice", "choice": "x", "probabilities": {"x": 1.0}, "confidence": 1.0})
}

fn field_path(error: &Error) -> &str {
    match error {
        Error::ResponseValidation { field_path, .. } => field_path,
        other => panic!("expected ResponseValidation, got {other:?}"),
    }
}

#[tokio::test]
async fn per_call_zero_timeout_is_invalid_request() {
    let (server, client) = setup(json!({})).await;
    let error = client
        .system_one_with(
            two_questions(),
            RequestOptions::new().timeout(Duration::ZERO),
        )
        .await
        .unwrap_err();
    assert!(matches!(error, Error::InvalidRequest(_)), "{error:?}");
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn per_call_invalid_jitter_is_invalid_request_not_panic() {
    let (server, client) = setup(json!({})).await;
    for jitter in [f64::NAN, -0.5, 1.5, f64::INFINITY] {
        let policy = RetryPolicy {
            backoff_jitter: jitter,
            ..RetryPolicy::default()
        };
        let error = client
            .system_one_with(two_questions(), RequestOptions::new().retry_policy(policy))
            .await
            .unwrap_err();
        assert!(
            matches!(error, Error::InvalidRequest(_)),
            "{jitter}: {error:?}"
        );
        let error = client
            .list_models_with(RequestOptions::new().retry_policy(RetryPolicy {
                backoff_jitter: jitter,
                ..RetryPolicy::default()
            }))
            .await
            .unwrap_err();
        assert!(
            matches!(error, Error::InvalidRequest(_)),
            "{jitter}: {error:?}"
        );
    }
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn complete_answers_and_extra_ids_accepted() {
    let (_server, client) = setup(json!({"model": "m", "answers": {
        "a": noul_a(), "b": choice_b(), "zzz": noul_a()
    }}))
    .await;
    let response = client.system_one(two_questions()).await.unwrap();
    assert!(response.noul("a").is_some());
    assert!(response.choice("b").is_some());
}

#[tokio::test]
async fn unknown_answer_type_passes_completeness() {
    let (_server, client) = setup(json!({"model": "m", "answers": {
        "a": {"type": "future_kind"}, "b": choice_b()
    }}))
    .await;
    client.system_one(two_questions()).await.unwrap();
}

#[tokio::test]
async fn missing_answer_is_validation_error() {
    let (_server, client) = setup(json!({"model": "m", "answers": {"b": choice_b()}})).await;
    let error = client.system_one(two_questions()).await.unwrap_err();
    assert_eq!(field_path(&error), "answers.a");
}

#[tokio::test]
async fn mismatched_answer_type_is_validation_error() {
    let (_server, client) =
        setup(json!({"model": "m", "answers": {"a": choice_b(), "b": choice_b()}})).await;
    let error = client.system_one(two_questions()).await.unwrap_err();
    assert_eq!(field_path(&error), "answers.a.type");
}

#[tokio::test]
async fn questions_override_skips_completeness() {
    let (_server, client) = setup(json!({"model": "m", "answers": {"other": noul_a()}})).await;
    client
        .system_one_with(
            two_questions(),
            RequestOptions::new().extra_body("questions", json!({"other": {"type": "noul"}})),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn overflowing_retry_after_does_not_panic() {
    // Duration::from_secs_f64 panics on overflow; these headers are
    // server-controlled, so they must be treated as absent (falling back to
    // backoff), never crash the caller.
    for (name, value) in [
        ("retry-after-ms", "1e300"),
        ("retry-after", "1e300"),
        ("retry-after", "18446744073709551616"),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/systemone"))
            .respond_with(ResponseTemplate::new(429).insert_header(name, value))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/systemone"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "model": "m", "answers": {"a": noul_a(), "b": choice_b()}
            })))
            .mount(&server)
            .await;
        let client = Client::builder()
            .api_key("sk-test-key")
            .base_url(server.uri())
            .retry_policy(RetryPolicy {
                backoff_initial: Duration::from_millis(1),
                backoff_max: Duration::from_millis(2),
                ..RetryPolicy::default()
            })
            .build()
            .unwrap();
        client
            .system_one(two_questions())
            .await
            .unwrap_or_else(|e| panic!("{name}: {value}: {e:?}"));
    }
}

#[test]
fn huge_backoff_max_saturates_instead_of_panicking() {
    let policy = RetryPolicy {
        backoff_initial: Duration::MAX,
        backoff_max: Duration::MAX,
        backoff_jitter: 0.0,
        ..RetryPolicy::default()
    };
    assert_eq!(policy.backoff_delay(3, 0.0), Duration::MAX);
}

#[test]
fn base_url_validation() {
    for bad in [
        "api.internal",
        "ftp://api.example",
        "https://gw.example/typesafe?key=1",
        "https://gw.example/?",
        "https://gw.example/#frag",
        "not a url",
    ] {
        let error = Client::builder()
            .api_key("sk-test-key")
            .base_url(bad)
            .build()
            .unwrap_err();
        assert!(matches!(error, Error::Config(_)), "{bad}: {error:?}");
    }
    for good in [
        "http://localhost:8080",
        "https://gw.example/typesafe/",
        "https://ai-gateway.vercel.sh/typesafe",
    ] {
        Client::builder()
            .api_key("sk-test-key")
            .base_url(good)
            .build()
            .unwrap_or_else(|e| panic!("{good}: {e:?}"));
    }
}

#[tokio::test]
async fn usage_and_legend_value_types_are_validated() {
    let score = |legend: serde_json::Value| {
        json!({"type": "score", "score": 1.0, "confidence": 1.0,
               "legend": legend, "probabilities": {"0": 1.0}})
    };
    let request = || {
        SystemOneRequest::new("x").question("s", typesafe_client::Score::new("S?", ["lo", "hi"]))
    };
    let cases = [
        (
            json!({"model": "m", "answers": {"s": score(json!({"0": "lo"}))}, "usage": {"input_tokens": "many"}}),
            "usage.input_tokens",
        ),
        (
            json!({"model": "m", "answers": {"s": score(json!({"0": "lo"}))}, "usage": {"output_tokens": -1}}),
            "usage.output_tokens",
        ),
        (
            json!({"model": "m", "answers": {"s": score(json!({"0": "lo"}))}, "usage": "lots"}),
            "usage",
        ),
        (
            json!({"model": "m", "answers": {"s": score(json!({"0": "lo", "1": true}))}}),
            "answers.s.legend.1",
        ),
        (
            json!({"model": "m", "answers": {"s": score(json!({"0": 3}))}}),
            "answers.s.legend.0",
        ),
    ];
    for (body, want) in cases {
        let (_server, client) = setup(body).await;
        let error = client.system_one(request()).await.unwrap_err();
        assert_eq!(field_path(&error), want);
    }
    // Absent or null usage, and absent fields, still default to 0.
    for usage in [json!(null), json!({}), json!({"input_tokens": null})] {
        let (_server, client) =
            setup(json!({"model": "m", "answers": {"s": score(json!({"0": ["structured"]}))}, "usage": usage})).await;
        let response = client.system_one(request()).await.unwrap();
        assert_eq!(response.usage.input_tokens, 0);
    }
}

#[tokio::test]
async fn model_field_paths_use_brackets_and_name_the_field() {
    let (_server, client) = setup(json!({"models": [
        {"name": "a", "description": "b", "release_date": "c"},
        {"name": "a", "description": "b"}
    ]}))
    .await;
    let error = client.list_models().await.unwrap_err();
    assert_eq!(field_path(&error), "models[1].release_date");

    let (_server, client) =
        setup(json!({"models": [{"name": 7, "description": "b", "release_date": "c"}]})).await;
    let error = client.list_models().await.unwrap_err();
    assert_eq!(field_path(&error), "models[0].name");

    let (_server, client) = setup(json!({"models": ["nope"]})).await;
    let error = client.list_models().await.unwrap_err();
    assert_eq!(field_path(&error), "models[0]");
}
