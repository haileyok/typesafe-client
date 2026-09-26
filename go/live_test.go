package typesafe

import (
	"context"
	"os"
	"strings"
	"testing"
	"time"
)

// TestLive calls the real API. It runs only when TYPESAFE_API_KEY is set
// and TYPESAFE_LIVE_TEST=1, so ordinary test runs never spend tokens.
func TestLive(t *testing.T) {
	t.Parallel()
	if os.Getenv("TYPESAFE_LIVE_TEST") != "1" || strings.TrimSpace(os.Getenv(EnvAPIKey)) == "" {
		t.Skip("set TYPESAFE_API_KEY and TYPESAFE_LIVE_TEST=1 to run live tests")
	}
	c, err := NewClient()
	if err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithTimeout(context.Background(), time.Minute)
	defer cancel()

	models, err := c.ListModels(ctx)
	if err != nil || len(models.Models) == 0 {
		t.Fatalf("ListModels: %+v %v", models, err)
	}

	resp, err := c.SystemOne(ctx, Request{
		State: "Help! My payouts have been failing for 3 days.",
		Questions: Questions{
			"urgent": Noul("Does this convey urgency?"),
			"dept":   ChoiceNames("Which team should handle this?", "billing", "technical", "sales"),
			"mood":   Score("How frustrated is the customer?", "Calm", "Frustrated", "Very angry"),
		},
	})
	if err != nil {
		t.Fatal(err)
	}
	if resp.Model == "" || resp.Usage.InputTokens == 0 {
		t.Errorf("metadata: %+v", resp)
	}
	n, ok1 := resp.Noul("urgent")
	ch, ok2 := resp.Choice("dept")
	sc, ok3 := resp.Score("mood")
	if !ok1 || !ok2 || !ok3 || n.Noul < 0 || n.Noul > 1 || len(ch.Probabilities) != 3 || len(sc.Legend) != 3 {
		t.Errorf("answers: %+v", resp.Answers)
	}
	t.Logf("model=%s request=%s urgent=%.2f dept=%s(%.2f) mood=%.2f", resp.Model, resp.RequestID, n.Noul, ch.Choice, ch.Confidence, sc.Score)
}
