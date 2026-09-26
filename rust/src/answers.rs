//! Response types: answers, usage, models, and hand-rolled decoding with
//! dotted-path validation errors.
//!
//! Answers are decoded by hand from [`serde_json::Value`] (not via derive) so
//! that every validation failure can carry a dotted field path such as
//! `answers.tone.confidence`, per the SPEC. An answer with an unrecognized
//! `type` is not an error: it is kept as an opaque [`Answer::Unknown`] that
//! carries its raw JSON.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::errors::Error;

/// The answer to a yes/no question: the probability of yes (0 to 1).
///
/// There is no confidence value for noul answers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NoulAnswer {
    /// Probability of a yes answer, from 0 to 1. Near 0.5 indicates uncertainty.
    pub noul: f64,
}

/// The answer to a choice question.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChoiceAnswer {
    /// The name of the option with the highest probability.
    pub choice: String,
    /// Probability of each option, from 0 to 1; the values sum to ~1.
    pub probabilities: BTreeMap<String, f64>,
    /// Confidence in the selected option, from 0 to 1.
    pub confidence: f64,
}

impl ChoiceAnswer {
    /// Returns `(option, probability)` pairs sorted by probability,
    /// descending.
    pub fn ranked(&self) -> Vec<(&str, f64)> {
        let mut ranked: Vec<(&str, f64)> = self
            .probabilities
            .iter()
            .map(|(option, probability)| (option.as_str(), *probability))
            .collect();
        ranked.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.cmp(b.0))
        });
        ranked
    }
}

/// The answer to a score question: a probability-weighted average of the
/// rubric levels.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScoreAnswer {
    /// The expected score. It may fall between integer levels.
    pub score: f64,
    /// Confidence in the score, from 0 to 1.
    pub confidence: f64,
    /// The requested criteria mapped to their score levels, keyed by
    /// integer level index.
    pub legend: BTreeMap<u32, Value>,
    /// Probability of each score level, keyed by integer level index.
    pub probabilities: BTreeMap<u32, f64>,
}

/// Any answer, discriminated by its `type` field.
///
/// Unknown future types are kept losslessly as [`Answer::Unknown`].
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Answer {
    /// A yes/no answer.
    Noul(NoulAnswer),
    /// A one-of-many selection.
    Choice(ChoiceAnswer),
    /// A rubric rating.
    Score(ScoreAnswer),
    /// An answer whose `type` this client does not know, carrying its raw
    /// JSON and the type name.
    Unknown {
        /// The unrecognized `type` value.
        kind: String,
        /// The raw JSON of the answer.
        raw: Value,
    },
}

impl From<NoulAnswer> for Answer {
    fn from(answer: NoulAnswer) -> Self {
        Self::Noul(answer)
    }
}

impl From<ChoiceAnswer> for Answer {
    fn from(answer: ChoiceAnswer) -> Self {
        Self::Choice(answer)
    }
}

impl From<ScoreAnswer> for Answer {
    fn from(answer: ScoreAnswer) -> Self {
        Self::Score(answer)
    }
}

/// Token usage for a request. Both fields default to 0 when the server
/// omits them, because some gateways omit usage.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    /// Number of billable input tokens used to evaluate the request.
    pub input_tokens: u64,
    /// Number of output tokens used to answer the questions. Currently free.
    pub output_tokens: u64,
}

/// The response to a [`SystemOneRequest`](crate::SystemOneRequest).
#[derive(Debug, Clone, PartialEq)]
pub struct SystemOneResponse {
    /// The versioned ID of the model that actually answered, which may
    /// differ from the alias supplied in the request.
    pub model: String,
    /// Answers keyed by the question IDs supplied in the request.
    pub answers: BTreeMap<String, Answer>,
    /// Token usage, defaulting to 0 per field when the server omits it.
    pub usage: Usage,
    /// The `x-typesafe-request-id` response header, when present.
    pub request_id: Option<String>,
}

impl SystemOneResponse {
    /// Returns the noul answer for `id`, when that question was answered as
    /// a noul.
    pub fn noul(&self, id: &str) -> Option<&NoulAnswer> {
        match self.answers.get(id)? {
            Answer::Noul(answer) => Some(answer),
            _ => None,
        }
    }

    /// Returns the choice answer for `id`, when that question was answered
    /// as a choice.
    pub fn choice(&self, id: &str) -> Option<&ChoiceAnswer> {
        match self.answers.get(id)? {
            Answer::Choice(answer) => Some(answer),
            _ => None,
        }
    }

    /// Returns the score answer for `id`, when that question was answered as
    /// a score.
    pub fn score(&self, id: &str) -> Option<&ScoreAnswer> {
        match self.answers.get(id)? {
            Answer::Score(answer) => Some(answer),
            _ => None,
        }
    }

    /// Iterates every `(id, noul answer)` pair.
    pub fn nouls(&self) -> impl Iterator<Item = (&str, &NoulAnswer)> {
        self.answers.iter().filter_map(|(id, answer)| match answer {
            Answer::Noul(answer) => Some((id.as_str(), answer)),
            _ => None,
        })
    }

    /// Iterates every `(id, choice answer)` pair.
    pub fn choices(&self) -> impl Iterator<Item = (&str, &ChoiceAnswer)> {
        self.answers.iter().filter_map(|(id, answer)| match answer {
            Answer::Choice(answer) => Some((id.as_str(), answer)),
            _ => None,
        })
    }

    /// Iterates every `(id, score answer)` pair.
    pub fn scores(&self) -> impl Iterator<Item = (&str, &ScoreAnswer)> {
        self.answers.iter().filter_map(|(id, answer)| match answer {
            Answer::Score(answer) => Some((id.as_str(), answer)),
            _ => None,
        })
    }
}

/// Metadata describing one available model or alias.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelMetadata {
    /// Model name or alias accepted by a request's `model` field.
    pub name: String,
    /// Human-readable description of the model and its capabilities.
    pub description: String,
    /// Model release date, formatted as `YYYY-MM-DD`.
    pub release_date: String,
}

/// A response validation failure with its dotted field path.
fn validation_error(
    status: u16,
    field_path: &str,
    request_id: Option<String>,
    endpoint: &str,
) -> Error {
    Error::ResponseValidation {
        status,
        field_path: field_path.to_owned(),
        request_id,
        endpoint: endpoint.to_owned(),
    }
}

/// Builds the validation error for a failed 2xx decode, including the
/// response's request ID.
fn decode_error(response: &crate::client::RawResponse, field_path: &str) -> Error {
    validation_error(
        response.status,
        field_path,
        response.request_id.clone(),
        &response.endpoint,
    )
}

/// Decodes a number field, reporting `path` when missing or not a number.
fn number(value: &Value, path: &str) -> Result<f64, String> {
    match value {
        Value::Number(number) => number.as_f64().ok_or_else(|| path.to_owned()),
        _ => Err(path.to_owned()),
    }
}

/// Decodes a string field, reporting `path` when missing or not a string.
fn string(value: &Value, path: &str) -> Result<String, String> {
    match value {
        Value::String(string) => Ok(string.clone()),
        _ => Err(path.to_owned()),
    }
}

/// Decodes a `{key: number}` map.
fn number_map(value: &Value, path: &str) -> Result<BTreeMap<String, f64>, String> {
    let Value::Object(map) = value else {
        return Err(path.to_owned());
    };
    let mut result = BTreeMap::new();
    for (key, value) in map {
        let key_path = format!("{path}.{key}");
        result.insert(key.clone(), number(value, &key_path)?);
    }
    Ok(result)
}

/// Decodes a `{"<level>": number}` map with decimal level-index keys into
/// integer keys.
fn level_number_map(value: &Value, path: &str) -> Result<BTreeMap<u32, f64>, String> {
    let Value::Object(map) = value else {
        return Err(path.to_owned());
    };
    let mut result = BTreeMap::new();
    for (key, value) in map {
        let key_path = format!("{path}.{key}");
        let level: u32 = key.parse().map_err(|_| key_path.clone())?;
        result.insert(level, number(value, &key_path)?);
    }
    Ok(result)
}

/// Decodes a `{"<level>": string|object|array}` legend map into integer keys.
fn level_value_map(value: &Value, path: &str) -> Result<BTreeMap<u32, Value>, String> {
    let Value::Object(map) = value else {
        return Err(path.to_owned());
    };
    let mut result = BTreeMap::new();
    for (key, value) in map {
        let key_path = format!("{path}.{key}");
        // Legend values are string, object, or array (OpenAPI
        // `ScoreAnswer.legend`); null, booleans, and numbers are invalid.
        if !matches!(value, Value::String(_) | Value::Object(_) | Value::Array(_)) {
            return Err(key_path);
        }
        let level: u32 = key.parse().map_err(|_| key_path.clone())?;
        result.insert(level, value.clone());
    }
    Ok(result)
}

/// Decodes one answer, reporting `answers.<id>.<field>` paths on failure.
fn decode_answer(id: &str, value: &Value) -> Result<Answer, String> {
    let Value::Object(answer) = value else {
        return Err(format!("answers.{id}"));
    };
    let answer_type = match answer.get("type") {
        Some(Value::String(answer_type)) => answer_type.as_str(),
        _ => return Err(format!("answers.{id}.type")),
    };
    let field = |name: &str| answer.get(name);
    match answer_type {
        "noul" => {
            let noul = field("noul").ok_or_else(|| format!("answers.{id}.noul"))?;
            Ok(Answer::Noul(NoulAnswer {
                noul: number(noul, &format!("answers.{id}.noul"))?,
            }))
        }
        "choice" => {
            let choice = field("choice").ok_or_else(|| format!("answers.{id}.choice"))?;
            let probabilities =
                field("probabilities").ok_or_else(|| format!("answers.{id}.probabilities"))?;
            let confidence =
                field("confidence").ok_or_else(|| format!("answers.{id}.confidence"))?;
            Ok(Answer::Choice(ChoiceAnswer {
                choice: string(choice, &format!("answers.{id}.choice"))?,
                probabilities: number_map(probabilities, &format!("answers.{id}.probabilities"))?,
                confidence: number(confidence, &format!("answers.{id}.confidence"))?,
            }))
        }
        "score" => {
            let score = field("score").ok_or_else(|| format!("answers.{id}.score"))?;
            let confidence =
                field("confidence").ok_or_else(|| format!("answers.{id}.confidence"))?;
            let legend = field("legend").ok_or_else(|| format!("answers.{id}.legend"))?;
            let probabilities =
                field("probabilities").ok_or_else(|| format!("answers.{id}.probabilities"))?;
            Ok(Answer::Score(ScoreAnswer {
                score: number(score, &format!("answers.{id}.score"))?,
                confidence: number(confidence, &format!("answers.{id}.confidence"))?,
                legend: level_value_map(legend, &format!("answers.{id}.legend"))?,
                probabilities: level_number_map(
                    probabilities,
                    &format!("answers.{id}.probabilities"),
                )?,
            }))
        }
        other => {
            tracing::warn!(
                answer_id = %id,
                answer_type = %other,
                "keeping answer with unrecognized type as Unknown"
            );
            Ok(Answer::Unknown {
                kind: other.to_owned(),
                raw: value.clone(),
            })
        }
    }
}

use serde::{Deserialize, Serialize};

impl SystemOneResponse {
    /// Decodes a 2xx response body, validating its structure.
    ///
    /// The body must be a JSON object with `model` (string) and `answers`
    /// (object). Each answer must be an object with a string `type` and
    /// every field its known `type` requires. Failures produce a
    /// [`Error::ResponseValidation`] with a dotted field path and the HTTP
    /// status. Unrecognized answer types are kept as [`Answer::Unknown`].
    pub(super) fn decode(response: &crate::client::RawResponse) -> Result<Self, Error> {
        let body: Value =
            serde_json::from_str(&response.body).map_err(|_| decode_error(response, ""))?;
        let Value::Object(object) = &body else {
            return Err(decode_error(response, ""));
        };
        let model = string(
            object
                .get("model")
                .ok_or_else(|| decode_error(response, "model"))?,
            "model",
        )
        .map_err(|_| decode_error(response, "model"))?;
        let answers_value = object
            .get("answers")
            .ok_or_else(|| decode_error(response, "answers"))?;
        let answers_object = match answers_value {
            Value::Object(map) => map,
            _ => return Err(decode_error(response, "answers")),
        };
        let mut answers: BTreeMap<String, Answer> = BTreeMap::new();
        for (id, answer_value) in answers_object {
            let decoded = decode_answer(id.as_str(), answer_value)
                .map_err(|field_path| decode_error(response, &field_path))?;
            answers.insert(id.clone(), decoded);
        }
        // Usage may be absent (some gateways omit it) and so may either
        // field, defaulting to 0; a present value of the wrong type is a
        // validation error, as in the Go client and the Python SDK.
        let usage = match object.get("usage") {
            None | Some(Value::Null) => Usage::default(),
            Some(Value::Object(usage)) => {
                let tokens = |key: &str| -> Result<u64, Error> {
                    match usage.get(key) {
                        None | Some(Value::Null) => Ok(0),
                        Some(value) => value
                            .as_u64()
                            .ok_or_else(|| decode_error(response, &format!("usage.{key}"))),
                    }
                };
                Usage {
                    input_tokens: tokens("input_tokens")?,
                    output_tokens: tokens("output_tokens")?,
                }
            }
            Some(_) => return Err(decode_error(response, "usage")),
        };
        Ok(Self {
            model,
            answers,
            usage,
            request_id: response.request_id.clone(),
        })
    }
}

/// Decodes a `GET /v1/models` response.
/// Requires an answer for every requested question, and that an answer of a
/// known type matches its question's type. Unknown (newer) answer types and
/// extra answer IDs pass through. Without this, a typed accessor on a
/// missing answer returns `None`, which is easy to mistake for a real "no".
pub(super) fn check_complete(
    response: &crate::client::RawResponse,
    answers: &BTreeMap<String, Answer>,
    questions: &BTreeMap<String, crate::questions::Question>,
) -> Result<(), Error> {
    use crate::questions::Question;
    // BTreeMap iteration is sorted, so the first problem reported is
    // deterministic.
    for (id, question) in questions {
        let Some(answer) = answers.get(id) else {
            return Err(decode_error(response, &format!("answers.{id}")));
        };
        let matches = matches!(
            (question, answer),
            (Question::Noul(_), Answer::Noul(_))
                | (Question::Choice(_), Answer::Choice(_))
                | (Question::Score(_), Answer::Score(_))
                | (_, Answer::Unknown { .. })
        );
        if !matches {
            return Err(decode_error(response, &format!("answers.{id}.type")));
        }
    }
    Ok(())
}

pub(super) fn decode_models(
    response: &crate::client::RawResponse,
) -> Result<Vec<ModelMetadata>, Error> {
    let body: Value =
        serde_json::from_str(&response.body).map_err(|_| decode_error(response, ""))?;
    let Value::Object(object) = &body else {
        return Err(decode_error(response, ""));
    };
    let Value::Array(models) = object
        .get("models")
        .ok_or_else(|| decode_error(response, "models"))?
    else {
        return Err(decode_error(response, "models"));
    };
    let mut result = Vec::with_capacity(models.len());
    for (index, model) in models.iter().enumerate() {
        // Same path format as the Go client and the Python SDK:
        // `models[0].release_date`.
        let path = format!("models[{index}]");
        let Value::Object(model) = model else {
            return Err(decode_error(response, &path));
        };
        let field = |key: &str| -> Result<String, Error> {
            let field_path = format!("{path}.{key}");
            let value = model
                .get(key)
                .ok_or_else(|| decode_error(response, &field_path))?;
            string(value, &field_path).map_err(|p| decode_error(response, &p))
        };
        result.push(ModelMetadata {
            name: field("name")?,
            description: field("description")?,
            release_date: field("release_date")?,
        });
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(body: &str) -> crate::client::RawResponse {
        crate::client::RawResponse {
            status: 200,
            body: body.to_owned(),
            request_id: None,
            endpoint: "POST https://api.typesafe.ai/v1/systemone".into(),
        }
    }

    #[test]
    fn decodes_all_three_answer_kinds() {
        let body = r#"{
            "model": "jev-1.13.0",
            "answers": {
                "spam": {"type": "noul", "noul": 0.98},
                "tone": {"type": "choice", "choice": "angry", "probabilities": {"angry": 0.8, "calm": 0.1, "excited": 0.1}, "confidence": 0.9},
                "urgency": {"type": "score", "score": 1.7, "confidence": 0.9, "legend": {"0": "Can wait", "1": "This week", "2": "Today"}, "probabilities": {"0": 0.1, "1": 0.1, "2": 0.8}}
            },
            "usage": {"input_tokens": 120, "output_tokens": 12}
        }"#;
        let response = SystemOneResponse::decode(&raw(body)).unwrap();
        assert_eq!(response.model, "jev-1.13.0");
        assert_eq!(response.noul("spam").unwrap().noul, 0.98);
        let tone = response.choice("tone").unwrap();
        assert_eq!(tone.choice, "angry");
        assert_eq!(tone.confidence, 0.9);
        assert_eq!(tone.probabilities["angry"], 0.8);
        let urgency = response.score("urgency").unwrap();
        assert_eq!(urgency.score, 1.7);
        assert_eq!(urgency.legend[&2], "Today");
        assert_eq!(urgency.probabilities[&2], 0.8);
        assert_eq!(response.usage.input_tokens, 120);
        assert_eq!(response.usage.output_tokens, 12);
        assert_eq!(response.nouls().count(), 1);
        assert_eq!(response.choices().count(), 1);
        assert_eq!(response.scores().count(), 1);
        assert!(response.noul("tone").is_none());
    }

    #[test]
    fn ranked_sorts_descending() {
        let tone = ChoiceAnswer {
            choice: "angry".into(),
            probabilities: [("calm", 0.1), ("angry", 0.8), ("excited", 0.1)]
                .into_iter()
                .map(|(key, value)| (key.to_owned(), value))
                .collect(),
            confidence: 0.9,
        };
        let ranked = tone.ranked();
        assert_eq!(ranked[0], ("angry", 0.8));
        // Ties are broken alphabetically.
        assert_eq!(ranked[1].0, "calm");
        assert_eq!(ranked[2].0, "excited");
    }

    #[test]
    fn unknown_answer_type_is_kept() {
        let body = r#"{
            "model": "jev-1.13.0",
            "answers": {
                "spam": {"type": "noul", "noul": 0.98},
                "future": {"type": "vibes", "vibe": "immaculate", "confidence": 0.5}
            },
            "usage": {"input_tokens": 1, "output_tokens": 2}
        }"#;
        let response = SystemOneResponse::decode(&raw(body)).unwrap();
        match response.answers.get("future").unwrap() {
            Answer::Unknown { kind, raw } => {
                assert_eq!(kind, "vibes");
                assert_eq!(raw["vibe"], "immaculate");
            }
            other => panic!("expected Unknown, got {other:?}"),
        }
        // Typed accessors skip it.
        assert!(response.noul("future").is_none());
        assert_eq!(response.nouls().count(), 1);
    }

    #[test]
    fn missing_usage_defaults_to_zero() {
        let body = r#"{"model": "jev-1.13.0", "answers": {"a": {"type": "noul", "noul": 1.0}}}"#;
        let response = SystemOneResponse::decode(&raw(body)).unwrap();
        assert_eq!(response.usage, Usage::default());
    }

    #[test]
    fn validation_error_field_paths() {
        // Missing confidence on a choice answer.
        let body = r#"{"model": "m", "answers": {"tone": {"type": "choice", "choice": "calm", "probabilities": {"calm": 1.0}}}, "usage": {}}"#;
        let error = SystemOneResponse::decode(&raw(body)).unwrap_err();
        match error {
            Error::ResponseValidation {
                field_path, status, ..
            } => {
                assert_eq!(field_path, "answers.tone.confidence");
                assert_eq!(status, 200);
            }
            other => panic!("expected ResponseValidation, got {other:?}"),
        }
        // Non-object body.
        let error = SystemOneResponse::decode(&raw("nope")).unwrap_err();
        match error {
            Error::ResponseValidation { field_path, .. } => {
                assert_eq!(field_path, "");
            }
            other => panic!("expected ResponseValidation, got {other:?}"),
        }
        // Missing model.
        let error = SystemOneResponse::decode(&raw(r#"{"answers": {}}"#)).unwrap_err();
        match error {
            Error::ResponseValidation { field_path, .. } => {
                assert_eq!(field_path, "model");
            }
            other => panic!("expected ResponseValidation, got {other:?}"),
        }
        // Missing answers.
        let error = SystemOneResponse::decode(&raw(r#"{"model": "m"}"#)).unwrap_err();
        match error {
            Error::ResponseValidation { field_path, .. } => {
                assert_eq!(field_path, "answers");
            }
            other => panic!("expected ResponseValidation, got {other:?}"),
        }
        // Bad answer type field.
        let body = r#"{"model": "m", "answers": {"a": {"noul": 1.0}}}"#;
        let error = SystemOneResponse::decode(&raw(body)).unwrap_err();
        match error {
            Error::ResponseValidation { field_path, .. } => {
                assert_eq!(field_path, "answers.a.type");
            }
            other => panic!("expected ResponseValidation, got {other:?}"),
        }
        // Non-numeric noul.
        let body = r#"{"model": "m", "answers": {"a": {"type": "noul", "noul": "yes"}}}"#;
        let error = SystemOneResponse::decode(&raw(body)).unwrap_err();
        match error {
            Error::ResponseValidation { field_path, .. } => {
                assert_eq!(field_path, "answers.a.noul");
            }
            other => panic!("expected ResponseValidation, got {other:?}"),
        }
    }

    #[test]
    fn decodes_models() {
        let body = r#"{"models": [
            {"name": "jev-latest", "description": "General-purpose system one model.", "release_date": "2026-09-15"}
        ]}"#;
        let models = decode_models(&raw(body)).unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].name, "jev-latest");
        assert_eq!(models[0].release_date, "2026-09-15");
        let error = decode_models(&raw(r#"{}"#)).unwrap_err();
        match error {
            Error::ResponseValidation { field_path, .. } => {
                assert_eq!(field_path, "models");
            }
            other => panic!("expected ResponseValidation, got {other:?}"),
        }
    }
}
