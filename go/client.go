package typesafe

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"log/slog"
	"math/rand/v2"
	"net"
	"net/http"
	"net/url"
	"os"
	"runtime"
	"strings"
	"time"
)

// DefaultTimeout is the default per-attempt timeout, covering connection
// through reading the full response body.
const DefaultTimeout = 10 * time.Second

// maxResponseBody bounds how much of a response body is read.
const maxResponseBody = 64 << 20

// Client calls the TypeSafe API. It is safe for concurrent use; create one
// and reuse it.
type Client struct {
	apiKey  string
	baseURL string
	model   string
	timeout time.Duration
	retry   RetryPolicy
	headers http.Header
	http    *http.Client
	logger  *slog.Logger

	// Test seams.
	random func() float64
	now    func() time.Time
}

// Option configures a Client.
type Option func(*Client) error

// WithAPIKey sets the API key. It overrides TYPESAFE_API_KEY.
func WithAPIKey(key string) Option {
	return func(c *Client) error { c.apiKey = key; return nil }
}

// WithBaseURL sets the API root (default https://api.typesafe.ai). It
// overrides TYPESAFE_BASE_URL. Any gateway that implements the TypeSafe
// OpenAPI spec works, for example "https://openrouter.ai/api" or
// "https://ai-gateway.vercel.sh/typesafe".
func WithBaseURL(u string) Option {
	return func(c *Client) error { c.baseURL = u; return nil }
}

// WithModel sets the default model (default "jev-latest"). It overrides
// TYPESAFE_DEFAULT_MODEL; Request.Model overrides it per call.
func WithModel(model string) Option {
	return func(c *Client) error { c.model = model; return nil }
}

// WithTimeout sets the per-attempt timeout (default 10s). Each retry gets a
// fresh timeout; bound the whole call with a context deadline or
// RetryPolicy.TotalBudget.
func WithTimeout(d time.Duration) Option {
	return func(c *Client) error {
		if d <= 0 {
			return configf("timeout must be positive, got %s", d)
		}
		c.timeout = d
		return nil
	}
}

// WithRetryPolicy replaces the retry policy (default DefaultRetryPolicy()).
func WithRetryPolicy(p RetryPolicy) Option {
	return func(c *Client) error {
		if err := p.validate(); err != nil {
			return err
		}
		p.HTTPStatuses = append([]int(nil), p.HTTPStatuses...)
		c.retry = p
		return nil
	}
}

// WithHeader adds a header to every request, for example a gateway
// attribution header. Client-managed headers (Authorization, User-Agent,
// X-TypeSafe-*) cannot be overridden.
func WithHeader(name, value string) Option {
	return func(c *Client) error { c.headers.Set(name, value); return nil }
}

// WithHTTPClient sets the HTTP client used for requests (default: a new
// http.Client). Use it to configure proxies, transports, or TLS.
func WithHTTPClient(h *http.Client) Option {
	return func(c *Client) error {
		if h == nil {
			return configf("http client must not be nil")
		}
		c.http = h
		return nil
	}
}

// WithLogger sets the logger. By default the client logs nothing unless
// TYPESAFE_LOG_LEVEL is set (debug, info, warn, error, off), in which case
// it logs to stderr at that level. At info the client logs one line per
// attempt and per retry; at debug it also logs headers (credentials
// redacted) and bodies (not redacted).
func WithLogger(l *slog.Logger) Option {
	return func(c *Client) error { c.logger = l; return nil }
}

// NewClient creates a Client. Configuration comes from options, then the
// TYPESAFE_* environment variables, then defaults. It fails with ErrConfig
// when no valid API key is available.
func NewClient(opts ...Option) (*Client, error) {
	c := &Client{
		timeout: DefaultTimeout,
		retry:   DefaultRetryPolicy(),
		headers: http.Header{},
		http:    &http.Client{},
		random:  rand.Float64,
		now:     time.Now,
	}
	for _, opt := range opts {
		if err := opt(c); err != nil {
			return nil, err
		}
	}
	key, err := resolveAPIKey(c.apiKey)
	if err != nil {
		return nil, err
	}
	c.apiKey = key
	c.baseURL = strings.TrimRight(resolve(c.baseURL, EnvBaseURL, DefaultBaseURL), "/")
	// Request paths are appended verbatim, so require an absolute http(s)
	// URL with a host and no query or fragment.
	if u, err := url.Parse(c.baseURL); err != nil || (u.Scheme != "http" && u.Scheme != "https") ||
		u.Host == "" || u.RawQuery != "" || u.Fragment != "" || strings.Contains(c.baseURL, "?") || strings.Contains(c.baseURL, "#") {
		return nil, configf("invalid base URL %q: expected an absolute http(s) URL with no query or fragment", c.baseURL)
	}
	c.model = resolve(c.model, EnvDefaultModel, DefaultModel)
	if c.logger == nil {
		l, err := loggerFromEnv()
		if err != nil {
			return nil, err
		}
		c.logger = l
	}
	return c, nil
}

// resolve returns explicit if set, else the trimmed environment value if
// non-empty, else def.
func resolve(explicit, env, def string) string {
	if explicit != "" {
		return explicit
	}
	if v := strings.TrimSpace(os.Getenv(env)); v != "" {
		return v
	}
	return def
}

func resolveAPIKey(explicit string) (string, error) {
	key := strings.TrimSpace(resolve(explicit, EnvAPIKey, ""))
	if key == "" {
		return "", configf("no API key was provided; pass WithAPIKey or set %s", EnvAPIKey)
	}
	for i := 0; i < len(key); i++ {
		// Printable ASCII excluding space.
		if key[i] < 0x21 || key[i] > 0x7e {
			return "", configf("API key must contain only printable ASCII characters without whitespace")
		}
	}
	return key, nil
}

// Request is one System One evaluation: a state evaluated against named
// questions.
type Request struct {
	// State is the content to evaluate: a string, or any value that
	// marshals to a JSON object or array (map, struct, slice). See
	// https://docs.typesafe.ai/concepts/state.
	State any
	// Questions maps your question IDs to questions; at least one is
	// required. Every question sees the same state and is evaluated
	// independently and in parallel, so batch every question about a state
	// into one request.
	Questions Questions
	// Model overrides the client's default model for this call.
	Model string
}

// RequestOption customizes a single call.
type RequestOption func(*callConfig)

type callConfig struct {
	timeout   time.Duration
	retry     *RetryPolicy
	headers   http.Header
	extraBody map[string]any
}

// RequestTimeout overrides the per-attempt timeout for this call.
func RequestTimeout(d time.Duration) RequestOption {
	return func(cc *callConfig) { cc.timeout = d }
}

// RequestRetryPolicy replaces the client's retry policy for this call.
func RequestRetryPolicy(p RetryPolicy) RequestOption {
	return func(cc *callConfig) { cc.retry = &p }
}

// RequestHeader adds a header to this call, overriding a client header of
// the same name.
func RequestHeader(name, value string) RequestOption {
	return func(cc *callConfig) { cc.headers.Set(name, value) }
}

// RequestExtraBody sets an additional top-level request body field. It is
// merged last, so it can override state, model, or questions.
func RequestExtraBody(key string, value any) RequestOption {
	return func(cc *callConfig) {
		if cc.extraBody == nil {
			cc.extraBody = map[string]any{}
		}
		cc.extraBody[key] = value
	}
}

func (c *Client) callConfig(opts []RequestOption) (callConfig, error) {
	cc := callConfig{timeout: c.timeout, headers: http.Header{}}
	for _, o := range opts {
		o(&cc)
	}
	if cc.timeout <= 0 {
		return cc, invalidf("timeout must be positive, got %s", cc.timeout)
	}
	if cc.retry == nil {
		cc.retry = &c.retry
	} else if err := cc.retry.validate(); err != nil {
		return cc, fmt.Errorf("%w: retry policy: %s", ErrInvalidRequest, err.Error())
	}
	return cc, nil
}

// SystemOne evaluates req.State against req.Questions and returns one typed
// answer per question.
//
// Errors: ErrInvalidRequest (checked before any I/O), *APIError (non-2xx
// after retries; see the ErrRateLimit etc. sentinels), *ConnectionError,
// *TimeoutError, *ResponseValidationError, or ctx.Err() when ctx ends.
func (c *Client) SystemOne(ctx context.Context, req Request, opts ...RequestOption) (*Response, error) {
	cc, err := c.callConfig(opts)
	if err != nil {
		return nil, err
	}
	body, err := c.encodeSystemOne(req, cc.extraBody)
	if err != nil {
		return nil, err
	}
	res, err := c.do(ctx, http.MethodPost, systemOnePath, body, cc)
	if err != nil {
		return nil, err
	}
	out, err := decodeSystemOne(res.body)
	// Completeness is checked against the questions actually sent, which an
	// extra-body "questions" override replaces; skip the check then.
	if _, overridden := cc.extraBody["questions"]; err == nil && !overridden {
		err = checkComplete(out, req.Questions)
	}
	if err != nil {
		return nil, res.validationError(err)
	}
	out.RequestID = res.header.Get(headerRequestID)
	out.Header = res.header
	return out, nil
}

// ListModels returns the model names the account can send in the model
// field. It currently lists aliases; versioned IDs such as "jev-1.13.0" are
// accepted whether or not they appear.
func (c *Client) ListModels(ctx context.Context, opts ...RequestOption) (*ModelList, error) {
	cc, err := c.callConfig(opts)
	if err != nil {
		return nil, err
	}
	res, err := c.do(ctx, http.MethodGet, modelsPath, nil, cc)
	if err != nil {
		return nil, err
	}
	models, err := decodeModels(res.body)
	if err != nil {
		return nil, res.validationError(err)
	}
	return &ModelList{Models: models, RequestID: res.header.Get(headerRequestID)}, nil
}

func (c *Client) encodeSystemOne(req Request, extra map[string]any) ([]byte, error) {
	if len(req.Questions) == 0 {
		return nil, invalidf("at least one question is required")
	}
	for id, q := range req.Questions {
		if q == nil {
			return nil, invalidf("question %q is nil", id)
		}
		if err := q.validate(id); err != nil {
			return nil, err
		}
	}
	state, err := json.Marshal(req.State)
	if err != nil {
		return nil, invalidf("state could not be encoded as JSON: %v", err)
	}
	if s := bytes.TrimSpace(state); len(s) == 0 || (s[0] != '"' && s[0] != '{' && s[0] != '[') {
		return nil, invalidf("state must be a string, JSON object, or array, got %s", state)
	}
	model := req.Model
	if model == "" {
		model = c.model
	}
	payload := map[string]any{
		"state":     json.RawMessage(state),
		"model":     model,
		"questions": req.Questions,
	}
	for k, v := range extra {
		payload[k] = v
	}
	body, err := json.Marshal(payload)
	if err != nil {
		return nil, invalidf("request could not be encoded as JSON: %v", err)
	}
	return body, nil
}

// result is a successful (2xx) HTTP exchange.
type result struct {
	status      int
	header      http.Header
	body        []byte
	method, url string
}

func (r *result) validationError(err error) error {
	var fe *fieldError
	if !errors.As(err, &fe) {
		return err
	}
	return &ResponseValidationError{
		StatusCode: r.status,
		FieldPath:  fe.path,
		RequestID:  r.header.Get(headerRequestID),
		Method:     r.method,
		URL:        r.url,
		Body:       r.body,
	}
}

// do sends one logical request with the retry policy.
func (c *Client) do(ctx context.Context, method, path string, body []byte, cc callConfig) (*result, error) {
	fullURL := c.baseURL + path
	displayURL := redactURL(fullURL)
	policy := *cc.retry
	start := c.now()

	for attempt := 0; ; attempt++ {
		if err := ctx.Err(); err != nil {
			return nil, err
		}
		res, err := c.attempt(ctx, method, fullURL, displayURL, path, body, cc, attempt)
		if err == nil {
			return res, nil
		}
		if ctxErr := ctx.Err(); ctxErr != nil {
			return nil, ctxErr
		}
		if attempt >= policy.MaxRetries || !policy.retryable(err) {
			return nil, err
		}
		var header http.Header
		var apiErr *APIError
		if errors.As(err, &apiErr) {
			header = apiErr.Header
		}
		now := c.now()
		delay := policy.delay(attempt, header, now, c.random)
		// Stop before a retry that cannot complete in time, returning the
		// real failure rather than an artificial deadline error.
		if policy.TotalBudget > 0 && now.Sub(start)+delay >= policy.TotalBudget {
			return nil, err
		}
		if dl, ok := ctx.Deadline(); ok && !now.Add(delay).Before(dl) {
			return nil, err
		}
		c.logger.LogAttrs(ctx, slog.LevelInfo, "typesafe: retrying",
			slog.String("method", method), slog.String("path", path),
			slog.Duration("delay", delay), slog.Int("retry", attempt+1),
			slog.Int("max_retries", policy.MaxRetries), slog.String("reason", err.Error()))
		if delay > 0 {
			t := time.NewTimer(delay)
			select {
			case <-ctx.Done():
				t.Stop()
				return nil, ctx.Err()
			case <-t.C:
			}
		}
	}
}

// attempt performs one HTTP round trip, including reading the body, under
// the per-attempt timeout. A non-2xx response is returned as *APIError.
func (c *Client) attempt(ctx context.Context, method, fullURL, displayURL, path string, body []byte, cc callConfig, attempt int) (*result, error) {
	actx, cancel := context.WithTimeout(ctx, cc.timeout)
	defer cancel()

	var rdr io.Reader
	if body != nil {
		rdr = bytes.NewReader(body)
	}
	req, err := http.NewRequestWithContext(actx, method, fullURL, rdr)
	if err != nil {
		return nil, &ConnectionError{Method: method, URL: displayURL, Err: err}
	}
	req.Header = c.requestHeaders(cc.headers, body != nil, attempt)

	if c.logger.Enabled(ctx, slog.LevelDebug) {
		c.logger.LogAttrs(ctx, slog.LevelDebug, "typesafe: request",
			slog.String("method", method), slog.String("url", displayURL),
			slog.Any("headers", redactHeaders(req.Header)), slog.String("body", string(body)))
	}

	started := time.Now()
	resp, err := c.http.Do(req) //nolint:gosec // G704: the URL is the operator-configured API base URL, not user input.
	if err != nil {
		return nil, c.transportError(ctx, actx, method, displayURL, cc.timeout, err)
	}
	defer func() { _ = resp.Body.Close() }()
	raw, err := io.ReadAll(io.LimitReader(resp.Body, maxResponseBody+1))
	if err != nil {
		return nil, c.transportError(ctx, actx, method, displayURL, cc.timeout, err)
	}
	if len(raw) > maxResponseBody {
		return nil, &ConnectionError{Method: method, URL: displayURL,
			Err: fmt.Errorf("response body exceeds %d bytes", maxResponseBody)}
	}

	c.logger.LogAttrs(ctx, slog.LevelInfo, "typesafe: response",
		slog.String("method", method), slog.String("path", path),
		slog.Int("status", resp.StatusCode), slog.Duration("duration", time.Since(started)),
		slog.String("request_id", resp.Header.Get(headerRequestID)))
	if c.logger.Enabled(ctx, slog.LevelDebug) {
		c.logger.LogAttrs(ctx, slog.LevelDebug, "typesafe: response body",
			slog.Any("headers", redactHeaders(resp.Header)), slog.String("body", string(raw)))
	}

	if resp.StatusCode < 200 || resp.StatusCode > 299 {
		return nil, newAPIError(method, displayURL, resp.StatusCode, resp.Header, raw)
	}
	return &result{status: resp.StatusCode, header: resp.Header, body: raw, method: method, url: displayURL}, nil
}

// transportError classifies a failure that produced no usable response.
func (c *Client) transportError(parent, actx context.Context, method, displayURL string, timeout time.Duration, err error) error {
	if parent.Err() != nil {
		return parent.Err()
	}
	var ne net.Error
	if errors.Is(actx.Err(), context.DeadlineExceeded) || (errors.As(err, &ne) && ne.Timeout()) {
		return &TimeoutError{Method: method, URL: displayURL, Timeout: timeout, Err: err}
	}
	return &ConnectionError{Method: method, URL: displayURL, Err: err}
}

var runtimeHeader = fmt.Sprintf("go/%s (%s; %s)", strings.TrimPrefix(runtime.Version(), "go"), runtime.GOOS, runtime.GOARCH)

// requestHeaders merges client headers, then call headers, then the
// client-managed headers, which always win.
func (c *Client) requestHeaders(call http.Header, hasBody bool, attempt int) http.Header {
	h := c.headers.Clone()
	for k, v := range call {
		h[k] = append([]string(nil), v...)
	}
	h.Del(headerRetryCount)
	ua := sdkName + "/" + Version
	h.Set("Authorization", "Bearer "+c.apiKey)
	h.Set("Accept", "application/json")
	h.Set("User-Agent", ua)
	h.Set("X-TypeSafe-SDK", ua)
	h.Set("X-TypeSafe-Runtime", runtimeHeader)
	if hasBody {
		h.Set("Content-Type", "application/json")
	} else {
		h.Del("Content-Type")
	}
	if attempt > 0 {
		h.Set(headerRetryCount, fmt.Sprint(attempt))
	}
	return h
}

// redactURL drops credentials, query, and fragment from u for error
// messages and logs.
func redactURL(u string) string {
	p, err := url.Parse(u)
	if err != nil {
		return u
	}
	p.User = nil
	p.RawQuery = ""
	p.Fragment = ""
	return p.String()
}
