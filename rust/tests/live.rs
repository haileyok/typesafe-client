//! Live tests against the real API. Ignored unless `TYPESAFE_API_KEY` is set:
//!
//! ```text
//! cargo test --test live -- --ignored
//! ```

use typesafe_client::{Choice, Client, Noul, Score, SystemOneRequest};

fn maybe_client() -> Option<Client> {
    let key = std::env::var("TYPESAFE_API_KEY")
        .ok()
        .map(|key| key.trim().to_owned())
        .filter(|key| !key.is_empty())?;
    Client::builder().api_key(key).build().ok()
}

#[tokio::test]
#[ignore = "live test: runs only with TYPESAFE_API_KEY set"]
async fn live_system_one_round_trip() {
    let Some(client) = maybe_client() else {
        panic!("TYPESAFE_API_KEY is required for the live test");
    };

    let response = client
        .system_one(
            SystemOneRequest::new(
                "Subject: charged twice\nBody: I signed up last week and my card was charged twice.",
            )
            .question("billing", Noul::new("Is this about billing?"))
            .question(
                "tone",
                Choice::new("What is the tone?")
                    .option("calm", "A neutral or polite message")
                    .option("angry", "An upset or hostile message"),
            )
            .question(
                "urgency",
                Score::new(
                    "How urgent is this?",
                    ["Can wait", "Needs attention this week", "Needs attention today"],
                ),
            ),
        )
        .await
        .expect("live request");

    assert!(!response.model.is_empty());
    assert!(response.noul("billing").is_some());
    let tone = response.choice("tone").expect("tone answer");
    assert!(tone.confidence >= 0.0 && tone.confidence <= 1.0);
    let urgency = response.score("urgency").expect("urgency answer");
    assert!(urgency.score >= 0.0 && urgency.score <= 2.0);
    println!("live response: {response:?}");
}

#[tokio::test]
#[ignore = "live test: runs only with TYPESAFE_API_KEY set"]
async fn live_list_models() {
    let Some(client) = maybe_client() else {
        panic!("TYPESAFE_API_KEY is required for the live test");
    };
    let models = client.list_models().await.expect("live models");
    assert!(!models.is_empty());
    assert!(models.iter().any(|model| model.name == "jev-latest"));
    println!("live models: {models:?}");
}
