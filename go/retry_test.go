package typesafe

import (
	"context"
	"errors"
	"net"
	"net/http"
	"strconv"
	"testing"
	"time"
)

func TestDefaultRetryPolicyMatchesOfficialSDKs(t *testing.T) {
	t.Parallel()
	p := DefaultRetryPolicy()
	if p.MaxRetries != 2 || p.BackoffInitial != 500*time.Millisecond || p.BackoffMax != 5*time.Second ||
		p.BackoffJitter != 0.25 || !p.RespectRetryAfter || p.MaxRetryAfter != time.Minute ||
		!p.RetryConnectionErrors || !p.RetryTimeouts || p.TotalBudget != 30*time.Second {
		t.Errorf("policy = %+v", p)
	}
	for _, s := range []int{408, 429, 500, 502, 503, 504, 529, 599} {
		if !p.retriesStatus(s) {
			t.Errorf("status %d not retried", s)
		}
	}
	for _, s := range []int{400, 401, 403, 404, 409, 422, 600} {
		if p.retriesStatus(s) {
			t.Errorf("status %d retried", s)
		}
	}
	if NoRetries().MaxRetries != 0 {
		t.Errorf("NoRetries retries")
	}
}

func TestBackoff(t *testing.T) {
	t.Parallel()
	zero := func() float64 { return 0 }
	cases := []struct {
		attempt int
		want    time.Duration
	}{{0, 500 * time.Millisecond}, {1, time.Second}, {2, 2 * time.Second}, {3, 4 * time.Second}, {4, 5 * time.Second}, {40, 5 * time.Second}, {200, 5 * time.Second}}
	for _, tc := range cases {
		if got := backoff(tc.attempt, 500*time.Millisecond, 5*time.Second, 0.25, zero); got != tc.want {
			t.Errorf("attempt %d: got %s want %s", tc.attempt, got, tc.want)
		}
	}
	// Maximum jitter subtracts up to 25%.
	almostOne := func() float64 { return 0.999999 }
	if got := backoff(0, time.Second, 5*time.Second, 0.25, almostOne); got < 750*time.Millisecond || got > 751*time.Millisecond {
		t.Errorf("jittered = %s", got)
	}
	if backoff(3, 0, time.Second, 0, zero) != 0 || backoff(3, time.Second, 0, 0, zero) != 0 {
		t.Errorf("zero initial/max must disable backoff")
	}
}

func TestParseRetryAfter(t *testing.T) {
	t.Parallel()
	now := time.Date(2026, 9, 26, 12, 0, 0, 0, time.UTC)
	mk := func(kv ...string) http.Header {
		h := http.Header{}
		for i := 0; i+1 < len(kv); i += 2 {
			k, v := kv[i], kv[i+1]
			h.Set(k, v)
		}
		return h
	}
	cases := []struct {
		name string
		h    http.Header
		want time.Duration
		ok   bool
	}{
		{"none", mk(), 0, false},
		{"ms", mk("retry-after-ms", "250.5"), 250500 * time.Microsecond, true},
		{"ms preferred", mk("retry-after-ms", "100", "Retry-After", "9"), 100 * time.Millisecond, true},
		{"ms negative falls through", mk("retry-after-ms", "-1", "Retry-After", "2"), 2 * time.Second, true},
		{"ms garbage falls through", mk("retry-after-ms", "soon", "Retry-After", "1.5"), 1500 * time.Millisecond, true},
		{"seconds", mk("Retry-After", "3"), 3 * time.Second, true},
		{"empty is zero", mk("Retry-After", ""), 0, true},
		{"negative seconds", mk("Retry-After", "-5"), 0, false},
		{"date", mk("Retry-After", now.Add(4*time.Second).Format(http.TimeFormat)), 4 * time.Second, true},
		{"past date", mk("Retry-After", now.Add(-time.Hour).Format(http.TimeFormat)), 0, true},
		{"garbage", mk("Retry-After", "later"), 0, false},
		{"infinite", mk("Retry-After", "Inf"), 0, false},
		{"huge", mk("Retry-After", "1e300"), 0, false},
		{"huge ms", mk("retry-after-ms", "1e300"), 0, false},
		// Exactly 2^63ns once scaled: must not wrap to a negative Duration.
		{"exactly 2^63ns", mk("Retry-After", "9223372036.854775808"), 0, false},
	}
	for _, tc := range cases {
		got, ok := parseRetryAfter(tc.h, now)
		if got != tc.want || ok != tc.ok {
			t.Errorf("%s: got %s %v, want %s %v", tc.name, got, ok, tc.want, tc.ok)
		}
	}
}

func TestRetryDelayUsesRetryAfterUpToCap(t *testing.T) {
	t.Parallel()
	p := DefaultRetryPolicy()
	zero := func() float64 { return 0 }
	h := http.Header{}
	h.Set("Retry-After", "30")
	if d := p.delay(0, h, time.Now(), zero); d != 30*time.Second {
		t.Errorf("honored delay = %s", d)
	}
	h.Set("Retry-After", "120")
	if d := p.delay(0, h, time.Now(), zero); d != 500*time.Millisecond {
		t.Errorf("over-cap delay should fall back to backoff, got %s", d)
	}
	p.RespectRetryAfter = false
	h.Set("Retry-After", "1")
	if d := p.delay(1, h, time.Now(), zero); d != time.Second {
		t.Errorf("ignored retry-after, got %s", d)
	}
}

func statusThenOK(statuses ...int) func(int, recorded, http.ResponseWriter) {
	return func(n int, r recorded, w http.ResponseWriter) {
		if n < len(statuses) {
			writeJSON(w, statuses[n], map[string]any{"error": "fail " + strconv.Itoa(statuses[n])})
			return
		}
		okHandler(n, r, w)
	}
}

func TestRetriesRetryableStatuses(t *testing.T) {
	t.Parallel()
	for _, st := range []int{408, 429, 500, 502, 503, 529} {
		t.Run(strconv.Itoa(st), func(t *testing.T) {
			t.Parallel()
			s := newTestServer(t, statusThenOK(st, st))
			c := newTestClient(t, s)
			if _, err := c.SystemOne(context.Background(), Request{State: "x", Questions: oneQuestion}); err != nil {
				t.Fatalf("err = %v", err)
			}
			if n := len(s.Requests()); n != 3 {
				t.Errorf("requests = %d, want 3", n)
			}
		})
	}
}

func TestDoesNotRetryClientErrors(t *testing.T) {
	t.Parallel()
	for _, st := range []int{400, 401, 403, 404, 422} {
		t.Run(strconv.Itoa(st), func(t *testing.T) {
			t.Parallel()
			s := newTestServer(t, statusThenOK(st))
			c := newTestClient(t, s)
			_, err := c.SystemOne(context.Background(), Request{State: "x", Questions: oneQuestion})
			var apiErr *APIError
			if !errors.As(err, &apiErr) || apiErr.StatusCode != st {
				t.Fatalf("err = %v", err)
			}
			if n := len(s.Requests()); n != 1 {
				t.Errorf("requests = %d, want 1", n)
			}
		})
	}
}

func TestRetriesExhaustedReturnsLastError(t *testing.T) {
	t.Parallel()
	s := newTestServer(t, statusThenOK(529, 529, 503, 529))
	c := newTestClient(t, s)
	_, err := c.SystemOne(context.Background(), Request{State: "x", Questions: oneQuestion})
	var apiErr *APIError
	if !errors.As(err, &apiErr) || apiErr.StatusCode != 503 || !errors.Is(err, ErrInternalServer) {
		t.Fatalf("err = %v, want the third attempt's 503", err)
	}
	if n := len(s.Requests()); n != 3 {
		t.Errorf("requests = %d, want 3 (1 + MaxRetries)", n)
	}
}

func TestPerCallRetryPolicy(t *testing.T) {
	t.Parallel()
	s := newTestServer(t, statusThenOK(429, 429, 429, 429))
	c := newTestClient(t, s)
	p := fastRetry()
	p.MaxRetries = 4
	if _, err := c.SystemOne(context.Background(), Request{State: "x", Questions: oneQuestion}, RequestRetryPolicy(p)); err != nil {
		t.Fatal(err)
	}
	if n := len(s.Requests()); n != 5 {
		t.Errorf("requests = %d, want 5", n)
	}
	if _, err := c.SystemOne(context.Background(), Request{State: "x", Questions: oneQuestion}, RequestRetryPolicy(RetryPolicy{BackoffJitter: 9})); !errors.Is(err, ErrInvalidRequest) {
		t.Errorf("invalid per-call policy: %v", err)
	}
}

func TestHonorsRetryAfterMS(t *testing.T) {
	t.Parallel()
	s := newTestServer(t, func(n int, r recorded, w http.ResponseWriter) {
		if n == 0 {
			w.Header().Set("retry-after-ms", "80")
			writeJSON(w, 429, map[string]any{"error": "slow down"})
			return
		}
		okHandler(n, r, w)
	})
	c := newTestClient(t, s)
	start := time.Now()
	if _, err := c.SystemOne(context.Background(), Request{State: "x", Questions: oneQuestion}); err != nil {
		t.Fatal(err)
	}
	if el := time.Since(start); el < 80*time.Millisecond {
		t.Errorf("retried after %s, want >= 80ms", el)
	}
}

func TestBudgetStopReturnsLastRealError(t *testing.T) {
	t.Parallel()
	s := newTestServer(t, func(_ int, _ recorded, w http.ResponseWriter) {
		w.Header().Set("Retry-After", "5")
		writeJSON(w, 529, map[string]any{"error": "overloaded"})
	})
	p := fastRetry()
	p.TotalBudget = time.Second // a 5s Retry-After cannot fit
	c := newTestClient(t, s, WithRetryPolicy(p))
	start := time.Now()
	_, err := c.SystemOne(context.Background(), Request{State: "x", Questions: oneQuestion})
	var apiErr *APIError
	if !errors.As(err, &apiErr) || apiErr.StatusCode != 529 {
		t.Fatalf("err = %v, want the 529", err)
	}
	if time.Since(start) > 500*time.Millisecond || len(s.Requests()) != 1 {
		t.Errorf("slept instead of stopping: %s, %d requests", time.Since(start), len(s.Requests()))
	}
}

func TestContextDeadlineStopReturnsLastRealError(t *testing.T) {
	t.Parallel()
	s := newTestServer(t, func(_ int, _ recorded, w http.ResponseWriter) {
		w.Header().Set("Retry-After", "2")
		writeJSON(w, 429, map[string]any{"error": "rate limited"})
	})
	c := newTestClient(t, s)
	ctx, cancel := context.WithTimeout(context.Background(), 500*time.Millisecond)
	defer cancel()
	_, err := c.SystemOne(ctx, Request{State: "x", Questions: oneQuestion})
	if !errors.Is(err, ErrRateLimit) {
		t.Fatalf("err = %v, want the 429 rather than a deadline error", err)
	}
}

func TestRetriesConnectionErrors(t *testing.T) {
	t.Parallel()
	// A listener that is closed immediately yields connection refused.
	ln, err := (&net.ListenConfig{}).Listen(context.Background(), "tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	addr := ln.Addr().String()
	_ = ln.Close()
	var attempts int
	rt := roundTripFunc(func(r *http.Request) (*http.Response, error) {
		attempts++
		return http.DefaultTransport.RoundTrip(r)
	})
	c, err := NewClient(WithAPIKey("sk-x"), WithBaseURL("http://"+addr), WithRetryPolicy(fastRetry()), WithHTTPClient(&http.Client{Transport: rt}))
	if err != nil {
		t.Fatal(err)
	}
	_, err = c.SystemOne(context.Background(), Request{State: "x", Questions: oneQuestion})
	var ce *ConnectionError
	if !errors.As(err, &ce) || !errors.Is(err, ErrConnection) {
		t.Fatalf("err = %v (%T), want *ConnectionError", err, err)
	}
	if attempts != 3 {
		t.Errorf("attempts = %d, want 3", attempts)
	}

	attempts = 0
	p := fastRetry()
	p.RetryConnectionErrors = false
	_, _ = c.SystemOne(context.Background(), Request{State: "x", Questions: oneQuestion}, RequestRetryPolicy(p))
	if attempts != 1 {
		t.Errorf("attempts = %d with connection retries disabled, want 1", attempts)
	}
}

type roundTripFunc func(*http.Request) (*http.Response, error)

func (f roundTripFunc) RoundTrip(r *http.Request) (*http.Response, error) { return f(r) }
