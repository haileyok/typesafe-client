//! Errors, API error kinds, message extraction, and error formatting.
//!
//! The error string for a non-2xx response is exactly
//! `"<METHOD> <URL>: <status> <message> (request_id=<id>)"`, where the
//! request-ID suffix appears only when the `x-typesafe-request-id` header is
//! present. The API key never appears in any error.

use std::fmt;
use std::time::Duration;

use serde_json::Value;

use crate::REQUEST_ID_HEADER;

/// The maximum length of a raw body embedded in an error message before truncation.
const MAX_RAW_BODY_IN_MESSAGE: usize = 200;

/// A request failed because the HTTP response carried an error status.
///
/// The `Display` format is `"<METHOD> <URL>: <status> <message> (request_id=<id>)"`,
/// with the request-ID suffix only when present.
#[derive(Debug, Clone)]
pub struct ApiError {
    /// HTTP response status code.
    pub status: u16,
    /// The status-derived error kind.
    pub kind: ApiErrorKind,
    /// The extracted or synthesized message (never the raw body over 200 chars).
    pub message: String,
    /// The server's JSON error body when parseable, the raw text otherwise,
    /// and `None` when the body was empty.
    pub body: Option<Value>,
    /// Response headers (the API key is never present in responses).
    pub headers: reqwest::header::HeaderMap,
    /// `x-typesafe-request-id` response header value, when present.
    pub request_id: Option<String>,
    /// `"<METHOD> <URL>"` of the request, without query parameters.
    pub endpoint: String,
    /// Parsed `retry-after-ms` / `Retry-After` delay, when present.
    pub retry_after: Option<Duration>,
}

/// The kind of API error, derived from the HTTP status code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ApiErrorKind {
    /// 400: the request was invalid.
    BadRequest,
    /// 401: authentication failed.
    Authentication,
    /// 403: access was denied.
    PermissionDenied,
    /// 404: the resource was not found.
    NotFound,
    /// 422: the request failed server validation.
    UnprocessableEntity,
    /// 429: the rate limit was exceeded.
    RateLimit,
    /// 5xx: the server failed to process the request.
    InternalServer,
    /// Any other status.
    Other,
}

impl ApiErrorKind {
    /// Maps an HTTP status code to its error kind.
    pub fn from_status(status: u16) -> Self {
        match status {
            400 => Self::BadRequest,
            401 => Self::Authentication,
            403 => Self::PermissionDenied,
            404 => Self::NotFound,
            422 => Self::UnprocessableEntity,
            429 => Self::RateLimit,
            s if (500..=599).contains(&s) => Self::InternalServer,
            _ => Self::Other,
        }
    }
}

impl fmt::Display for ApiErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::BadRequest => "bad request",
            Self::Authentication => "authentication",
            Self::PermissionDenied => "permission denied",
            Self::NotFound => "not found",
            Self::UnprocessableEntity => "unprocessable entity",
            Self::RateLimit => "rate limit",
            Self::InternalServer => "internal server",
            Self::Other => "other",
        };
        f.write_str(name)
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {} {}", self.endpoint, self.status, self.message)?;
        if let Some(request_id) = &self.request_id {
            write!(f, " (request_id={request_id})")?;
        }
        Ok(())
    }
}

impl std::error::Error for ApiError {}

/// Extracts a message from a text, error, or validation response body.
///
/// First match wins: non-empty string body (truncated to 200 Unicode scalar
/// values plus `…`); `error` (string); `error.message`; `message`; `detail`
/// (string); `detail.message`; `detail[]` as FastAPI validation errors
/// rendered as `"<loc without 'body' joined by '.'>: <msg>"` joined by `"; "`.
/// Returns `None` otherwise.
///
/// The plain-string body is truncated deliberately, unlike the official
/// SDKs: gateway error pages can be huge.
pub(super) fn extract_message(body: &Value) -> Option<String> {
    match body {
        Value::String(s) => {
            if s.is_empty() {
                return None;
            }
            Some(truncate_chars(s))
        }
        Value::Object(map) => {
            let error = map.get("error");
            if let Some(Value::String(error)) = error {
                return Some(error.clone());
            }
            if let Some(Value::Object(error)) = error {
                if let Some(Value::String(message)) = error.get("message") {
                    return Some(message.clone());
                }
            }
            if let Some(Value::String(message)) = map.get("message") {
                return Some(message.clone());
            }
            let detail = map.get("detail");
            if let Some(Value::String(detail)) = detail {
                return Some(detail.clone());
            }
            if let Some(Value::Object(detail)) = detail {
                if let Some(Value::String(message)) = detail.get("message") {
                    return Some(message.clone());
                }
            }
            if let Some(Value::Array(entries)) = detail {
                let mut parts = Vec::new();
                for entry in entries {
                    let Value::Object(entry) = entry else {
                        continue;
                    };
                    let Some(Value::String(msg)) = entry.get("msg") else {
                        continue;
                    };
                    let path = match entry.get("loc") {
                        Some(Value::Array(location)) => location
                            .iter()
                            .filter(|item| **item != Value::String("body".into()))
                            .map(|item| match item {
                                Value::String(s) => s.clone(),
                                other => other.to_string(),
                            })
                            .collect::<Vec<_>>()
                            .join("."),
                        _ => String::new(),
                    };
                    parts.push(if path.is_empty() {
                        msg.clone()
                    } else {
                        format!("{path}: {msg}")
                    });
                }
                if !parts.is_empty() {
                    return Some(parts.join("; "));
                }
            }
            None
        }
        _ => None,
    }
}

/// Truncates a string to 200 Unicode scalar values, appending `…` when
/// truncation happened.
fn truncate_chars(raw: &str) -> String {
    if raw.chars().count() > MAX_RAW_BODY_IN_MESSAGE {
        let mut truncated: String = raw.chars().take(MAX_RAW_BODY_IN_MESSAGE).collect();
        truncated.push('…');
        truncated
    } else {
        raw.to_owned()
    }
}

/// Renders a body for embedding in an error message: the raw text or
/// compact JSON, truncated to 200 characters plus `…`.
pub(super) fn truncated_body(body: &Value) -> String {
    let raw = match body {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    truncate_chars(&raw)
}

/// Constructs the message for an API error from its parsed body.
///
/// When extraction finds nothing, the raw body (truncated) is used; an empty
/// body yields `"status code (no body)"`.
pub(super) fn api_error_message(body: &Option<Value>) -> String {
    if let Some(body) = body {
        if let Some(message) = extract_message(body) {
            return message;
        }
        if matches!(body, Value::String(s) if s.is_empty()) || (body.is_null() && false) {
            return "status code (no body)".into();
        }
        return truncated_body(body);
    }
    "status code (no body)".into()
}

/// Every error this crate can return.
///
/// The API key never appears in any error. The top-level error is
/// `#[non_exhaustive]`: match on the variants you handle and keep a wildcard
/// arm for forward compatibility.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// The client could not be constructed: a missing, empty, or invalid API
    /// key, an invalid timeout, or invalid retry policy settings.
    Config(String),
    /// The request failed client-side validation before any network I/O.
    InvalidRequest(String),
    /// The server returned a non-2xx response.
    Api(Box<ApiError>),
    /// The error variant: no HTTP response arrived.
    Connection {
        /// Description of the transport failure.
        message: String,
        /// The underlying transport error, when available.
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },
    /// The attempt exceeded the per-attempt timeout. A kind of connection error.
    Timeout {
        /// The per-attempt timeout that expired.
        timeout: Duration,
    },
    /// A 2xx response body did not match the expected schema.
    ResponseValidation {
        /// HTTP status of the offending response.
        status: u16,
        /// Dotted path to the first missing or invalid field, e.g.
        /// `answers.tone.confidence`; `""` for a non-object body.
        field_path: String,
        /// `x-typesafe-request-id` of the offending response, when present.
        request_id: Option<String>,
        /// `"<METHOD> <URL>"` of the request.
        endpoint: String,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(message) => write!(f, "configuration error: {message}"),
            Self::InvalidRequest(message) => write!(f, "invalid request: {message}"),
            Self::Api(error) => write!(f, "{error}"),
            Self::Connection { message, .. } => write!(f, "connection error: {message}"),
            Self::Timeout { timeout } => {
                write!(f, "request timed out (timeout={:?})", timeout)
            }
            Self::ResponseValidation { field_path, .. } => {
                write!(f, "invalid response data at '{field_path}'")
            }
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Api(error) => Some(error.as_ref()),
            Self::Connection {
                source: Some(source),
                ..
            } => Some(source.as_ref()),
            _ => None,
        }
    }
}

impl From<ApiError> for Error {
    fn from(error: ApiError) -> Self {
        Self::Api(Box::new(error))
    }
}

impl Error {
    /// Returns `true` when this error is a timeout.
    pub fn is_timeout(&self) -> bool {
        matches!(self, Self::Timeout { .. })
    }

    /// Returns `true` when this error is a connection error. Timeouts count
    /// as connection errors.
    pub fn is_connection(&self) -> bool {
        matches!(self, Self::Connection { .. } | Self::Timeout { .. })
    }

    /// Returns the HTTP status code when this error came from a response.
    pub fn status(&self) -> Option<u16> {
        match self {
            Self::Api(error) => Some(error.status),
            Self::ResponseValidation { status, .. } => Some(*status),
            _ => None,
        }
    }

    /// Returns the `x-typesafe-request-id` when this error came from a response.
    pub fn request_id(&self) -> Option<&str> {
        match self {
            Self::Api(error) => error.request_id.as_deref(),
            Self::ResponseValidation { request_id, .. } => request_id.as_deref(),
            _ => None,
        }
    }

    /// Returns the underlying [`ApiError`] when this is a non-2xx response error.
    pub fn as_api(&self) -> Option<&ApiError> {
        match self {
            Self::Api(error) => Some(error),
            _ => None,
        }
    }
}

/// Reads `x-typesafe-request-id` from a response header map.
pub(super) fn request_id_of(headers: &reqwest::header::HeaderMap) -> Option<String> {
    headers
        .get(REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn api_error(status: u16, body: Value, request_id: Option<&str>) -> ApiError {
        let mut headers = reqwest::header::HeaderMap::new();
        if let Some(request_id) = request_id {
            headers.insert(
                REQUEST_ID_HEADER,
                reqwest::header::HeaderValue::from_str(request_id).unwrap(),
            );
        }
        let body = if body.is_null() { None } else { Some(body) };
        ApiError {
            status,
            kind: ApiErrorKind::from_status(status),
            message: api_error_message(&body),
            body,
            headers,
            request_id: request_id.map(str::to_owned),
            endpoint: "POST https://api.typesafe.ai/v1/systemone".into(),
            retry_after: None,
        }
    }

    #[test]
    fn kind_per_status() {
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
            assert_eq!(ApiErrorKind::from_status(status), kind, "{status}");
        }
    }

    #[test]
    fn message_extraction_first_match_wins() {
        let cases = [
            (json!("plain string"), "plain string"),
            (json!({"error": "an error"}), "an error"),
            (
                json!({"error": {"message": "nested"}, "message": "flat"}),
                "nested",
            ),
            (json!({"message": "flat"}), "flat"),
            (json!({"detail": "details"}), "details"),
            (
                json!({"detail": {"message": "detail message"}}),
                "detail message",
            ),
            (
                json!({"detail": [
                    {"loc": ["body", "questions", "urgency", "score", "criteria"], "msg": "Field required"},
                    {"loc": ["body", "state"], "msg": "Bad state"}
                ]}),
                "questions.urgency.score.criteria: Field required; state: Bad state",
            ),
        ];
        for (body, expected) in cases {
            assert_eq!(extract_message(&body).as_deref(), Some(expected), "{body}");
        }
    }

    #[test]
    fn plain_string_body_is_truncated_to_200_chars() {
        // A JSON string body is truncated to 200 Unicode scalar values + "…",
        // even when its byte length differs wildly (multi-byte characters).
        let long = "é".repeat(250);
        let message = extract_message(&Value::String(long.clone())).unwrap();
        assert_eq!(message.chars().count(), 201);
        assert!(message.ends_with('…'));
        assert_eq!(message.chars().filter(|c| *c == 'é').count(), 200);
        assert_eq!(message.len(), 200 * 2 + 3); // é is two bytes in UTF-8.

        // Exactly 200 chars is not truncated.
        let exact = "é".repeat(200);
        let message = extract_message(&Value::String(exact.clone())).unwrap();
        assert_eq!(message, exact);

        // Non-JSON raw text parses to a JSON string at the transport layer
        // and truncates the same way.
        let raw_html = format!("<html>{}</html>", "z".repeat(300));
        let message = extract_message(&Value::String(raw_html)).unwrap();
        assert_eq!(message.chars().count(), 201);
        assert!(message.ends_with('…'));
    }

    #[test]
    fn fastapi_detail_renders_loc_without_body() {
        let error = api_error(
            422,
            json!({"detail": [
                {"loc": ["body", "questions", "urgency", "score", "criteria"], "msg": "Field required", "type": "missing"}
            ]}),
            None,
        );
        assert_eq!(
            error.message,
            "questions.urgency.score.criteria: Field required"
        );
    }

    #[test]
    fn message_falls_back_to_raw_body_truncated() {
        let long = "x".repeat(300);
        let error = api_error(500, json!({"unexpected": long.clone()}), None);
        assert_eq!(error.message.chars().count(), 201);
        assert!(error.message.ends_with('…'));
        assert!(error.message.starts_with("{\"unexpected\":\"xxx"));
        // A body whose compact JSON is exactly 200 chars is not truncated.
        // {"unexpected":"..."} with 183 inner chars = 200 total.
        let exact = "y".repeat(183);
        let error = api_error(500, json!({"unexpected": exact}), None);
        assert_eq!(
            error.message,
            "{\"unexpected\":\"".to_owned() + &"y".repeat(183) + "\"}"
        );
        // Raw non-JSON text is the string body itself, truncated the same way.
        let raw = "z".repeat(250);
        let error = api_error(500, Value::String(raw.clone()), None);
        assert_eq!(error.message, "z".repeat(200) + "…");
    }

    #[test]
    fn empty_body_message() {
        // Empty body: Value::String("") parses to None at the transport layer;
        // simulate that with None.
        let error = api_error(404, Value::Null, None);
        assert_eq!(error.message, "status code (no body)");
        // Non-empty string body uses the string.
        let error = api_error(404, json!("whoops"), None);
        assert_eq!(error.message, "whoops");
    }

    #[test]
    fn display_format() {
        let error = api_error(429, json!({"error": "too fast"}), Some("req-123"));
        assert_eq!(
            error.to_string(),
            "POST https://api.typesafe.ai/v1/systemone: 429 too fast (request_id=req-123)"
        );
        let error = api_error(500, json!("boom"), None);
        assert_eq!(
            error.to_string(),
            "POST https://api.typesafe.ai/v1/systemone: 500 boom"
        );
    }

    #[test]
    fn error_helpers() {
        let error = Error::from(api_error(429, json!("rl"), Some("req-1")));
        assert_eq!(error.status(), Some(429));
        assert_eq!(error.request_id(), Some("req-1"));
        assert!(error.as_api().is_some());
        assert!(!error.is_connection());
        assert!(!error.is_timeout());

        let error = Error::Timeout {
            timeout: Duration::from_secs(2),
        };
        assert!(error.is_timeout());
        assert!(error.is_connection());
        assert_eq!(error.status(), None);

        let error = Error::Connection {
            message: "reset".into(),
            source: None,
        };
        assert!(error.is_connection());
        assert!(!error.is_timeout());
    }
}
