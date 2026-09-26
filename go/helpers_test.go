package typesafe

import (
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"sync"
	"testing"
	"time"
)

// recorded is one request seen by a test server.
type recorded struct {
	Method string
	Path   string
	Header http.Header
	Body   []byte
}

// testServer serves scripted responses and records requests.
type testServer struct {
	*httptest.Server
	mu       sync.Mutex
	requests []recorded
	handler  func(n int, r recorded, w http.ResponseWriter)
}

func newTestServer(t *testing.T, handler func(n int, r recorded, w http.ResponseWriter)) *testServer {
	t.Helper()
	s := &testServer{handler: handler}
	s.Server = httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, req *http.Request) {
		body, _ := io.ReadAll(req.Body)
		rec := recorded{Method: req.Method, Path: req.URL.Path, Header: req.Header.Clone(), Body: body}
		s.mu.Lock()
		s.requests = append(s.requests, rec)
		n := len(s.requests) - 1
		s.mu.Unlock()
		s.handler(n, rec, w)
	}))
	t.Cleanup(s.Close)
	return s
}

func (s *testServer) Requests() []recorded {
	s.mu.Lock()
	defer s.mu.Unlock()
	return append([]recorded(nil), s.requests...)
}

// fastRetry is the default policy with millisecond backoff.
func fastRetry() RetryPolicy {
	p := DefaultRetryPolicy()
	p.BackoffInitial = time.Millisecond
	p.BackoffMax = 2 * time.Millisecond
	return p
}

func newTestClient(t *testing.T, s *testServer, opts ...Option) *Client {
	t.Helper()
	base := []Option{WithAPIKey("sk-test-key-123456"), WithBaseURL(s.URL), WithModel("jev-latest"), WithRetryPolicy(fastRetry())}
	c, err := NewClient(append(base, opts...)...)
	if err != nil {
		t.Fatalf("NewClient: %v", err)
	}
	return c
}

func writeJSON(w http.ResponseWriter, status int, v any) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(v)
}

// okResponse is a valid System One response answering a noul "q" and a
// choice "tone" (extra answer IDs are ignored by the completeness check).
var okResponse = map[string]any{
	"model": "jev-1.13.0",
	"answers": map[string]any{
		"q": map[string]any{"type": "noul", "noul": 0.9},
		"tone": map[string]any{"type": "choice", "choice": "calm",
			"probabilities": map[string]any{"calm": 0.8, "angry": 0.2}, "confidence": 0.7},
	},
	"usage": map[string]any{"input_tokens": 12, "output_tokens": 3},
}

func okHandler(_ int, _ recorded, w http.ResponseWriter) {
	w.Header().Set("x-typesafe-request-id", "req_123")
	writeJSON(w, 200, okResponse)
}

var oneQuestion = Questions{"q": Noul("Is this true?")}

func mustJSON(t *testing.T, b []byte) map[string]any {
	t.Helper()
	var m map[string]any
	if err := json.Unmarshal(b, &m); err != nil {
		t.Fatalf("unmarshal %s: %v", b, err)
	}
	return m
}
