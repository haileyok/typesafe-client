package typesafe

import (
	"bytes"
	"context"
	"errors"
	"log/slog"
	"net/http"
	"strings"
	"testing"
	"time"
)

func TestRequestShape(t *testing.T) {
	t.Parallel()
	s := newTestServer(t, okHandler)
	c := newTestClient(t, s)
	_, err := c.SystemOne(context.Background(), Request{
		State: map[string]any{"ticket": "charged twice"},
		Questions: Questions{
			"tone": ChoiceNames("Tone?", "calm", "angry"),
			"q":    Noul("Billing?"),
		},
	})
	if err != nil {
		t.Fatal(err)
	}
	r := s.Requests()[0]
	if r.Method != http.MethodPost || r.Path != "/v1/systemone" {
		t.Errorf("got %s %s", r.Method, r.Path)
	}
	want := `{"model":"jev-latest","questions":{"q":{"type":"noul","instructions":"Billing?"},"tone":{"type":"choice","instructions":"Tone?","criteria":{"calm":null,"angry":null}}},"state":{"ticket":"charged twice"}}`
	if string(r.Body) != want {
		t.Errorf("body\n got  %s\n want %s", r.Body, want)
	}
}

func TestPerRequestModelAndExtraBody(t *testing.T) {
	t.Parallel()
	s := newTestServer(t, okHandler)
	c := newTestClient(t, s)
	_, err := c.SystemOne(context.Background(),
		Request{State: "x", Questions: oneQuestion, Model: "jev-1.13.0"},
		RequestExtraBody("trace", "abc"), RequestExtraBody("model", "override-wins"))
	if err != nil {
		t.Fatal(err)
	}
	body := mustJSON(t, s.Requests()[0].Body)
	if body["trace"] != "abc" || body["model"] != "override-wins" {
		t.Errorf("body = %v", body)
	}
	if _, err := c.SystemOne(context.Background(), Request{State: "x", Questions: oneQuestion, Model: "jev-1.13.0"}); err != nil {
		t.Fatal(err)
	}
	if m := mustJSON(t, s.Requests()[1].Body)["model"]; m != "jev-1.13.0" {
		t.Errorf("model = %v", m)
	}
}

func TestHeaders(t *testing.T) {
	t.Parallel()
	s := newTestServer(t, func(n int, r recorded, w http.ResponseWriter) {
		if n == 0 {
			writeJSON(w, 529, map[string]any{"error": "overloaded"})
			return
		}
		okHandler(n, r, w)
	})
	c := newTestClient(t, s,
		WithHeader("X-Agent-Client", "client-default"),
		WithHeader("X-Override", "client"),
		WithHeader("authorization", "Bearer hijack"),
		WithHeader("x-typesafe-retry-count", "99"))
	_, err := c.SystemOne(context.Background(), Request{State: "x", Questions: oneQuestion},
		RequestHeader("x-override", "call"), RequestHeader("user-agent", "nope"),
		RequestHeader("AUTHORIZATION", "Bearer hijack-too"))
	if err != nil {
		t.Fatal(err)
	}
	reqs := s.Requests()
	if len(reqs) != 2 {
		t.Fatalf("got %d requests, want 2", len(reqs))
	}
	h := reqs[0].Header
	checks := map[string]string{
		"Authorization":      "Bearer sk-test-key-123456",
		"Accept":             "application/json",
		"Content-Type":       "application/json",
		"User-Agent":         "typesafe-client-go/" + Version,
		"X-Typesafe-Sdk":     "typesafe-client-go/" + Version,
		"X-Agent-Client":     "client-default",
		"X-Override":         "call",
		"X-Typesafe-Runtime": runtimeHeader,
	}
	for k, want := range checks {
		if got := h.Get(k); got != want {
			t.Errorf("%s = %q, want %q", k, got, want)
		}
	}
	if !strings.HasPrefix(runtimeHeader, "go/") {
		t.Errorf("runtime header %q", runtimeHeader)
	}
	if got := h.Get("X-Typesafe-Retry-Count"); got != "" {
		t.Errorf("first attempt retry count = %q, want absent", got)
	}
	if got := reqs[1].Header.Get("X-Typesafe-Retry-Count"); got != "1" {
		t.Errorf("retry count = %q, want 1", got)
	}
}

func TestListModels(t *testing.T) {
	t.Parallel()
	s := newTestServer(t, func(_ int, r recorded, w http.ResponseWriter) {
		w.Header().Set("x-typesafe-request-id", "req_m")
		writeJSON(w, 200, map[string]any{"models": []any{
			map[string]any{"name": "jev-latest", "description": "Flagship", "release_date": "2026-09-15", "extra": 1},
		}})
	})
	c := newTestClient(t, s)
	got, err := c.ListModels(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	r := s.Requests()[0]
	if r.Method != http.MethodGet || r.Path != "/v1/models" || r.Header.Get("Content-Type") != "" || len(r.Body) != 0 {
		t.Errorf("request %s %s ct=%q body=%q", r.Method, r.Path, r.Header.Get("Content-Type"), r.Body)
	}
	if len(got.Models) != 1 || got.Models[0] != (ModelMetadata{"jev-latest", "Flagship", "2026-09-15"}) || got.RequestID != "req_m" {
		t.Errorf("got %+v", got)
	}
}

func TestListModelsValidation(t *testing.T) {
	t.Parallel()
	s := newTestServer(t, func(_ int, _ recorded, w http.ResponseWriter) {
		writeJSON(w, 200, map[string]any{"models": []any{map[string]any{"name": "a", "description": "b"}}})
	})
	c := newTestClient(t, s)
	_, err := c.ListModels(context.Background())
	var ve *ResponseValidationError
	if !errors.As(err, &ve) || ve.FieldPath != "models[0].release_date" {
		t.Fatalf("err = %v", err)
	}
}

func TestPerAttemptTimeout(t *testing.T) {
	t.Parallel()
	s := newTestServer(t, func(n int, r recorded, w http.ResponseWriter) {
		if n == 0 {
			time.Sleep(200 * time.Millisecond)
		}
		okHandler(n, r, w)
	})
	c := newTestClient(t, s, WithTimeout(50*time.Millisecond))
	resp, err := c.SystemOne(context.Background(), Request{State: "x", Questions: oneQuestion})
	if err != nil {
		t.Fatalf("timeout should be retried: %v", err)
	}
	if resp.Model != "jev-1.13.0" || len(s.Requests()) != 2 {
		t.Errorf("model %q, %d requests", resp.Model, len(s.Requests()))
	}

	_, err = c.SystemOne(context.Background(), Request{State: "x", Questions: oneQuestion},
		RequestRetryPolicy(NoRetries()), RequestTimeout(time.Nanosecond))
	var te *TimeoutError
	if !errors.As(err, &te) || !errors.Is(err, ErrTimeout) || !errors.Is(err, ErrConnection) {
		t.Fatalf("err = %v (%T), want *TimeoutError", err, err)
	}
	if te.Timeout != time.Nanosecond || !strings.Contains(te.Error(), "timed out") {
		t.Errorf("timeout error %v", te)
	}
}

func TestCallerCancellation(t *testing.T) {
	t.Parallel()
	s := newTestServer(t, func(n int, r recorded, w http.ResponseWriter) {
		time.Sleep(300 * time.Millisecond)
		okHandler(n, r, w)
	})
	c := newTestClient(t, s)
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Millisecond)
	defer cancel()
	_, err := c.SystemOne(ctx, Request{State: "x", Questions: oneQuestion})
	if !errors.Is(err, context.DeadlineExceeded) {
		t.Fatalf("err = %v, want context.DeadlineExceeded", err)
	}
	var te *TimeoutError
	if errors.As(err, &te) {
		t.Errorf("caller deadline must not be reported as *TimeoutError")
	}
	ctx2, cancel2 := context.WithCancel(context.Background())
	cancel2()
	if _, err := c.SystemOne(ctx2, Request{State: "x", Questions: oneQuestion}); !errors.Is(err, context.Canceled) {
		t.Errorf("err = %v, want context.Canceled", err)
	}
}

func TestLoggingRedactsCredentials(t *testing.T) {
	t.Parallel()
	s := newTestServer(t, okHandler)
	var buf bytes.Buffer
	logger := slog.New(slog.NewTextHandler(&buf, &slog.HandlerOptions{Level: slog.LevelDebug}))
	c := newTestClient(t, s, WithLogger(logger), WithHeader("Cookie", "session=secret"))
	if _, err := c.SystemOne(context.Background(), Request{State: "x", Questions: oneQuestion}); err != nil {
		t.Fatal(err)
	}
	out := buf.String()
	if strings.Contains(out, "sk-test-key-123456") || strings.Contains(out, "session=secret") {
		t.Errorf("credentials leaked into logs:\n%s", out)
	}
	for _, want := range []string{"Bearer ***3456", "status=200", "request_id=req_123", "Is this true?"} {
		if !strings.Contains(out, want) {
			t.Errorf("log missing %q:\n%s", want, out)
		}
	}
}

func TestRedactKey(t *testing.T) {
	t.Parallel()
	cases := map[string]string{
		"Bearer sk-abcdefghij": "Bearer ***ghij",
		"Bearer short":         "Bearer ***",
		"rawkey1234567":        "***4567",
	}
	for in, want := range cases {
		if got := redactKey(in); got != want {
			t.Errorf("redactKey(%q) = %q, want %q", in, got, want)
		}
	}
}

func TestRedactURL(t *testing.T) {
	t.Parallel()
	if got := redactURL("https://u:p@gw.example/typesafe/v1/systemone?key=x#f"); got != "https://gw.example/typesafe/v1/systemone" {
		t.Errorf("got %q", got)
	}
}
