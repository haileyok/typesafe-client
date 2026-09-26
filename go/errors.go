package typesafe

import (
	"encoding/json"
	"errors"
	"fmt"
	"net/http"
	"strings"
	"time"
	"unicode/utf8"
)

// Sentinel errors. Use errors.Is to classify an error returned by the
// client:
//
//	if errors.Is(err, typesafe.ErrRateLimit) { ... }
var (
	// ErrConfig reports invalid client configuration, such as a missing API
	// key. Returned by NewClient.
	ErrConfig = errors.New("typesafe: invalid configuration")
	// ErrInvalidRequest reports a request rejected before any network I/O,
	// such as an empty question set or a one-level Score.
	ErrInvalidRequest = errors.New("typesafe: invalid request")

	// ErrBadRequest matches an *APIError with status 400.
	ErrBadRequest = errors.New("typesafe: bad request")
	// ErrAuthentication matches an *APIError with status 401.
	ErrAuthentication = errors.New("typesafe: authentication failed")
	// ErrPermissionDenied matches an *APIError with status 403.
	ErrPermissionDenied = errors.New("typesafe: permission denied")
	// ErrNotFound matches an *APIError with status 404.
	ErrNotFound = errors.New("typesafe: not found")
	// ErrUnprocessableEntity matches an *APIError with status 422 (the
	// request failed server-side validation).
	ErrUnprocessableEntity = errors.New("typesafe: unprocessable entity")
	// ErrRateLimit matches an *APIError with status 429.
	ErrRateLimit = errors.New("typesafe: rate limit exceeded")
	// ErrInternalServer matches an *APIError with status >= 500, including
	// 529 Overloaded.
	ErrInternalServer = errors.New("typesafe: server error")

	// ErrConnection matches a *ConnectionError or *TimeoutError: the request
	// produced no HTTP response.
	ErrConnection = errors.New("typesafe: connection error")
	// ErrTimeout matches a *TimeoutError: an attempt exceeded the
	// per-attempt timeout.
	ErrTimeout = errors.New("typesafe: request timed out")
)

func invalidf(format string, args ...any) error {
	return fmt.Errorf("%w: %s", ErrInvalidRequest, fmt.Sprintf(format, args...))
}

func configf(format string, args ...any) error {
	return fmt.Errorf("%w: %s", ErrConfig, fmt.Sprintf(format, args...))
}

// maxErrorBody bounds the raw body quoted in an error message.
const maxErrorBody = 200

// APIError is a non-2xx HTTP response, returned after any retries.
type APIError struct {
	// StatusCode is the HTTP status.
	StatusCode int
	// Message is the server's error message, extracted from the body.
	Message string
	// Body is the parsed JSON error body, the raw text when the body is not
	// JSON, or nil when the body was empty.
	Body any
	// Header holds the response headers.
	Header http.Header
	// RequestID is the x-typesafe-request-id response header, if present.
	RequestID string
	// Method and URL identify the request (the URL excludes credentials,
	// query, and fragment).
	Method string
	URL    string
}

func newAPIError(method, url string, status int, header http.Header, raw []byte) *APIError {
	body := parseBody(raw)
	msg := extractMessage(body)
	if msg == "" {
		switch b := body.(type) {
		case nil:
			msg = "status code (no body)"
		case string:
			msg = truncate(b)
		default:
			enc, _ := json.Marshal(b)
			msg = truncate(string(enc))
		}
	}
	return &APIError{
		StatusCode: status,
		Message:    msg,
		Body:       body,
		Header:     header,
		RequestID:  header.Get(headerRequestID),
		Method:     method,
		URL:        url,
	}
}

// Error formats the error as "<METHOD> <URL>: <status> <message>" with a
// "(request_id=...)" suffix when the server returned one.
func (e *APIError) Error() string {
	var b strings.Builder
	if e.Method != "" {
		fmt.Fprintf(&b, "%s %s: ", e.Method, e.URL)
	}
	fmt.Fprintf(&b, "%d", e.StatusCode)
	if e.Message != "" {
		b.WriteByte(' ')
		b.WriteString(e.Message)
	}
	if e.RequestID != "" {
		fmt.Fprintf(&b, " (request_id=%s)", e.RequestID)
	}
	return b.String()
}

// Is matches the status sentinels (ErrRateLimit, ErrAuthentication, ...).
func (e *APIError) Is(target error) bool {
	switch target {
	case ErrBadRequest:
		return e.StatusCode == http.StatusBadRequest
	case ErrAuthentication:
		return e.StatusCode == http.StatusUnauthorized
	case ErrPermissionDenied:
		return e.StatusCode == http.StatusForbidden
	case ErrNotFound:
		return e.StatusCode == http.StatusNotFound
	case ErrUnprocessableEntity:
		return e.StatusCode == http.StatusUnprocessableEntity
	case ErrRateLimit:
		return e.StatusCode == http.StatusTooManyRequests
	case ErrInternalServer:
		return e.StatusCode >= 500
	}
	return false
}

// RetryAfter returns the delay the server asked for via retry-after-ms or
// Retry-After, and whether one was present and valid.
func (e *APIError) RetryAfter() (time.Duration, bool) {
	return parseRetryAfter(e.Header, time.Now())
}

// ConnectionError reports a request that produced no HTTP response: DNS,
// TLS, connection reset, or a failure reading the response body.
type ConnectionError struct {
	Method string
	URL    string
	Err    error
}

func (e *ConnectionError) Error() string {
	return fmt.Sprintf("%s %s: connection error: %v", e.Method, e.URL, e.Err)
}

// Unwrap returns the underlying transport error.
func (e *ConnectionError) Unwrap() error { return e.Err }

// Is matches ErrConnection.
func (e *ConnectionError) Is(target error) bool { return target == ErrConnection }

// TimeoutError reports an attempt that exceeded the per-attempt timeout
// (connect through reading the full response body). It is a kind of
// connection error: errors.Is(err, ErrConnection) is also true.
//
// Cancellation or expiry of the caller's context is not a TimeoutError; the
// client returns ctx.Err() for that.
type TimeoutError struct {
	Method  string
	URL     string
	Timeout time.Duration
	Err     error
}

func (e *TimeoutError) Error() string {
	return fmt.Sprintf("%s %s: request timed out (timeout=%s)", e.Method, e.URL, e.Timeout)
}

// Unwrap returns the underlying transport error.
func (e *TimeoutError) Unwrap() error { return e.Err }

// Is matches ErrTimeout and ErrConnection.
func (e *TimeoutError) Is(target error) bool {
	return target == ErrTimeout || target == ErrConnection
}

// ResponseValidationError reports a 2xx response whose body is missing a
// required field or has a field of the wrong type.
type ResponseValidationError struct {
	StatusCode int
	// FieldPath is the dotted path to the first offending field, for
	// example "answers.tone.confidence". It is empty when the body as a
	// whole is not a JSON object.
	FieldPath string
	RequestID string
	Method    string
	URL       string
	// Body is the raw response body.
	Body []byte
}

func (e *ResponseValidationError) Error() string {
	s := fmt.Sprintf("%s %s: %d invalid response data at %q", e.Method, e.URL, e.StatusCode, e.FieldPath)
	if e.RequestID != "" {
		s += fmt.Sprintf(" (request_id=%s)", e.RequestID)
	}
	return s
}

// parseBody decodes an error body as JSON, falling back to its text; an
// empty body is nil.
func parseBody(raw []byte) any {
	if len(raw) == 0 {
		return nil
	}
	var v any
	if err := json.Unmarshal(raw, &v); err == nil {
		return v
	}
	return string(raw)
}

// extractMessage pulls a human-readable message out of an error body, in
// the same order as the official SDKs.
func extractMessage(body any) string {
	switch b := body.(type) {
	case string:
		// Unlike the official SDKs, bound plain-text bodies too: a gateway
		// or proxy error page can be arbitrarily large.
		return truncate(b)
	case map[string]any:
		if s, ok := b["error"].(string); ok {
			return s
		}
		if m, ok := b["error"].(map[string]any); ok {
			if s, ok := m["message"].(string); ok {
				return s
			}
		}
		if s, ok := b["message"].(string); ok {
			return s
		}
		switch d := b["detail"].(type) {
		case string:
			return d
		case map[string]any:
			if s, ok := d["message"].(string); ok {
				return s
			}
		case []any:
			return describeValidationErrors(d)
		}
	}
	return ""
}

// describeValidationErrors renders FastAPI-style validation errors as
// "questions.urgency.criteria: Field required; ...".
func describeValidationErrors(entries []any) string {
	var parts []string
	for _, e := range entries {
		m, ok := e.(map[string]any)
		if !ok {
			continue
		}
		msg, ok := m["msg"].(string)
		if !ok {
			continue
		}
		var path []string
		if loc, ok := m["loc"].([]any); ok {
			for _, seg := range loc {
				if seg == "body" {
					continue
				}
				switch s := seg.(type) {
				case string:
					path = append(path, s)
				case float64:
					path = append(path, fmt.Sprintf("%d", int64(s)))
				default:
					path = append(path, fmt.Sprint(s))
				}
			}
		}
		if len(path) > 0 {
			parts = append(parts, strings.Join(path, ".")+": "+msg)
		} else {
			parts = append(parts, msg)
		}
	}
	return strings.Join(parts, "; ")
}

// truncate bounds s to maxErrorBody runes, appending an ellipsis.
func truncate(s string) string {
	if utf8.RuneCountInString(s) <= maxErrorBody {
		return s
	}
	r := []rune(s)
	return string(r[:maxErrorBody]) + "…"
}
