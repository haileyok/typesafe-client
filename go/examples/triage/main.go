// Command triage demonstrates speculative fan-out and confidence-gated
// routing (https://docs.typesafe.ai/patterns/fan-out): every question the
// router might need is asked in one request, and code decides which answers
// matter. Run with TYPESAFE_API_KEY set:
//
//	go run ./examples/triage "I was charged twice and can't log in"
package main

import (
	"context"
	"errors"
	"fmt"
	"log"
	"os"
	"strings"
	"time"

	typesafe "github.com/haileyok/typesafe-client/go"
)

// Questions and thresholds live in one place so they are easy to review.
var questions = typesafe.Questions{
	"category": typesafe.Choice("Determine the broad category of this support ticket.",
		typesafe.Opt("bug_report", "The user is reporting something that is broken or producing errors"),
		typesafe.Opt("billing", "Charges, invoices, refunds, subscriptions"),
		typesafe.Opt("feature_request", "The user is requesting new functionality"),
		typesafe.Opt("account", "Login, permissions, profile, security"),
		typesafe.Opt("other", "None of the above"),
	),
	// Speculative: only matters for bug reports.
	"bug_severity": typesafe.Score("If this ticket reports a bug, how severe is it?",
		"Cosmetic; no impact to functionality",
		"Broken or degraded feature; workaround exists",
		"Blocking issue; no workaround exists",
	),
	"has_repro_steps": typesafe.Noul("The user describes specific steps to reproduce the issue."),
	// Speculative: only matters for billing.
	"refund_requested": typesafe.Noul("The user is explicitly asking for a refund or credit."),
	"frustration": typesafe.Score("How frustrated does the user appear?",
		"Calm, matter-of-fact", "Frustrated but civil", "Very angry"),
}

const (
	minCategoryConfidence = 0.5 // below this, a human routes the ticket
	highSeverity          = 1.5
	refundLikely          = 0.7
	priorityFrustration   = 1.5
)

func main() {
	ticket := "Hi, I placed an order (#98423) last Thursday and was charged twice. " +
		"I also can't log in after the site update. This is getting frustrating."
	if len(os.Args) > 1 {
		ticket = strings.Join(os.Args[1:], " ")
	}

	client, err := typesafe.NewClient()
	if err != nil {
		log.Fatal(err)
	}
	ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
	defer cancel()

	resp, err := client.SystemOne(ctx, typesafe.Request{State: map[string]string{"ticket": ticket}, Questions: questions})
	switch {
	case errors.Is(err, typesafe.ErrRateLimit), errors.Is(err, typesafe.ErrInternalServer), errors.Is(err, typesafe.ErrConnection):
		log.Fatalf("TypeSafe unavailable after retries, falling back to manual triage: %v", err)
	case err != nil:
		log.Fatal(err)
	}

	category, _ := resp.Choice("category")
	severity, _ := resp.Score("bug_severity")
	repro, _ := resp.Noul("has_repro_steps")
	refund, _ := resp.Noul("refund_requested")
	frustration, _ := resp.Score("frustration")

	fmt.Printf("category %s (confidence %.2f)\n", category.Choice, category.Confidence)
	switch {
	case category.Confidence < minCategoryConfidence:
		fmt.Println("→ uncertain: route to a human")
	case category.Choice == "bug_report" && severity.Score > highSeverity && repro.Noul > 0.6:
		fmt.Println("→ escalate to engineering (high severity, reproducible)")
	case category.Choice == "bug_report":
		fmt.Println("→ bug backlog")
	case category.Choice == "billing" && refund.Noul > refundLikely:
		fmt.Println("→ billing, refund likely")
	default:
		fmt.Printf("→ %s queue\n", category.Choice)
	}
	if frustration.Score > priorityFrustration {
		fmt.Println("→ flag for priority response")
	}
}
