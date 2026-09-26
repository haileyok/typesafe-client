// Command quickstart asks one of each question type about a support
// message. Run with TYPESAFE_API_KEY set:
//
//	go run ./examples/quickstart
package main

import (
	"context"
	"fmt"
	"log"
	"time"

	typesafe "github.com/haileyok/typesafe-client/go"
)

func main() {
	client, err := typesafe.NewClient()
	if err != nil {
		log.Fatal(err)
	}
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()

	resp, err := client.SystemOne(ctx, typesafe.Request{
		State: "Help! My payouts have been failing for 3 days.",
		Questions: typesafe.Questions{
			"is_urgent": typesafe.Noul("Does this convey urgency?").
				WithCriteria("Explicitly time-sensitive", "No urgency expressed"),
			"department": typesafe.Choice("Which team should handle this?",
				typesafe.Opt("billing", "Payments, invoicing, refunds"),
				typesafe.Opt("technical", "Bugs, outages, integrations"),
				typesafe.Opt("sales", "Pricing, upgrades, new accounts"),
			),
			"frustration": typesafe.Score("How frustrated is the customer?",
				"Calm", "Frustrated", "Very angry"),
		},
	})
	if err != nil {
		log.Fatal(err)
	}

	urgent, _ := resp.Noul("is_urgent")
	dept, _ := resp.Choice("department")
	frustration, _ := resp.Score("frustration")

	fmt.Printf("model:       %s (request %s)\n", resp.Model, resp.RequestID)
	fmt.Printf("urgent:      p(yes)=%.2f\n", urgent.Noul)
	fmt.Printf("department:  %s (confidence %.2f)\n", dept.Choice, dept.Confidence)
	for _, o := range dept.Ranked() {
		fmt.Printf("               %-10s %.2f\n", o.Option, o.Probability)
	}
	fmt.Printf("frustration: %.2f on 0–2 (confidence %.2f)\n", frustration.Score, frustration.Confidence)
	fmt.Printf("usage:       %d input tokens\n", resp.Usage.InputTokens)
}
