package typesafe

import (
	"errors"
	"math"
	"net/http"
	"strconv"
	"strings"
	"time"
)

// RetryPolicy controls how failed attempts are retried. Start from
// DefaultRetryPolicy and adjust fields; the zero value disables every retry
// rule.
//
//	p := typesafe.DefaultRetryPolicy()
//	p.MaxRetries = 4
//	client, err := typesafe.NewClient(typesafe.WithRetryPolicy(p))
type RetryPolicy struct {
	// MaxRetries is the number of retries after the first attempt; 0
	// disables retries.
	MaxRetries int
	// BackoffInitial is the first backoff delay, doubled on each retry up to
	// BackoffMax. Either being zero disables backoff delays.
	BackoffInitial time.Duration
	// BackoffMax caps the exponential backoff delay.
	BackoffMax time.Duration
	// BackoffJitter is the fraction of each backoff delay randomly
	// subtracted, between 0 and 1.
	BackoffJitter float64
	// HTTPStatuses lists the response statuses that are retried.
	HTTPStatuses []int
	// RespectRetryAfter honors the server's retry-after-ms / Retry-After
	// headers, up to MaxRetryAfter.
	RespectRetryAfter bool
	// MaxRetryAfter is the longest server-requested delay honored; a longer
	// one falls back to the computed backoff.
	MaxRetryAfter time.Duration
	// RetryConnectionErrors retries attempts that produced no HTTP response
	// (*ConnectionError).
	RetryConnectionErrors bool
	// RetryTimeouts retries attempts that exceeded the per-attempt timeout
	// (*TimeoutError).
	RetryTimeouts bool
	// TotalBudget limits retrying, measured from the first attempt. A retry
	// is not started when the elapsed time plus its delay would reach or
	// exceed the budget; the last real error is returned instead. An attempt
	// that has started runs to its own per-attempt timeout, so a call can
	// overrun the budget by up to one timeout (the same semantics as the
	// official Python SDK). The caller's context deadline gets the same
	// check and is also a hard bound. Zero disables the budget.
	TotalBudget time.Duration
}

// DefaultRetryPolicy returns the policy used by TypeSafe's official SDKs:
// two retries, 500ms doubling to a 5s cap with 25% jitter, retrying 408,
// 429, and 5xx responses plus connection failures and timeouts, honoring
// Retry-After up to 60s, within a 30s total budget.
func DefaultRetryPolicy() RetryPolicy {
	statuses := []int{http.StatusRequestTimeout, http.StatusTooManyRequests}
	for s := 500; s < 600; s++ {
		statuses = append(statuses, s)
	}
	return RetryPolicy{
		MaxRetries:            2,
		BackoffInitial:        500 * time.Millisecond,
		BackoffMax:            5 * time.Second,
		BackoffJitter:         0.25,
		HTTPStatuses:          statuses,
		RespectRetryAfter:     true,
		MaxRetryAfter:         60 * time.Second,
		RetryConnectionErrors: true,
		RetryTimeouts:         true,
		TotalBudget:           30 * time.Second,
	}
}

// NoRetries returns DefaultRetryPolicy with retries disabled.
func NoRetries() RetryPolicy {
	p := DefaultRetryPolicy()
	p.MaxRetries = 0
	return p
}

func (p RetryPolicy) validate() error {
	switch {
	case p.MaxRetries < 0:
		return configf("retry MaxRetries must be non-negative, got %d", p.MaxRetries)
	case p.BackoffInitial < 0 || p.BackoffMax < 0 || p.MaxRetryAfter < 0 || p.TotalBudget < 0:
		return configf("retry durations must be non-negative")
	case math.IsNaN(p.BackoffJitter) || p.BackoffJitter < 0 || p.BackoffJitter > 1:
		return configf("retry BackoffJitter must be between 0 and 1, got %v", p.BackoffJitter)
	}
	for _, s := range p.HTTPStatuses {
		if s < 100 || s > 999 {
			return configf("retry HTTPStatuses must contain HTTP status codes, got %d", s)
		}
	}
	return nil
}

func (p RetryPolicy) retriesStatus(status int) bool {
	for _, s := range p.HTTPStatuses {
		if s == status {
			return true
		}
	}
	return false
}

// retryable reports whether err (from one attempt) may be retried.
func (p RetryPolicy) retryable(err error) bool {
	var apiErr *APIError
	var timeoutErr *TimeoutError
	var connErr *ConnectionError
	switch {
	case errors.As(err, &apiErr):
		return p.retriesStatus(apiErr.StatusCode)
	case errors.As(err, &timeoutErr):
		return p.RetryTimeouts
	case errors.As(err, &connErr):
		return p.RetryConnectionErrors
	}
	return false
}

// delay returns the wait before retry number attempt (0-based). header is
// the failed response's headers (nil for transport errors); random returns
// a value in [0, 1).
func (p RetryPolicy) delay(attempt int, header http.Header, now time.Time, random func() float64) time.Duration {
	if p.RespectRetryAfter && header != nil {
		if d, ok := parseRetryAfter(header, now); ok && d <= p.MaxRetryAfter {
			return d
		}
	}
	return backoff(attempt, p.BackoffInitial, p.BackoffMax, p.BackoffJitter, random)
}

// backoff is min(initial*2^attempt, max) * (1 - random()*jitter).
func backoff(attempt int, initial, maxDelay time.Duration, jitter float64, random func() float64) time.Duration {
	if initial <= 0 || maxDelay <= 0 {
		return 0
	}
	exp := maxDelay
	// Shifting past 62 bits overflows; by then the cap has long applied.
	if attempt < 62 {
		if d := initial << attempt; d > 0 && d < maxDelay && d>>attempt == initial {
			exp = d
		}
	}
	return time.Duration(math.Round(float64(exp) * (1 - random()*jitter)))
}

// parseRetryAfter reads retry-after-ms (milliseconds, may be fractional)
// in preference to Retry-After (seconds, may be fractional, or an HTTP
// date). Negative, non-finite, or unparseable values are treated as absent.
func parseRetryAfter(h http.Header, now time.Time) (time.Duration, bool) {
	if h == nil {
		return 0, false
	}
	// Like the official SDKs, an empty value reads as zero.
	if vals, ok := h[http.CanonicalHeaderKey(headerRetryAfterMS)]; ok && len(vals) > 0 {
		v := strings.TrimSpace(vals[0])
		if v == "" {
			v = "0"
		}
		if ms, err := strconv.ParseFloat(v, 64); err == nil && ms >= 0 && !math.IsInf(ms, 0) {
			return durationFrom(ms, float64(time.Millisecond))
		}
	}
	vals, ok := h[http.CanonicalHeaderKey(headerRetryAfter)]
	if !ok || len(vals) == 0 {
		return 0, false
	}
	v := strings.TrimSpace(vals[0])
	if v == "" {
		v = "0"
	}
	if secs, err := strconv.ParseFloat(v, 64); err == nil {
		if secs < 0 || math.IsInf(secs, 0) || math.IsNaN(secs) {
			return 0, false
		}
		return durationFrom(secs, float64(time.Second))
	}
	if t, err := http.ParseTime(v); err == nil {
		return max(t.Sub(now), 0), true
	}
	return 0, false
}

func durationFrom(v, unit float64) (time.Duration, bool) {
	d := v * unit
	// float64(math.MaxInt64) rounds up to 2^63, which itself overflows
	// time.Duration, so the comparison must be >=.
	if math.IsNaN(d) || d >= float64(math.MaxInt64) {
		return 0, false
	}
	return time.Duration(d), true
}
