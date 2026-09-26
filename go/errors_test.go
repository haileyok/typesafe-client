package typesafe

import (
	"context"
	"errors"
	"net/http"
	"strings"
	"testing"
	"time"
)

func TestAPIErrorKinds(t *testing.T) {
	t.Parallel()
	cases := []struct {
		status   int
		sentinel error
	}{
		{400, ErrBadRequest},
		{401, ErrAuthentication},
		{403, ErrPermissionDenied},
		{404, ErrNotFound},
		{422, ErrUnprocessableEntity},
		{429, ErrRateLimit},
		{500, ErrInternalServer},
		{529, ErrInternalServer},
	}
	all := []error{ErrBadRequest, ErrAuthentication, ErrPermissionDenied, ErrNotFound, ErrUnprocessableEntity, ErrRateLimit, ErrInternalServer}
	for _, tc := range cases {
		e := &APIError{StatusCode: tc.status}
		for _, s := range all {
			if got, want := errors.Is(e, s), errors.Is(s, tc.sentinel); got != want {
				t.Errorf("status %d: errors.Is(%v) = %v, want %v", tc.status, s, got, want)
			}
		}
	}
	if errors.Is(&APIError{StatusCode: 418}, ErrBadRequest) {
		t.Errorf("418 matched a sentinel")
	}
}

func TestErrorMessageExtraction(t *testing.T) {
	t.Parallel()
	long := strings.Repeat("é", 250)
	cases := []struct {
		name string
		body string
		want string
	}{
		{"string body", `plain failure`, "plain failure"},
		{"error string", `{"error":"bad key"}`, "bad key"},
		{"error object", `{"error":{"message":"nested"}}`, "nested"},
		{"message", `{"message":"msg"}`, "msg"},
		{"detail string", `{"detail":"Not authenticated"}`, "Not authenticated"},
		{"detail object", `{"detail":{"message":"det"}}`, "det"},
		{"fastapi", `{"detail":[{"loc":["body","questions","urgency","score","criteria"],"msg":"Field required","type":"missing"},{"loc":["body","state"],"msg":"Field required"},{"msg":"no loc"},{"bogus":1}]}`,
			"questions.urgency.score.criteria: Field required; state: Field required; no loc"},
		{"fastapi index", `{"detail":[{"loc":["body","questions",0],"msg":"bad"}]}`, "questions.0: bad"},
		{"unknown json", `{"weird":true}`, `{"weird":true}`},
		{"empty", ``, "status code (no body)"},
		{"long raw", `"` + long + `"`, strings.Repeat("é", 200) + "…"},
	}
	for _, tc := range cases {
		e := newAPIError("POST", "https://x/v1/systemone", 422, http.Header{}, []byte(tc.body))
		if e.Message != tc.want {
			t.Errorf("%s: message = %q, want %q", tc.name, e.Message, tc.want)
		}
	}
}

func TestAPIErrorString(t *testing.T) {
	t.Parallel()
	h := http.Header{}
	h.Set("x-typesafe-request-id", "req_9")
	e := newAPIError("POST", "https://api.typesafe.ai/v1/systemone", 401, h, []byte(`{"detail":"Invalid API key"}`))
	want := "POST https://api.typesafe.ai/v1/systemone: 401 Invalid API key (request_id=req_9)"
	if e.Error() != want || e.RequestID != "req_9" {
		t.Errorf("got %q", e.Error())
	}
	if (&APIError{StatusCode: 500}).Error() != "500" {
		t.Errorf("bare error = %q", (&APIError{StatusCode: 500}).Error())
	}
}

func TestAPIErrorFromServer(t *testing.T) {
	t.Parallel()
	s := newTestServer(t, func(_ int, _ recorded, w http.ResponseWriter) {
		w.Header().Set("x-typesafe-request-id", "req_422")
		writeJSON(w, 422, map[string]any{"detail": []any{map[string]any{"loc": []any{"body", "state"}, "msg": "Field required"}}})
	})
	c := newTestClient(t, s)
	_, err := c.SystemOne(context.Background(), Request{State: "x", Questions: oneQuestion})
	var apiErr *APIError
	if !errors.As(err, &apiErr) || !errors.Is(err, ErrUnprocessableEntity) {
		t.Fatalf("err = %v", err)
	}
	want := "POST " + s.URL + "/v1/systemone: 422 state: Field required (request_id=req_422)"
	if apiErr.Error() != want {
		t.Errorf("got  %q\nwant %q", apiErr.Error(), want)
	}
	if _, ok := apiErr.Body.(map[string]any); !ok {
		t.Errorf("body = %#v", apiErr.Body)
	}
	if strings.Contains(err.Error(), "sk-test") {
		t.Errorf("key leaked")
	}
}

func TestRateLimitRetryAfter(t *testing.T) {
	t.Parallel()
	h := http.Header{}
	h.Set("retry-after-ms", "1500")
	e := &APIError{StatusCode: 429, Header: h}
	if d, ok := e.RetryAfter(); !ok || d != 1500*time.Millisecond {
		t.Errorf("RetryAfter = %s %v", d, ok)
	}
}

func TestConnectionErrorKinds(t *testing.T) {
	t.Parallel()
	ce := &ConnectionError{Method: "POST", URL: "u", Err: errors.New("reset")}
	if !errors.Is(ce, ErrConnection) || errors.Is(ce, ErrTimeout) || !strings.Contains(ce.Error(), "reset") {
		t.Errorf("connection error: %v", ce)
	}
	te := &TimeoutError{Method: "POST", URL: "u", Timeout: time.Second, Err: context.DeadlineExceeded}
	if !errors.Is(te, ErrConnection) || !errors.Is(te, ErrTimeout) || !errors.Is(te, context.DeadlineExceeded) {
		t.Errorf("timeout error kinds")
	}
}
