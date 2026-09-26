//! Quickstart: ask one of each question kind about a support ticket.
//!
//! Reads `TYPESAFE_API_KEY` (and optional `TYPESAFE_BASE_URL`,
//! `TYPESAFE_DEFAULT_MODEL`) from the environment.
//!
//! ```text
//! cargo run --example quickstart
//! ```

use typesafe_system_one::{Choice, Client, Noul, Score, SystemOneRequest};

#[tokio::main]
async fn main() -> Result<(), typesafe_system_one::Error> {
    let client = Client::from_env()?;

    let request = SystemOneRequest::new(
        "Subject: charged twice\nBody: I signed up last week and my card was charged twice.",
    )
    .question("billing", Noul::new("Is this about billing?"))
    .question(
        "tone",
        Choice::new("What is the tone of this message?")
            .option("calm", "A neutral or polite message")
            .option("angry", "An upset or hostile message"),
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
    );

    let response = client.system_one(request).await?;

    println!("model: {}", response.model);
    println!(
        "usage: {} in / {} out",
        response.usage.input_tokens, response.usage.output_tokens
    );
    if let Some(id) = &response.request_id {
        println!("request id: {id}");
    }
    if let Some(billing) = response.noul("billing") {
        println!("billing: {:.0}% yes", billing.noul * 100.0);
    }
    if let Some(tone) = response.choice("tone") {
        println!("tone: {} (confidence {:.2})", tone.choice, tone.confidence);
        for (option, probability) in tone.ranked() {
            println!("  {option}: {probability:.2}");
        }
    }
    if let Some(urgency) = response.score("urgency") {
        println!(
            "urgency: {:.2} (confidence {:.2})",
            urgency.score, urgency.confidence
        );
        for (level, description) in &urgency.legend {
            println!("  {level}: {description}");
        }
    }
    Ok(())
}
