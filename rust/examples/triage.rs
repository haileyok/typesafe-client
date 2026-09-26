//! Speculative fan-out: ask a choice, a score, and a noul question about the
//! same content in ONE request, then route the ticket in ordinary code.
//!
//! System One answers mixed question kinds in one round trip, which makes it
//! a cheap "speculative fan-out": gather routing signals up front, and only
//! escalate what needs a human or a larger model.
//!
//! ```text
//! cargo run --example triage
//! ```

use typesafe_client::{Choice, Client, Noul, Score, SystemOneRequest};

#[tokio::main]
async fn main() -> Result<(), typesafe_client::Error> {
    let client = Client::from_env()?;

    let request = SystemOneRequest::new(
        "Subject: refund\nBody: You charged me twice for the same subscription. \
         This is ridiculous and I want my money back TODAY.",
    )
    .question(
        "team",
        Choice::new("Which team should handle this?")
            .option("billing", "Charges, refunds, subscriptions, invoices")
            .option("tech", "Bugs, outages, broken features")
            .option("legal", "Compliance, legal threats, regulation"),
    )
    .question(
        "urgency",
        Score::new(
            "How urgent is this?",
            [
                "Can wait",
                "Needs attention this week",
                "Needs attention today",
            ],
        ),
    )
    .question("refund", Noul::new("Is the customer asking for a refund?"));

    let response = client.system_one(request).await?;

    // Route on the choice, gating on confidence.
    let team = match response.choice("team") {
        Some(choice) if choice.confidence >= 0.7 => choice.choice.clone(),
        _ => "general".to_owned(), // Low confidence: a human triages.
    };

    // Escalate on the score, gating on confidence.
    let needs_today = response
        .score("urgency")
        .is_some_and(|score| score.confidence >= 0.6 && score.score >= 2.0);

    // Cheap boolean signal, no confidence to gate on for noul answers.
    let wants_refund = response
        .noul("refund")
        .map(|noul| noul.noul > 0.5)
        .unwrap_or(false);

    println!("route to:        {team}");
    println!("needs attention today: {needs_today}");
    println!("wants refund:    {wants_refund}");
    println!(
        "usage: {} in / {} out",
        response.usage.input_tokens, response.usage.output_tokens
    );
    Ok(())
}
