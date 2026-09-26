package typesafe

import (
	"errors"
	"strings"
	"testing"
	"time"
)

// Config tests use t.Setenv, so they cannot run in parallel. Go runs all
// sequential top-level tests before resuming parallel ones, so the process
// environment is stable while parallel tests call NewClient.

func TestConfigFromEnv(t *testing.T) {
	t.Setenv(EnvAPIKey, "  sk-env  ")
	t.Setenv(EnvBaseURL, "https://gw.example/typesafe//")
	t.Setenv(EnvDefaultModel, "jev-1.13.0")
	t.Setenv(EnvLogLevel, "")
	c, err := NewClient()
	if err != nil {
		t.Fatal(err)
	}
	if c.apiKey != "sk-env" || c.baseURL != "https://gw.example/typesafe" || c.model != "jev-1.13.0" {
		t.Errorf("got key=%q base=%q model=%q", c.apiKey, c.baseURL, c.model)
	}
	if c.timeout != DefaultTimeout {
		t.Errorf("timeout = %s", c.timeout)
	}
}

func TestConfigExplicitWinsAndBlankEnvIgnored(t *testing.T) {
	t.Setenv(EnvAPIKey, "sk-env")
	t.Setenv(EnvBaseURL, "   ")
	t.Setenv(EnvDefaultModel, "\t")
	c, err := NewClient(WithAPIKey("sk-explicit"), WithModel("m"))
	if err != nil {
		t.Fatal(err)
	}
	if c.apiKey != "sk-explicit" || c.baseURL != DefaultBaseURL || c.model != "m" {
		t.Errorf("got key=%q base=%q model=%q", c.apiKey, c.baseURL, c.model)
	}
	c, err = NewClient()
	if err != nil {
		t.Fatal(err)
	}
	if c.model != DefaultModel {
		t.Errorf("model = %q, want default", c.model)
	}
}

func TestConfigAPIKeyValidation(t *testing.T) {
	t.Setenv(EnvAPIKey, "")
	cases := map[string]string{
		"":             "no API key",
		"   ":          "no API key",
		"sk bad":       "printable ASCII",
		"sk\x00bad":    "printable ASCII",
		"sk-ключ":      "printable ASCII",
		"sk\tbad\nkey": "printable ASCII",
	}
	for key, want := range cases {
		_, err := NewClient(WithAPIKey(key))
		if !errors.Is(err, ErrConfig) || !strings.Contains(err.Error(), want) {
			t.Errorf("key %q: err = %v, want ErrConfig containing %q", key, err, want)
		}
		if key != "" && strings.TrimSpace(key) != "" && strings.Contains(err.Error(), key) {
			t.Errorf("error echoes the key: %v", err)
		}
	}
}

func TestConfigInvalidOptions(t *testing.T) {
	t.Setenv(EnvAPIKey, "sk-x")
	bad := RetryPolicy{BackoffJitter: 2}
	for name, opt := range map[string]Option{
		"timeout":  WithTimeout(0),
		"retry":    WithRetryPolicy(bad),
		"negative": WithRetryPolicy(RetryPolicy{MaxRetries: -1}),
		"status":   WithRetryPolicy(RetryPolicy{HTTPStatuses: []int{42}}),
		"http":     WithHTTPClient(nil),
		"baseurl":  WithBaseURL("not a url"),
		"relative": WithBaseURL("api.internal"),
		"scheme":   WithBaseURL("ftp://api.example"),
		"query":    WithBaseURL("https://gw.example/typesafe?key=1"),
		"emptyq":   WithBaseURL("https://gw.example/?"),
		"fragment": WithBaseURL("https://gw.example/#frag"),
		"nohost":   WithBaseURL("https:///v1"),
	} {
		if _, err := NewClient(opt); !errors.Is(err, ErrConfig) {
			t.Errorf("%s: err = %v, want ErrConfig", name, err)
		}
	}
	if _, err := NewClient(WithTimeout(time.Second)); err != nil {
		t.Errorf("valid timeout: %v", err)
	}
	for _, u := range []string{"http://localhost:8080", "https://gw.example/typesafe/", "https://u:p@gw.example"} {
		if _, err := NewClient(WithBaseURL(u)); err != nil {
			t.Errorf("valid base URL %q: %v", u, err)
		}
	}
}

func TestConfigLogLevelEnv(t *testing.T) {
	t.Setenv(EnvAPIKey, "sk-x")
	for _, lvl := range []string{"debug", "INFO", "warn", "error", "off", ""} {
		t.Setenv(EnvLogLevel, lvl)
		if _, err := NewClient(); err != nil {
			t.Errorf("level %q: %v", lvl, err)
		}
	}
	t.Setenv(EnvLogLevel, "loud")
	if _, err := NewClient(); !errors.Is(err, ErrConfig) {
		t.Errorf("err = %v, want ErrConfig", err)
	}
}
