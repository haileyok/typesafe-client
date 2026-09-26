//! [`SystemOneRequest`] builder.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::errors::Error;
use crate::questions::{validate, Question};

/// A request to answer questions about some content.
///
/// The `state` is the content to evaluate — anything that serializes to a
/// JSON string, object, or array (numbers, booleans, and `null` are rejected
/// by client-side validation). Questions are keyed by caller-chosen IDs that
/// the response uses to key its answers.
///
/// ```
/// # use typesafe_client::{Noul, Score, SystemOneRequest};
/// let request = SystemOneRequest::new("Please help.")
///     .model("jev-1.13.0")
///     .question("billing", Noul::new("Is this about billing?"))
///     .question(
///         "urgency",
///         Score::new("How urgent?", ["Can wait", "Needs attention today"]),
///     );
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct SystemOneRequest {
    /// The content to evaluate: JSON string, object, or array.
    pub state: Value,
    /// Model name or alias; `None` uses the client's default model.
    pub model: Option<String>,
    /// Questions keyed by caller-chosen IDs.
    pub questions: BTreeMap<String, Question>,
}

impl SystemOneRequest {
    /// Creates a request from state that is already a JSON value.
    ///
    /// The state must serialize to a JSON string, object, or array;
    /// numbers, booleans, and `null` are rejected by
    /// [`Client::system_one`](crate::Client::system_one) before any network
    /// I/O.
    pub fn new(state: impl Into<Value>) -> Self {
        Self {
            state: state.into(),
            model: None,
            questions: BTreeMap::new(),
        }
    }

    /// Creates a request by serializing any `Serialize` value as the state.
    ///
    /// The serialized state must be a JSON string, object, or array.
    pub fn with_state_serialize<T: serde::Serialize>(state: &T) -> Result<Self, Error> {
        let value = serde_json::to_value(state).map_err(|error| {
            Error::InvalidRequest(format!("state failed to serialize: {error}"))
        })?;
        if !matches!(value, Value::String(_) | Value::Object(_) | Value::Array(_)) {
            return Err(Error::InvalidRequest(
                "state must serialize to a JSON string, object, or array.".into(),
            ));
        }
        Ok(Self::new(value))
    }

    /// Overrides the model for this request.
    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// Adds a question under a caller-chosen ID.
    pub fn question(mut self, id: impl Into<String>, question: impl Into<Question>) -> Self {
        self.questions.insert(id.into(), question.into());
        self
    }

    /// Adds every question in an iterable of `(id, question)` pairs.
    pub fn questions<I, S>(mut self, questions: I) -> Self
    where
        I: IntoIterator<Item = (S, Question)>,
        S: Into<String>,
    {
        for (id, question) in questions {
            self.questions.insert(id.into(), question);
        }
        self
    }

    /// Validates the request client-side, before any network I/O.
    pub(crate) fn validate(&self) -> Result<(), Error> {
        validate(&self.state, &self.questions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::questions::{Choice, Noul, Score};
    use serde_json::json;

    #[derive(serde::Serialize)]
    struct Ticket {
        subject: String,
        body: String,
    }

    #[test]
    fn with_state_serialize_accepts_objects() {
        let ticket = Ticket {
            subject: "Refund".into(),
            body: "Charged twice".into(),
        };
        let request = SystemOneRequest::with_state_serialize(&ticket).unwrap();
        assert_eq!(
            request.state,
            json!({"subject": "Refund", "body": "Charged twice"})
        );
    }

    #[test]
    fn with_state_serialize_rejects_primitives() {
        let err = SystemOneRequest::with_state_serialize(&42).unwrap_err();
        assert!(matches!(err, Error::InvalidRequest(_)));
        let err = SystemOneRequest::with_state_serialize(&true).unwrap_err();
        assert!(matches!(err, Error::InvalidRequest(_)));
        let err = SystemOneRequest::with_state_serialize::<Option<u8>>(&None).unwrap_err();
        assert!(matches!(err, Error::InvalidRequest(_)));
        let err = SystemOneRequest::with_state_serialize(&"string is fine").unwrap();
        assert_eq!(err.state, json!("string is fine"));
    }

    #[test]
    fn builder_methods() {
        let request = SystemOneRequest::new("text")
            .model("jev-1.13.0")
            .question("a", Noul::new("q1"))
            .question("b", Choice::new("q2").option("x", "y"))
            .question("c", Score::new("q3", ["1", "2"]));
        assert_eq!(request.model.as_deref(), Some("jev-1.13.0"));
        assert_eq!(request.questions.len(), 3);
        assert_eq!(request.questions["a"], Question::Noul(Noul::new("q1")));
        assert!(request.validate().is_ok());
    }
}
