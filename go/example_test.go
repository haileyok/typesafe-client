package typesafe_test

import (
	"context"
	"errors"
	"fmt"
	"log"
	"time"

	typesafe "github.com/haileyok/typesafe-client/go"
)

func ExampleClient_SystemOne() {
	client, err := typesafe.NewClient() // reads TYPESAFE_API_KEY
	if err != nil {
		log.Fatal(err)
	}
	resp, err := client.SystemOne(context.Background(), typesafe.Request{
		State: map[string]any{
			"ticket_message": "My flight was cancelled. Can I get a refund?",
			"refund_policy":  "Cancelled flights are eligible for a full refund.",
		},
		Questions: typesafe.Questions{
			"refund_requested": typesafe.Noul("Does `ticket_message` request a refund?"),
			"request_type": typesafe.Choice("What is the main request in `ticket_message`?",
				typesafe.Opt("refund", "The customer wants money returned."),
				typesafe.Opt("rebooking", "The customer wants a replacement flight."),
				typesafe.Opt("information", "The customer is asking for information only."),
			),
		},
	})
	if err != nil {
		log.Fatal(err)
	}
	refund, _ := resp.Noul("refund_requested")
	kind, _ := resp.Choice("request_type")
	fmt.Println(refund.Noul > 0.5, kind.Choice, kind.Confidence)
}

func ExampleChoice_structured() {
	// Option descriptions may be structured to sharpen the boundaries
	// between options; options are sent in the order given.
	_ = typesafe.Choice(map[string]any{
		"question": "Which team should handle this message?",
		"focus":    "Classify the customer's primary request, not every topic mentioned.",
	},
		typesafe.Opt("billing", map[string]any{
			"what":     "Charges, invoices, refunds, or subscriptions",
			"not_for":  "Order tracking or account access",
			"examples": []string{"I was charged twice", "Where is my refund?"},
		}),
		typesafe.Opt("orders", map[string]any{
			"what":    "Order status, delivery, cancellation, or returns",
			"not_for": "Charges or account access",
		}),
		typesafe.Bare("other"),
	)
}

func ExampleRetryPolicy() {
	p := typesafe.DefaultRetryPolicy()
	p.MaxRetries = 4
	p.TotalBudget = 15 * time.Second
	client, err := typesafe.NewClient(
		typesafe.WithRetryPolicy(p),
		typesafe.WithTimeout(5*time.Second),
	)
	if err != nil {
		log.Fatal(err)
	}
	_, err = client.SystemOne(context.Background(), typesafe.Request{
		State:     "…",
		Questions: typesafe.Questions{"q": typesafe.Noul("…")},
	})
	var apiErr *typesafe.APIError
	switch {
	case errors.Is(err, typesafe.ErrRateLimit):
		// Still rate limited after retries.
	case errors.As(err, &apiErr):
		log.Printf("status %d, request %s: %s", apiErr.StatusCode, apiErr.RequestID, apiErr.Message)
	}
}

func ExampleWithBaseURL_gateway() {
	// Any gateway implementing the TypeSafe OpenAPI spec works, e.g.
	// OpenRouter with an OpenRouter key and model ID.
	_, _ = typesafe.NewClient(
		typesafe.WithAPIKey("sk-or-..."),
		typesafe.WithBaseURL("https://openrouter.ai/api"),
		typesafe.WithModel("~typesafe/jev-latest"),
	)
}
