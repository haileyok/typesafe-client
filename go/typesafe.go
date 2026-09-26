// Package typesafe is an unofficial Go client for the TypeSafe AI System One
// API (https://docs.typesafe.ai). System One models, including TypeSafe's
// flagship model Jev, evaluate a state (text or JSON) against a set of named,
// typed questions and return structured answers instead of generated text:
//
//   - Noul: a yes/no question; the answer is the probability of yes.
//   - Choice: pick one option from a set; the answer is the chosen option, a
//     probability for every option, and a confidence.
//   - Score: rate against ordered levels; the answer is a probability-weighted
//     position, a probability for every level, and a confidence.
//
// A minimal call:
//
//	client, err := typesafe.NewClient() // reads TYPESAFE_API_KEY
//	if err != nil { ... }
//	resp, err := client.SystemOne(ctx, typesafe.Request{
//		State: "Help! My payouts have been failing for 3 days.",
//		Questions: typesafe.Questions{
//			"is_urgent":  typesafe.Noul("Does this convey urgency?"),
//			"department": typesafe.ChoiceNames("Which team should handle this?", "billing", "technical", "sales"),
//		},
//	})
//	if err != nil { ... }
//	urgent, _ := resp.Noul("is_urgent")
//	dept, _ := resp.Choice("department")
//	fmt.Println(urgent.Noul, dept.Choice, dept.Confidence)
//
// The client mirrors the behavior of TypeSafe's official Python and
// JavaScript SDKs: environment-based configuration, retries with
// exponential backoff and jitter for 408/429/5xx and transport failures,
// Retry-After handling, typed errors, and response validation. See SPEC.md in
// the repository for the full behavior contract.
//
// This package is not affiliated with or endorsed by TypeSafe.
package typesafe

// Version is the client version, sent in the User-Agent and X-TypeSafe-SDK
// headers.
const Version = "0.1.0"

// Defaults shared with TypeSafe's official SDKs.
const (
	// DefaultBaseURL is the TypeSafe API root.
	DefaultBaseURL = "https://api.typesafe.ai"
	// DefaultModel is TypeSafe's flagship System One model alias. Pin a
	// versioned ID such as "jev-1.13.0" when you have tuned thresholds
	// against a specific model version.
	DefaultModel = "jev-latest"
)

// Environment variables consulted by NewClient. Explicit options take
// precedence; empty or whitespace-only values are ignored.
const (
	EnvAPIKey       = "TYPESAFE_API_KEY" //nolint:gosec // G101: an environment variable name, not a credential.
	EnvBaseURL      = "TYPESAFE_BASE_URL"
	EnvDefaultModel = "TYPESAFE_DEFAULT_MODEL"
	EnvLogLevel     = "TYPESAFE_LOG_LEVEL"
)

const (
	systemOnePath = "/v1/systemone"
	modelsPath    = "/v1/models"
	sdkName       = "typesafe-client-go"

	headerRequestID    = "X-Typesafe-Request-Id"
	headerRetryCount   = "X-TypeSafe-Retry-Count"
	headerRetryAfter   = "Retry-After"
	headerRetryAfterMS = "Retry-After-Ms"
)
