//! The [`Client`], its builder, per-call [`RequestOptions`], and the retrying
//! transport loop.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::answers::{decode_models, ModelMetadata, SystemOneResponse};
use crate::call_options::{CallOptions, ResolvedCall};
use crate::config::{
    resolve_api_key, resolve_base_url, resolve_default_model, validate_timeout, LogLevel,
};
use crate::errors::{api_error_message, request_id_of, ApiError, ApiErrorKind, Error};
use crate::logging;
use crate::request::SystemOneRequest;
use crate::retry::RetryPolicy;

/// One completed HTTP response, before decoding.
pub(crate) struct RawResponse {
    /// HTTP status code.
    pub status: u16,
    /// The response body as text.
    pub body: String,
    /// `x-typesafe-request-id`, when present.
    pub request_id: Option<String>,
    /// `"<METHOD> <URL>"` of the request, without query parameters.
    pub endpoint: String,
}

/// Per-call options that override the client's settings for one call.
///
/// A per-call retry policy fully replaces the client's policy for that call.
///
/// ```
/// # use std::time::Duration;
/// # use typesafe_client::{Client, RequestOptions, RetryPolicy, SystemOneRequest, Noul};
/// # fn demo(client: Client) {
/// let options = RequestOptions::new()
///     .timeout(Duration::from_secs(30))
///     .retry_policy(RetryPolicy::none())
///     .header("X-Agent-Client", "my-app")
///     .extra_body("foo", serde_json::json!(1));
/// let request = SystemOneRequest::new("text").question("a", Noul::new("q"));
/// # let _ = async {
/// let response = client
///     .system_one_with(request, options)
///     .await?;
/// # Ok::<_, typesafe_client::Error>(response)
/// # };
/// # }
/// ```
#[derive(Debug, Clone, Default)]
pub struct RequestOptions {
    pub(crate) options: CallOptions,
}

impl RequestOptions {
    /// Creates empty options; every setting falls back to the client's.
    pub fn new() -> Self {
        Self::default()
    }

    /// Overrides the per-attempt timeout for this call. Must be > 0.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.options.timeout = Some(timeout);
        self
    }

    /// Replaces the retry policy for this call.
    pub fn retry_policy(mut self, policy: RetryPolicy) -> Self {
        self.options.retry_policy = Some(policy);
        self
    }

    /// Adds a header for this call. SDK-owned headers always win.
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.options.headers.insert(name.into(), value.into());
        self
    }

    /// Merges an extra field into the JSON request body for this call.
    ///
    /// Existing fields with the same name are replaced. The extra fields are
    /// included in what is sent, but not parsed by this client.
    pub fn extra_body(mut self, name: impl Into<String>, value: Value) -> Self {
        self.options
            .extra_body
            .get_or_insert_with(serde_json::Map::new)
            .insert(name.into(), value);
        self
    }
}

/// The resolved inner configuration, shared via [`Arc`].
struct Inner {
    api_key: String,
    base_url: String,
    default_model: String,
    timeout: Duration,
    retry_policy: RetryPolicy,
    default_headers: BTreeMap<String, String>,
    log_level: LogLevel,
    http: reqwest::Client,
}

/// An async client for the TypeSafe System One API.
///
/// Construct with [`Client::builder`] or [`Client::from_env`]. The client is
/// [`Clone`] and cheap to clone (an [`Arc`] inside), and `Send + Sync`.
///
/// Every method is async; there is no blocking client. Cancellation is
/// dropping the returned future.
///
/// ```
/// # use typesafe_client::Client;
/// # fn demo() -> Result<(), typesafe_client::Error> {
/// let client = Client::builder()
///     .api_key("sk-live-...")
///     .default_header("X-Agent-Client", "my-app")
///     .build()?;
/// # Ok(())
/// # }
/// ```
#[derive(Clone)]
pub struct Client {
    inner: Arc<Inner>,
}

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Never include the API key.
        f.debug_struct("Client")
            .field("base_url", &self.inner.base_url)
            .field("default_model", &self.inner.default_model)
            .field("timeout", &self.inner.timeout)
            .field("retry_policy", &self.inner.retry_policy)
            .field("default_headers", &self.inner.default_headers)
            .field("log_level", &self.inner.log_level)
            .finish_non_exhaustive()
    }
}

/// A builder for [`Client`].
///
/// Explicit options override environment variables, and environment
/// variables override defaults. Environment values are trimmed, and blank
/// values are ignored. [`ClientBuilder::build`] returns
/// [`Error::Config`](crate::Error::Config) for a missing or invalid API key
/// or an invalid setting.
#[derive(Default)]
pub struct ClientBuilder {
    api_key: Option<String>,
    base_url: Option<String>,
    default_model: Option<String>,
    timeout: Option<Duration>,
    retry_policy: Option<RetryPolicy>,
    default_headers: BTreeMap<String, String>,
    log_level: Option<LogLevel>,
    http_client: Option<reqwest::Client>,
}

impl fmt::Debug for ClientBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Never include the API key, even masked.
        f.debug_struct("ClientBuilder")
            .field("base_url", &self.base_url)
            .field("default_model", &self.default_model)
            .field("timeout", &self.timeout)
            .field("retry_policy", &self.retry_policy)
            .field("default_headers", &self.default_headers)
            .field("log_level", &self.log_level)
            .field(
                "http_client",
                &self.http_client.as_ref().map(|_| "reqwest::Client"),
            )
            .finish()
    }
}

impl ClientBuilder {
    /// Sets the API key. Else `TYPESAFE_API_KEY`. Required.
    ///
    /// The key is trimmed and validated: empty keys and keys containing
    /// whitespace, control characters, or non-ASCII characters are rejected
    /// with [`Error::Config`], without echoing the key.
    pub fn api_key(mut self, api_key: impl Into<String>) -> Self {
        self.api_key = Some(api_key.into());
        self
    }

    /// Sets the base URL. Else `TYPESAFE_BASE_URL`, else
    /// `https://api.typesafe.ai`. A trailing `/` is stripped.
    ///
    /// Any base URL that implements the TypeSafe OpenAPI spec works, for
    /// example a gateway.
    pub fn base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = Some(base_url.into());
        self
    }

    /// Sets the model used when a request does not specify one. Else
    /// `TYPESAFE_DEFAULT_MODEL`, else `jev-latest`.
    pub fn default_model(mut self, model: impl Into<String>) -> Self {
        self.default_model = Some(model.into());
        self
    }

    /// Sets the per-attempt timeout, which covers connecting through
    /// reading the full response body. Defaults to 10 seconds. Must be > 0.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Sets the default retry policy. Defaults to [`RetryPolicy::default`].
    pub fn retry_policy(mut self, policy: RetryPolicy) -> Self {
        self.retry_policy = Some(policy);
        self
    }

    /// Adds a default header sent on every request, for gateway attribution.
    ///
    /// SDK-owned headers (`Authorization`, `Accept`, `Content-Type`,
    /// `User-Agent`, `X-TypeSafe-SDK`, `X-TypeSafe-Runtime`,
    /// `X-TypeSafe-Retry-Count`) always take precedence over caller headers.
    pub fn default_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.default_headers.insert(name.into(), value.into());
        self
    }

    /// Sets the log level. Else `TYPESAFE_LOG_LEVEL`, else off.
    pub fn log_level(mut self, level: LogLevel) -> Self {
        self.log_level = Some(level);
        self
    }

    /// Supplies a preconfigured `reqwest` client. One is built otherwise.
    pub fn http_client(mut self, http: reqwest::Client) -> Self {
        self.http_client = Some(http);
        self
    }

    /// Builds the client, validating all settings.
    pub fn build(self) -> Result<Client, Error> {
        let api_key = resolve_api_key(self.api_key.as_deref())?;
        let base_url = resolve_base_url(self.base_url.as_deref());
        validate_base_url(&base_url)?;
        let default_model = resolve_default_model(self.default_model.as_deref());
        let timeout = match self.timeout {
            Some(timeout) => validate_timeout(timeout)?,
            None => crate::DEFAULT_TIMEOUT,
        };
        let retry_policy = self.retry_policy.unwrap_or_default();
        retry_policy.validate()?;
        let log_level = LogLevel::resolve(self.log_level);
        let http = self.http_client.unwrap_or_default();
        Ok(Client {
            inner: Arc::new(Inner {
                api_key,
                base_url,
                default_model,
                timeout,
                retry_policy,
                default_headers: self.default_headers,
                log_level,
                http,
            }),
        })
    }
}

impl Client {
    /// Creates a builder.
    pub fn builder() -> ClientBuilder {
        ClientBuilder::default()
    }

    /// Creates a client from `TYPESAFE_API_KEY` (and the other
    /// `TYPESAFE_*` variables), exactly like
    /// `Client::builder().build()`.
    pub fn from_env() -> Result<Self, Error> {
        Self::builder().build()
    }

    /// The configured base URL.
    pub fn base_url(&self) -> &str {
        &self.inner.base_url
    }

    /// The configured default model.
    pub fn default_model(&self) -> &str {
        &self.inner.default_model
    }

    /// The configured per-attempt timeout.
    pub fn timeout(&self) -> Duration {
        self.inner.timeout
    }

    /// The configured default retry policy.
    pub fn retry_policy(&self) -> &RetryPolicy {
        &self.inner.retry_policy
    }

    /// The configured log level.
    pub fn log_level(&self) -> LogLevel {
        self.inner.log_level
    }

    /// Answers the questions in `request` about its state.
    ///
    /// Client-side validation runs before any network I/O; failures return
    /// [`Error::InvalidRequest`](crate::Error::InvalidRequest). Retries,
    /// per the client's default policy, are automatic.
    pub async fn system_one(&self, request: SystemOneRequest) -> Result<SystemOneResponse, Error> {
        self.system_one_with(request, RequestOptions::new()).await
    }

    /// Answers the questions in `request` with per-call options.
    ///
    /// A per-call retry policy fully replaces the client's policy for this
    /// call.
    pub async fn system_one_with(
        &self,
        request: SystemOneRequest,
        options: RequestOptions,
    ) -> Result<SystemOneResponse, Error> {
        request.validate()?;
        let call = self.resolve_call(options.options)?;
        let model = request
            .model
            .clone()
            .unwrap_or_else(|| self.inner.default_model.clone());
        let mut body = serde_json::Map::new();
        body.insert("state".into(), request.state.clone());
        body.insert("model".into(), Value::String(model));
        body.insert(
            "questions".into(),
            serde_json::to_value(&request.questions).map_err(|error| {
                Error::InvalidRequest(format!("questions failed to serialize: {error}"))
            })?,
        );
        if let Some(extra) = call.extra_body.clone() {
            for (name, value) in extra {
                body.insert(name, value);
            }
        }
        let path = "/v1/systemone";
        let url = format!("{}{}", self.inner.base_url, path);
        let raw = self
            .execute(
                reqwest::Method::POST,
                &url,
                path,
                Some(&Value::Object(body)),
                &call,
            )
            .await?;
        let response = SystemOneResponse::decode(&raw)?;
        // Completeness is checked against the questions actually sent, which
        // an extra-body "questions" override replaces; skip the check then.
        let overridden = call
            .extra_body
            .as_ref()
            .is_some_and(|extra| extra.iter().any(|(name, _)| name == "questions"));
        if !overridden {
            crate::answers::check_complete(&raw, &response.answers, &request.questions)?;
        }
        Ok(response)
    }

    /// Lists the models and aliases available to the authenticated account.
    pub async fn list_models(&self) -> Result<Vec<ModelMetadata>, Error> {
        self.list_models_with(RequestOptions::new()).await
    }

    /// Lists the models with per-call options.
    ///
    /// A per-call retry policy fully replaces the client's policy for this
    /// call. `extra_body` has no effect here (the request has no body).
    pub async fn list_models_with(
        &self,
        options: RequestOptions,
    ) -> Result<Vec<ModelMetadata>, Error> {
        let call = self.resolve_call(options.options)?;
        let path = "/v1/models";
        let url = format!("{}{}", self.inner.base_url, path);
        let raw = self
            .execute(reqwest::Method::GET, &url, path, None, &call)
            .await?;
        decode_models(&raw)
    }

    /// Merges per-call options over the client's settings. Per-call values
    /// get the same validation as the builder's (a zero timeout or an
    /// out-of-range jitter would otherwise fire instantly or panic in the
    /// backoff math), reported as [`Error::InvalidRequest`].
    fn resolve_call(&self, options: CallOptions) -> Result<ResolvedCall, Error> {
        let per_call = |error: Error| match error {
            Error::Config(message) => Error::InvalidRequest(format!("per-call option: {message}")),
            other => other,
        };
        let timeout = match options.timeout {
            Some(timeout) => crate::config::validate_timeout(timeout).map_err(per_call)?,
            None => self.inner.timeout,
        };
        let retry_policy = match options.retry_policy {
            Some(policy) => {
                policy.validate().map_err(per_call)?;
                policy
            }
            None => self.inner.retry_policy.clone(),
        };
        Ok(ResolvedCall {
            timeout,
            retry_policy,
            headers: options.headers,
            extra_body: options.extra_body,
        })
    }

    /// The full request/response exchange with retries.
    ///
    /// This is the heart of the client: header assembly with precedence,
    /// per-attempt timeout, error mapping, retry delays, budget stop
    /// conditions, and logging.
    async fn execute(
        &self,
        method: reqwest::Method,
        url: &str,
        path: &str,
        body: Option<&Value>,
        call: &ResolvedCall,
    ) -> Result<RawResponse, Error> {
        let endpoint = format!("{method} {url}");
        // Base headers: client defaults, then per-call headers, then
        // SDK-owned headers. Header names are case-insensitive, so keys are
        // lowercased before insertion: otherwise `authorization` and
        // `Authorization` would be distinct map entries, and whichever sorted
        // last would win in the HeaderMap, letting a caller header override
        // an SDK-owned one. A caller-supplied retry count is dropped.
        let mut headers: BTreeMap<String, String> = BTreeMap::new();
        for (name, value) in self.inner.default_headers.iter().chain(call.headers.iter()) {
            headers.insert(name.to_ascii_lowercase(), value.clone());
        }
        headers.remove("x-typesafe-retry-count");
        if body.is_none() {
            headers.remove("content-type");
        }
        headers.insert(
            "authorization".into(),
            format!("Bearer {}", self.inner.api_key),
        );
        headers.insert("accept".into(), "application/json".into());
        if body.is_some() {
            headers.insert("content-type".into(), "application/json".into());
        }
        headers.insert("user-agent".into(), sdk_version_header());
        headers.insert("x-typesafe-sdk".into(), sdk_version_header());
        headers.insert("x-typesafe-runtime".into(), runtime_header());

        let policy = call.retry_policy.clone();
        let total_budget = policy.total_budget.filter(|budget| !budget.is_zero());
        let call_start = Instant::now();
        let mut last_error: Option<Error> = None;

        for retry_number in 0..=policy.max_retries {
            let mut attempt_headers = headers.clone();
            if retry_number > 0 {
                attempt_headers.insert("x-typesafe-retry-count".into(), retry_number.to_string());
            }
            let reqwest_headers = build_header_map(&attempt_headers)?;
            if self.inner.log_level >= LogLevel::Debug {
                logging::log_request_debug(method.as_str(), path, &reqwest_headers, body);
            }

            let started = Instant::now();
            let mut attempt = self
                .inner
                .http
                .request(method.clone(), url)
                .headers(reqwest_headers.clone())
                .timeout(call.timeout);
            if let Some(body) = body {
                attempt = attempt.body(body.to_string());
            }
            let result = attempt.send().await;
            let response = match result {
                Ok(response) => response,
                Err(error) => {
                    let error: Error = if error.is_timeout() {
                        Error::Timeout {
                            timeout: call.timeout,
                        }
                    } else {
                        // Connect errors, resets, TLS failures, and body
                        // read failures are all connection errors.
                        Error::Connection {
                            message: format!("Connection error: {error}"),
                            source: Some(Box::new(error)),
                        }
                    };
                    if self.inner.log_level >= LogLevel::Info {
                        logging::log_attempt_error(method.as_str(), path, &error.to_string());
                    }
                    last_error = Some(error);
                    // Fall through to the retry decision below.
                    if !should_retry(&policy, &last_error, retry_number) {
                        return Err(last_error.unwrap());
                    }
                    let delay = policy.delay_for_retry(retry_number, None);
                    if !delay_within_budget(&delay, &total_budget, call_start) {
                        // Return the last real error, not an artificial timeout.
                        return Err(last_error.unwrap());
                    }
                    if self.inner.log_level >= LogLevel::Info {
                        logging::log_retry_scheduled(
                            method.as_str(),
                            path,
                            delay,
                            retry_number + 1,
                            &last_error.as_ref().unwrap().to_string(),
                        );
                    }
                    tokio::time::sleep(delay).await;
                    continue;
                }
            };

            let status = response.status().as_u16();
            let response_headers = response.headers().clone();
            let request_id = request_id_of(&response_headers);
            let text = match response.text().await {
                Ok(text) => text,
                // Body read failure: a timeout when the per-attempt timeout
                // (which covers the body) fired, else a connection error.
                Err(error) => {
                    let error = if error.is_timeout() {
                        Error::Timeout {
                            timeout: call.timeout,
                        }
                    } else {
                        Error::Connection {
                            message: format!("Connection error: {error}"),
                            source: Some(Box::new(error)),
                        }
                    };
                    if self.inner.log_level >= LogLevel::Info {
                        logging::log_attempt_error(method.as_str(), path, &error.to_string());
                    }
                    last_error = Some(error);
                    if !should_retry(&policy, &last_error, retry_number) {
                        return Err(last_error.unwrap());
                    }
                    let delay = policy.delay_for_retry(retry_number, None);
                    if !delay_within_budget(&delay, &total_budget, call_start) {
                        return Err(last_error.unwrap());
                    }
                    if self.inner.log_level >= LogLevel::Info {
                        logging::log_retry_scheduled(
                            method.as_str(),
                            path,
                            delay,
                            retry_number + 1,
                            &last_error.as_ref().unwrap().to_string(),
                        );
                    }
                    tokio::time::sleep(delay).await;
                    continue;
                }
            };
            let duration = started.elapsed();
            if self.inner.log_level >= LogLevel::Info {
                logging::log_attempt_info(
                    method.as_str(),
                    path,
                    status,
                    duration,
                    request_id.as_deref(),
                );
            }
            if self.inner.log_level >= LogLevel::Debug {
                logging::log_response_debug(
                    method.as_str(),
                    path,
                    status,
                    &response_headers,
                    &text,
                );
            }

            if (200..300).contains(&status) {
                return Ok(RawResponse {
                    status,
                    body: text,
                    request_id,
                    endpoint,
                });
            }

            // Non-2xx: build the API error.
            let parsed_body = parse_body(&text);
            // Always expose the server's requested delay on the error;
            // whether a retry honors it is decided by `delay_for_retry`.
            let retry_after = policy.parse_retry_after(&response_headers);
            let api_error = ApiError {
                status,
                kind: ApiErrorKind::from_status(status),
                message: api_error_message(&parsed_body),
                body: parsed_body,
                headers: response_headers,
                request_id,
                endpoint: endpoint.clone(),
                retry_after,
            };
            let error = Error::from(api_error);
            last_error = Some(error);

            if !should_retry(&policy, &last_error, retry_number) {
                return Err(last_error.unwrap());
            }
            let delay =
                policy.delay_for_retry(retry_number, last_error.as_ref().and_then(Error::as_api));
            if !delay_within_budget(&delay, &total_budget, call_start) {
                // Budget reached: return the last real error (e.g. the 529),
                // not an artificial timeout.
                return Err(last_error.unwrap());
            }
            if self.inner.log_level >= LogLevel::Info {
                logging::log_retry_scheduled(
                    method.as_str(),
                    path,
                    delay,
                    retry_number + 1,
                    &last_error.as_ref().unwrap().to_string(),
                );
            }
            tokio::time::sleep(delay).await;
        }

        Err(last_error.unwrap_or_else(|| Error::Connection {
            message: "Connection error: exhausted retries.".into(),
            source: None,
        }))
    }
}

/// Whether the next retry should happen: retries left, error retryable.
fn should_retry(policy: &RetryPolicy, last_error: &Option<Error>, retry_number: u32) -> bool {
    if retry_number >= policy.max_retries {
        return false;
    }
    last_error
        .as_ref()
        .map(|error| policy.is_retryable(error))
        .unwrap_or(false)
}

/// Whether `delay` fits in the remaining total budget.
fn delay_within_budget(
    delay: &Duration,
    total_budget: &Option<Duration>,
    call_start: Instant,
) -> bool {
    match total_budget {
        None => true,
        Some(budget) => {
            let elapsed = call_start.elapsed();
            // Stop when the next delay would reach or exceed the budget.
            elapsed.saturating_add(*delay) < *budget
        }
    }
}

/// Parses a response body: JSON when parseable, else the raw text as a JSON
/// string; `None` when empty.
fn parse_body(text: &str) -> Option<Value> {
    if text.is_empty() {
        return None;
    }
    match serde_json::from_str(text) {
        Ok(value) => Some(value),
        Err(_) => Some(Value::String(text.to_owned())),
    }
}

fn build_header_map(
    headers: &BTreeMap<String, String>,
) -> Result<reqwest::header::HeaderMap, Error> {
    let mut map = reqwest::header::HeaderMap::new();
    for (name, value) in headers {
        let name: reqwest::header::HeaderName = name
            .parse()
            .map_err(|error| Error::Config(format!("invalid header name {name:?}: {error}")))?;
        let value = reqwest::header::HeaderValue::from_str(value).map_err(|error| {
            // Never include the value in the error.
            Error::Config(format!("invalid value for header {name}: {error}"))
        })?;
        map.insert(name, value);
    }
    Ok(map)
}

/// Requires an absolute http(s) URL with a host and no query or fragment,
/// since request paths are appended to it verbatim.
fn validate_base_url(base_url: &str) -> Result<(), Error> {
    let invalid = || {
        Error::Config(format!("invalid base URL {base_url:?}: expected an absolute http(s) URL with no query or fragment"))
    };
    let url = reqwest::Url::parse(base_url).map_err(|_| invalid())?;
    let scheme_ok = matches!(url.scheme(), "http" | "https");
    if !scheme_ok || url.host_str().is_none() || url.query().is_some() || url.fragment().is_some() {
        return Err(invalid());
    }
    Ok(())
}

fn sdk_version_header() -> String {
    format!("typesafe-client-rust/{}", env!("CARGO_PKG_VERSION"))
}

fn runtime_header() -> String {
    format!(
        "rust ({}; {})",
        std::env::consts::OS,
        std::env::consts::ARCH
    )
}
