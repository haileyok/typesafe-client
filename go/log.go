package typesafe

import (
	"context"
	"log/slog"
	"net/http"
	"os"
	"strings"
)

// secretHeaders are redacted from logs.
var secretHeaders = map[string]bool{
	"authorization":       true,
	"proxy-authorization": true,
	"x-api-key":           true,
	"api-key":             true,
	"cookie":              true,
	"set-cookie":          true,
}

// redactHeaders copies h with credential values masked. Key-like values
// keep their scheme and, for secrets longer than eight characters, the last
// four characters, so keys can be told apart without being revealed.
func redactHeaders(h http.Header) map[string]string {
	out := make(map[string]string, len(h))
	for k, v := range h {
		val := strings.Join(v, ", ")
		lower := strings.ToLower(k)
		switch {
		case lower == "cookie" || lower == "set-cookie":
			val = "***"
		case secretHeaders[lower]:
			val = redactKey(val)
		}
		out[k] = val
	}
	return out
}

func redactKey(v string) string {
	scheme, secret := "", v
	if i := strings.IndexByte(v, ' '); i >= 0 {
		scheme, secret = v[:i+1], strings.TrimSpace(v[i+1:])
	}
	tail := ""
	if len(secret) > 8 {
		tail = secret[len(secret)-4:]
	}
	return scheme + "***" + tail
}

// loggerFromEnv builds the default logger from TYPESAFE_LOG_LEVEL: unset or
// "off" discards everything; otherwise text to stderr at that level.
func loggerFromEnv() (*slog.Logger, error) {
	v := strings.ToLower(strings.TrimSpace(os.Getenv(EnvLogLevel)))
	var level slog.Level
	switch v {
	case "", "off":
		return slog.New(discardHandler{}), nil
	case "debug":
		level = slog.LevelDebug
	case "info":
		level = slog.LevelInfo
	case "warn", "warning":
		level = slog.LevelWarn
	case "error":
		level = slog.LevelError
	default:
		return nil, configf("invalid log level %q from %s; expected debug, info, warn, error, or off", v, EnvLogLevel)
	}
	return slog.New(slog.NewTextHandler(os.Stderr, &slog.HandlerOptions{Level: level})), nil
}

// discardHandler drops every record without formatting it.
type discardHandler struct{}

func (discardHandler) Enabled(context.Context, slog.Level) bool  { return false }
func (discardHandler) Handle(context.Context, slog.Record) error { return nil }
func (h discardHandler) WithAttrs([]slog.Attr) slog.Handler      { return h }
func (h discardHandler) WithGroup(string) slog.Handler           { return h }
