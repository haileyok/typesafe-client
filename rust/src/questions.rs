//! Question builders and wire serialization.
//!
//! A request's `questions` field is a map of caller-chosen IDs to question
//! objects. The IDs are for code only — they are not sent to the model — and
//! the response uses the same IDs to key its answers.
//!
//! Each question is exactly one of three kinds, discriminated on the wire by a
//! `type` field (`"noul"`, `"choice"`, or `"score"`). Unset optional fields are
//! omitted from the wire, never sent as `null`; nulls the caller deliberately
//! places *inside* criteria are preserved.
//!
//! Choice option order is preserved exactly as supplied, because
//! `serde_json` is built with the `preserve_order` feature.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::errors::Error;

/// Optional descriptions of the yes and no outcomes of a noul question.
///
/// Either or both may be `None`, in which case the corresponding key is
/// omitted from the wire. Each present value may be a string, object, or
/// array — or deliberately `null` to leave that outcome undescribed while
/// still sending the criteria object.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct NoulCriteria {
    /// What counts as a yes answer. Serialized as `"true"`, omitted when `None`.
    #[serde(rename = "true", skip_serializing_if = "Option::is_none")]
    pub true_: Option<Value>,
    /// What counts as a no answer. Serialized as `"false"`, omitted when `None`.
    #[serde(rename = "false", skip_serializing_if = "Option::is_none")]
    pub false_: Option<Value>,
}

impl NoulCriteria {
    /// Creates criteria from optional descriptions of the yes and no outcomes.
    ///
    /// `Some(value)` sends that key; `None` omits it. Pass
    /// `Some(Value::Null)` to send an explicit `null` for a side.
    pub fn new(true_: Option<impl Into<Value>>, false_: Option<impl Into<Value>>) -> Self {
        Self {
            true_: true_.map(Into::into),
            false_: false_.map(Into::into),
        }
    }
}

/// A yes/no question, answered with the probability of yes.
///
/// ```no_run
/// # use typesafe_system_one::Noul;
/// let spam = Noul::new("Is this message spam?")
///     .criteria_true("Unsolicited advertising")
///     .criteria_false("A legitimate conversation");
/// ```
///
/// Instructions and criteria are optional on the wire; both accept strings,
/// objects, or arrays for advanced structure.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename = "noul")]
pub struct Noul {
    /// The question or statement to evaluate: string, object, or array.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<Value>,
    /// Optional descriptions of what counts as yes and no.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub criteria: Option<NoulCriteria>,
}

impl Noul {
    /// Creates a noul question with instructions and no criteria.
    pub fn new(instructions: impl Into<Value>) -> Self {
        Self {
            instructions: Some(instructions.into()),
            criteria: None,
        }
    }

    /// Creates a noul question with no instructions and no criteria.
    pub fn without_instructions() -> Self {
        Self::default()
    }

    /// Replaces the instructions.
    pub fn instructions(mut self, instructions: impl Into<Value>) -> Self {
        self.instructions = Some(instructions.into());
        self
    }

    /// Replaces the criteria from optional true/false descriptions.
    ///
    /// `Some(value)` sends that side; `None` omits it. Pass
    /// `Some(Value::Null)` to send an explicit `null` for a side.
    ///
    /// ```no_run
    /// # use typesafe_system_one::Noul;
    /// let urgent = Noul::new("Is this urgent?")
    ///     .criteria(Some("Time-sensitive"), None::<&str>);
    /// ```
    pub fn criteria(
        mut self,
        true_: Option<impl Into<Value>>,
        false_: Option<impl Into<Value>>,
    ) -> Self {
        self.criteria = Some(NoulCriteria::new(true_, false_));
        self
    }

    /// Replaces the criteria from a [`NoulCriteria`] value directly.
    pub fn criteria_value(mut self, criteria: NoulCriteria) -> Self {
        self.criteria = Some(criteria);
        self
    }

    /// Sets the "what counts as yes" side of the criteria, leaving the false
    /// side unset.
    pub fn criteria_true(mut self, true_: impl Into<Value>) -> Self {
        let criteria = self.criteria.take().unwrap_or_default();
        self.criteria = Some(NoulCriteria {
            true_: Some(true_.into()),
            ..criteria
        });
        self
    }

    /// Sets the "what counts as no" side of the criteria, leaving the true
    /// side unset.
    pub fn criteria_false(mut self, false_: impl Into<Value>) -> Self {
        let criteria = self.criteria.take().unwrap_or_default();
        self.criteria = Some(NoulCriteria {
            false_: Some(false_.into()),
            ..criteria
        });
        self
    }
}

/// A question that selects one option from choices the caller defines.
///
/// Options serialize in the order supplied, each mapped to a description that
/// is a string, object, array — or `null` for an option interpreted by its
/// name alone.
///
/// ```no_run
/// # use typesafe_system_one::Choice;
/// let tone = Choice::new("What is the tone of this message?")
///     .option("calm", "A neutral or polite message")
///     .option("angry", "An upset or hostile message");
/// ```
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename = "choice")]
pub struct Choice {
    /// What the model should decide when choosing an option.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<Value>,
    /// Options in the order the caller supplied them, mapped to descriptions
    /// (a JSON `null` description means "interpret the name alone").
    pub criteria: serde_json::Map<String, Value>,
}

impl Choice {
    /// Creates a choice question with instructions and no options yet.
    pub fn new(instructions: impl Into<Value>) -> Self {
        Self {
            instructions: Some(instructions.into()),
            criteria: serde_json::Map::new(),
        }
    }

    /// Creates a choice question with no instructions.
    pub fn without_instructions() -> Self {
        Self::default()
    }

    /// Creates a choice question whose options all have `null` descriptions.
    ///
    /// Options serialize in iteration order of `options`.
    pub fn from_options<I, S>(instructions: impl Into<Value>, options: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut criteria = serde_json::Map::new();
        for option in options {
            criteria.insert(option.into(), Value::Null);
        }
        Self {
            instructions: Some(instructions.into()),
            criteria,
        }
    }

    /// Creates a choice question from an ordered list of `(option, description)` pairs.
    ///
    /// Each description may be any JSON value; use `Value::Null` for an
    /// undescribed option.
    pub fn from_pairs<I, S>(instructions: impl Into<Value>, pairs: I) -> Self
    where
        I: IntoIterator<Item = (S, Value)>,
        S: Into<String>,
    {
        let mut criteria = serde_json::Map::new();
        for (option, description) in pairs {
            criteria.insert(option.into(), description);
        }
        Self {
            instructions: Some(instructions.into()),
            criteria,
        }
    }

    /// Replaces the instructions.
    pub fn instructions(mut self, instructions: impl Into<Value>) -> Self {
        self.instructions = Some(instructions.into());
        self
    }

    /// Appends an option with a description.
    ///
    /// The description may be a string, object, or array; pass
    /// `Value::Null` for an undescribed option.
    pub fn option(mut self, option: impl Into<String>, description: impl Into<Value>) -> Self {
        self.criteria.insert(option.into(), description.into());
        self
    }

    /// Appends an option whose description is `null` (interpreted by name alone).
    pub fn bare_option(self, option: impl Into<String>) -> Self {
        self.option(option, Value::Null)
    }
}

/// A question that rates the content against an ordered rubric.
///
/// Each level's position determines its score, starting at zero. The rubric
/// must have at least two levels and no level may be null; the client
/// validates both before any network I/O.
///
/// ```no_run
/// # use typesafe_system_one::Score;
/// let urgency = Score::new(
///     "How urgent is this?",
///     ["Can wait", "Needs attention this week", "Needs attention today"],
/// );
/// ```
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename = "score")]
pub struct Score {
    /// What the model should rate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<Value>,
    /// Ordered level descriptions, lowest first.
    pub criteria: Vec<Value>,
}

impl Score {
    /// Creates a score question with instructions and an ordered rubric.
    pub fn new<I>(instructions: impl Into<Value>, levels: I) -> Self
    where
        I: IntoIterator,
        I::Item: Into<Value>,
    {
        Self {
            instructions: Some(instructions.into()),
            criteria: levels.into_iter().map(Into::into).collect(),
        }
    }

    /// Creates a score question with no instructions.
    pub fn without_instructions<I>(levels: I) -> Self
    where
        I: IntoIterator,
        I::Item: Into<Value>,
    {
        Self {
            instructions: None,
            criteria: levels.into_iter().map(Into::into).collect(),
        }
    }

    /// Replaces the instructions.
    pub fn instructions(mut self, instructions: impl Into<Value>) -> Self {
        self.instructions = Some(instructions.into());
        self
    }
}

/// Any of the three question kinds, discriminated on the wire by `type`.
///
/// ```
/// # use typesafe_system_one::{Noul, Question};
/// let q: Question = Noul::new("Is this about billing?").into();
/// assert_eq!(
///     serde_json::to_value(&q).unwrap()["type"],
///     serde_json::json!("noul")
/// );
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
#[non_exhaustive]
pub enum Question {
    /// A yes/no question.
    Noul(Noul),
    /// A one-of-many selection.
    Choice(Choice),
    /// A rubric rating.
    Score(Score),
}

impl From<Noul> for Question {
    fn from(q: Noul) -> Self {
        Question::Noul(q)
    }
}

impl From<Choice> for Question {
    fn from(q: Choice) -> Self {
        Question::Choice(q)
    }
}

impl From<Score> for Question {
    fn from(q: Score) -> Self {
        Question::Score(q)
    }
}

pub(super) fn validate(
    state: &Value,
    questions: &std::collections::BTreeMap<String, Question>,
) -> Result<(), Error> {
    if questions.is_empty() {
        return Err(Error::InvalidRequest(
            "At least one question is required.".into(),
        ));
    }
    match state {
        Value::String(_) | Value::Object(_) | Value::Array(_) => {}
        Value::Number(_) => {
            return Err(Error::InvalidRequest(
                "state must be a JSON string, object, or array; numbers are rejected.".into(),
            ))
        }
        Value::Bool(_) => {
            return Err(Error::InvalidRequest(
                "state must be a JSON string, object, or array; booleans are rejected.".into(),
            ))
        }
        Value::Null => {
            return Err(Error::InvalidRequest(
                "state must be a JSON string, object, or array; null is rejected.".into(),
            ))
        }
    }
    for (name, question) in questions {
        match question {
            Question::Choice(choice) => {
                if choice.criteria.is_empty() {
                    return Err(Error::InvalidRequest(format!(
                        "Choice question \"{name}\" has no options; at least one is required."
                    )));
                }
            }
            Question::Score(score) => {
                if score.criteria.len() < 2 {
                    return Err(Error::InvalidRequest(format!(
                        "Score question \"{name}\" has {} {}; at least two are required.",
                        score.criteria.len(),
                        if score.criteria.len() == 1 {
                            "criterion"
                        } else {
                            "criteria"
                        }
                    )));
                }
                if let Some(position) = score.criteria.iter().position(Value::is_null) {
                    return Err(Error::InvalidRequest(format!(
                        "Score question \"{name}\" has a null level at position {position}; levels must be strings, objects, or arrays."
                    )));
                }
            }
            Question::Noul(_) => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::BTreeMap;

    #[test]
    fn serializes_noul_with_omitted_optionals() {
        let q = Noul::new("Is this spam?");
        let v = serde_json::to_value(&q).unwrap();
        assert_eq!(v, json!({"type": "noul", "instructions": "Is this spam?"}));
    }

    #[test]
    fn serializes_noul_criteria_renames_and_omits() {
        let q = Noul::new("Spam?").criteria(Some("Unsolicited"), None::<&str>);
        let v = serde_json::to_value(&q).unwrap();
        assert_eq!(
            v,
            json!({
                "type": "noul",
                "instructions": "Spam?",
                "criteria": {"true": "Unsolicited"}
            })
        );
        let q = Noul::without_instructions().criteria(None::<&str>, Some("Legit"));
        let v = serde_json::to_value(&q).unwrap();
        assert_eq!(v, json!({"type": "noul", "criteria": {"false": "Legit"}}));
    }

    #[test]
    fn preserves_null_criteria_inside_noul() {
        let q = Noul::new("Spam?").criteria(Some("Unsolicited"), Some(Value::Null));
        let v = serde_json::to_value(&q).unwrap();
        assert_eq!(v["criteria"], json!({"true": "Unsolicited", "false": null}));
    }

    #[test]
    fn preserves_choice_option_order() {
        let mut choice = Choice::new("Tone?")
            .option("b", "Second")
            .option("a", "First");
        choice = choice.bare_option("z").option("m", json!({"x": 1}));
        let v = serde_json::to_value(&choice).unwrap();
        assert_eq!(
            v["criteria"]
                .as_object()
                .unwrap()
                .keys()
                .collect::<Vec<_>>(),
            vec!["b", "a", "z", "m"]
        );
        assert_eq!(v["criteria"]["z"], json!(null));
        assert_eq!(v["criteria"]["m"], json!({"x": 1}));
    }

    #[test]
    fn from_options_uses_null_descriptions_in_order() {
        let q = Choice::from_options("Team?", ["billing", "tech", "sales"]);
        let v = serde_json::to_value(&q).unwrap();
        assert_eq!(
            v["criteria"]
                .as_object()
                .unwrap()
                .keys()
                .collect::<Vec<_>>(),
            vec!["billing", "tech", "sales"]
        );
        assert!(v["criteria"]["billing"].is_null());
    }

    #[test]
    fn score_serializes_ordered_levels() {
        let q = Score::new("Urgency?", ["Low", "High"]);
        let v = serde_json::to_value(&q).unwrap();
        assert_eq!(
            v,
            json!({"type": "score", "instructions": "Urgency?", "criteria": ["Low", "High"]})
        );
    }

    #[test]
    fn tag_serialization_round_trips() {
        for question in [
            Question::Noul(Noul::new("a")),
            Question::Choice(Choice::new("b").option("x", "y")),
            Question::Score(Score::new("c", ["1", "2"])),
        ] {
            let v = serde_json::to_value(&question).unwrap();
            let back: Question = serde_json::from_value(v).unwrap();
            assert_eq!(back, question);
        }
    }

    #[test]
    fn validation_rejects_bad_input() {
        let mut questions = BTreeMap::new();
        // Empty questions.
        let state = json!("text");
        assert!(validate(&state, &questions).is_err());
        // Number state.
        questions.insert("x".into(), Question::Noul(Noul::new("a")));
        for bad in [json!(1), json!(true), json!(null)] {
            let err = validate(&bad, &questions).unwrap_err();
            assert!(matches!(err, Error::InvalidRequest(_)), "{err}");
        }
        // Empty choice.
        questions.insert("c".into(), Question::Choice(Choice::new("c")));
        let err = validate(&state, &questions).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains('c') && msg.contains("at least one"), "{msg}");
        // One-level score.
        questions.remove("c");
        questions.insert("s".into(), Question::Score(Score::new("s", ["only"])));
        let err = validate(&state, &questions).unwrap_err();
        assert!(err.to_string().contains("at least two"), "{err}");
        // Null score level.
        let criteria: Vec<serde_json::Value> = vec!["a".into(), serde_json::Value::Null];
        questions.insert(
            "s".into(),
            Question::Score(Score {
                instructions: Some("s".into()),
                criteria,
            }),
        );
        let err = validate(&state, &questions).unwrap_err();
        assert!(err.to_string().contains("null level"), "{err}");
    }
}
