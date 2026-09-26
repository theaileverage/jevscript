//! The Jev client.
//!
//! Jev reads one structured state and answers typed questions with
//! probabilities. It does not generate text and it does not count (spec
//! sections 1 and 6). One request carries one state and every question batched
//! into it; the runtime is charged one call per request, not per question.
//!
//! The types here are the runtime's own shape: a [`Question`] knows its state
//! path and its detail block, and a [`JevAnswer`] is keyed by the question id.
//! The [`wire`] module maps them onto the TypeSafe API's request and response
//! bodies, which are documented at <https://docs.typesafe.ai/api.md>.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use thiserror::Error;

use crate::error::{RuntimeError, RuntimeErrorCode};
use crate::profile::Profile;
use crate::tokens::{CharsPerToken, TokenEstimator, estimator_for};

/// Where Jev lives.
pub const JEV_ENDPOINT: &str = "https://api.typesafe.ai/v1/systemone";

/// The environment variable the bearer token is read from.
pub const JEV_API_KEY_ENV: &str = "TYPESAFE_API_KEY";

/// How long one request may take before it counts as `jev_unavailable`.
/// Jev answers in well under a second; a hung connection should not hang a
/// run.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

/// The detail keys, which map directly onto Jev's instruction object (spec 6.8).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Instruction {
    /// What to look at within the subject.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus: Option<String>,
    /// A supporting fact the question needs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Other state paths to read alongside the subject.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub compare: Vec<String>,
    /// Short concrete examples of the positive side.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub yes: Vec<String>,
    /// Short concrete examples of the negative side.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub no: Vec<String>,
}

impl Instruction {
    /// Whether the program wrote any detail at all.
    pub fn is_empty(&self) -> bool {
        self == &Instruction::default()
    }
}

/// One label of a Choice question.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChoiceLabel {
    /// The label name, which is what comes back in the answer.
    pub name: String,
    /// Its description. Absent on the bare escape option.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// What the label covers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub what: Option<String>,
    /// What the label does not cover.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_for: Option<String>,
    /// Short concrete instances of the label.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub examples: Vec<String>,
}

impl ChoiceLabel {
    /// A label with only a one-line description.
    pub fn described(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: Some(description.into()),
            what: None,
            not_for: None,
            examples: Vec::new(),
        }
    }

    /// The bare `other` / `none` escape option (spec section 6.3).
    pub fn escape(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: None,
            what: None,
            not_for: None,
            examples: Vec::new(),
        }
    }
}

/// One level of a Score question.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScoreLevel {
    /// The level's name, if the program named it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The situation it describes. Situations stand alone (spec section 6.4).
    pub situation: String,
}

/// A question. `feels` is a Noul, `pick` a Choice, `rate` a Score.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Question {
    /// A yes/no question.
    Noul {
        /// The id the answer is keyed by.
        id: String,
        /// The state path to inspect, such as `obs.summary` or `files[3]`.
        path: String,
        /// The condition, phrased as one crisp property.
        condition: String,
        /// The detail block, if the program wrote one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        instruction: Option<Instruction>,
    },
    /// A choice among labelled descriptions.
    Choice {
        /// The id the answer is keyed by.
        id: String,
        /// The state path to inspect.
        path: String,
        /// What Jev is choosing for. A `pick among` writes one (spec 6.4a); a
        /// `pick` has none, since its labels' descriptions are the question.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        question: Option<String>,
        /// Two to eight labels, exactly one of them the escape option.
        labels: Vec<ChoiceLabel>,
        /// The detail block, if the program wrote one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        instruction: Option<Instruction>,
    },
    /// A place on a spectrum of situations.
    Score {
        /// The id the answer is keyed by.
        id: String,
        /// The state path to inspect.
        path: String,
        /// Two to ten situations, low to high.
        levels: Vec<ScoreLevel>,
        /// The detail block, if the program wrote one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        instruction: Option<Instruction>,
    },
}

impl Question {
    /// The id this question's answer is keyed by.
    pub fn id(&self) -> &str {
        match self {
            Question::Noul { id, .. }
            | Question::Choice { id, .. }
            | Question::Score { id, .. } => id,
        }
    }

    /// The state path this question inspects.
    pub fn path(&self) -> &str {
        match self {
            Question::Noul { path, .. }
            | Question::Choice { path, .. }
            | Question::Score { path, .. } => path,
        }
    }

    /// The detail block, if any.
    pub fn instruction(&self) -> Option<&Instruction> {
        match self {
            Question::Noul { instruction, .. }
            | Question::Choice { instruction, .. }
            | Question::Score { instruction, .. } => instruction.as_ref(),
        }
    }
}

/// One request: one state, many questions (spec section 6.9).
///
/// The state is built from the subjects of every question in the request, each
/// placed at its path, and nothing else is sent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JevRequest {
    /// The state, keyed by path root.
    pub state: BTreeMap<String, serde_json::Value>,
    /// Which Jev model to ask.
    pub model: String,
    /// Every question in this request.
    pub questions: Vec<Question>,
}

/// One answer, keyed back to its question by `id`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum JevAnswer {
    /// The probability that a `feels` condition holds.
    Noul {
        /// Which question.
        id: String,
        /// The probability, in `[0, 1]`.
        prob: f64,
    },
    /// The chosen label of a `pick`.
    Choice {
        /// Which question.
        id: String,
        /// The chosen label.
        label: String,
        /// How peaked the distribution is.
        confidence: f64,
        /// Label to probability.
        probabilities: BTreeMap<String, f64>,
    },
    /// The placement of a `rate`.
    Score {
        /// Which question.
        id: String,
        /// The nearest whole level index.
        level: u32,
        /// The probability-weighted mean.
        score: f64,
        /// How peaked the distribution is.
        confidence: f64,
        /// Level name, or index as text, to probability.
        probabilities: BTreeMap<String, f64>,
    },
}

impl JevAnswer {
    /// The id of the question this answers.
    pub fn id(&self) -> &str {
        match self {
            JevAnswer::Noul { id, .. }
            | JevAnswer::Choice { id, .. }
            | JevAnswer::Score { id, .. } => id,
        }
    }
}

/// What one request cost.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct JevUsage {
    /// Tokens in the state plus the questions.
    pub tokens: u64,
    /// Estimated spend, if the runtime's price table covers the model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usd: Option<f64>,
}

/// One response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JevResponse {
    /// One answer per question, in any order.
    pub answers: Vec<JevAnswer>,
    /// What the request cost.
    #[serde(default)]
    pub usage: JevUsage,
    /// How long it took.
    #[serde(default)]
    pub latency_ms: u64,
}

/// What can go wrong talking to Jev.
#[derive(Debug, Error)]
pub enum JevError {
    /// Network failure or 5xx. Retryable.
    #[error("jev is unavailable: {0}")]
    Unavailable(String),
    /// 4xx, with Jev's message. Not retryable.
    #[error("jev rejected the request: {0}")]
    Rejected(String),
}

impl From<JevError> for RuntimeError {
    fn from(error: JevError) -> Self {
        match error {
            JevError::Unavailable(message) => {
                RuntimeError::new(RuntimeErrorCode::JevUnavailable, message)
            }
            JevError::Rejected(message) => {
                RuntimeError::new(RuntimeErrorCode::JevRejected, message)
            }
        }
    }
}

/// The runtime asks Jev only through this trait, so replay, tests and a
/// recorded fixture can all stand in for the real service.
///
/// The trait is synchronous on purpose: the interpreter is stepped by the host
/// and never owns an async runtime (see [`crate::run`]). The HTTP
/// implementation below is the only place that touches tokio.
pub trait JevClient: Send + Sync {
    /// Send one request and wait for its answers.
    ///
    /// # Errors
    ///
    /// [`JevError::Unavailable`] for a network failure or a 5xx,
    /// [`JevError::Rejected`] for a 4xx.
    fn send(&self, request: &JevRequest) -> Result<JevResponse, JevError>;
}

/// The mapping between the runtime's question and answer types and the
/// TypeSafe API's JSON.
///
/// Sources, read on 2026-09-21:
///
/// - <https://docs.typesafe.ai/api.md>: `POST /v1/systemone` takes
///   `{state, model, questions: {id: Question}}` and answers with
///   `{model, answers: {id: Answer}, usage: {input_tokens, output_tokens}}`.
///   A question is `{type: "noul" | "choice" | "score", instructions,
///   criteria}`; a Noul answer is `{noul}`, a Choice answer `{choice,
///   probabilities, confidence}`, a Score answer `{score, legend,
///   probabilities, confidence}` with levels keyed by index as text.
/// - <https://docs.typesafe.ai/primitives/advanced.md>: `instructions` may be
///   an object with `question`, `inspect`, `focus`, `note` and `compare`;
///   a Choice criterion may be `{what, not_for, examples}`; a Noul criterion
///   has `true` and `false` sides, each `{what, examples}`.
/// - <https://docs.typesafe.ai/confidence.md>: `confidence` is derived from
///   the probabilities and returned on every Choice and Score answer.
pub mod wire {
    use super::{ChoiceLabel, Instruction, JevAnswer, JevRequest, Question, ScoreLevel};
    use serde_json::{Map, Value as Json, json};
    use std::collections::BTreeMap;

    /// The description the bare `other` / `none` label is sent with, so that
    /// Jev knows it is the escape option (spec section 6.3).
    pub const ESCAPE_DESCRIPTION: &str = "none of the other options fits";

    /// The request body for `POST /v1/systemone`.
    pub fn request_body(request: &JevRequest) -> Json {
        let questions: Map<String, Json> = request
            .questions
            .iter()
            .map(|q| (q.id().to_string(), question(q)))
            .collect();
        json!({
            "state": request.state,
            "model": request.model,
            "questions": questions,
        })
    }

    /// One question object.
    pub fn question(question: &Question) -> Json {
        match question {
            Question::Noul {
                path,
                condition,
                instruction,
                ..
            } => {
                let mut body = json!({
                    "type": "noul",
                    "instructions": instructions(
                        format!("Is it the case that `{path}` {condition}?"),
                        path,
                        instruction.as_ref(),
                    ),
                });
                let detail = instruction.as_ref();
                let yes = detail.map(|d| d.yes.as_slice()).unwrap_or_default();
                let no = detail.map(|d| d.no.as_slice()).unwrap_or_default();
                if !yes.is_empty() || !no.is_empty() {
                    let mut criteria = Map::new();
                    if !yes.is_empty() {
                        criteria.insert("true".into(), json!({ "examples": yes }));
                    }
                    if !no.is_empty() {
                        criteria.insert("false".into(), json!({ "examples": no }));
                    }
                    body["criteria"] = Json::Object(criteria);
                }
                body
            }
            Question::Choice {
                path,
                question,
                labels,
                instruction,
                ..
            } => {
                let text = question
                    .clone()
                    .unwrap_or_else(|| format!("Which option describes `{path}`?"));
                let criteria: Map<String, Json> = labels
                    .iter()
                    .map(|label| (label.name.clone(), criterion(label)))
                    .collect();
                json!({
                    "type": "choice",
                    "instructions": instructions(text, path, instruction.as_ref()),
                    "criteria": criteria,
                })
            }
            Question::Score {
                path,
                levels,
                instruction,
                ..
            } => json!({
                "type": "score",
                "instructions": instructions(
                    format!("Which level describes `{path}`?"),
                    path,
                    instruction.as_ref(),
                ),
                "criteria": levels.iter().map(|l: &ScoreLevel| Json::String(l.situation.clone())).collect::<Vec<_>>(),
            }),
        }
    }

    /// The structured `instructions` object: the question, the path to
    /// inspect, and the detail keys that apply to every verb (spec 6.8).
    fn instructions(question: String, path: &str, detail: Option<&Instruction>) -> Json {
        let mut map = Map::new();
        map.insert("question".into(), Json::String(question));
        map.insert("inspect".into(), Json::String(path.to_string()));
        if let Some(detail) = detail {
            if let Some(focus) = &detail.focus {
                map.insert("focus".into(), Json::String(focus.clone()));
            }
            if let Some(note) = &detail.note {
                map.insert("note".into(), Json::String(note.clone()));
            }
            if !detail.compare.is_empty() {
                map.insert("compare".into(), json!(detail.compare));
            }
        }
        Json::Object(map)
    }

    /// One Choice criterion: a plain description, or the contrastive
    /// `{what, not_for, examples}` object.
    fn criterion(label: &ChoiceLabel) -> Json {
        if label.what.is_none() && label.not_for.is_none() && label.examples.is_empty() {
            return Json::String(
                label
                    .description
                    .clone()
                    .unwrap_or_else(|| ESCAPE_DESCRIPTION.to_string()),
            );
        }
        let mut map = Map::new();
        if let Some(what) = label.what.clone().or_else(|| label.description.clone()) {
            map.insert("what".into(), Json::String(what));
        }
        if let Some(not_for) = &label.not_for {
            map.insert("not_for".into(), Json::String(not_for.clone()));
        }
        if !label.examples.is_empty() {
            map.insert("examples".into(), json!(label.examples));
        }
        Json::Object(map)
    }

    /// The answers of a response body, keyed back to our question ids, and
    /// the token usage if the body reports one.
    ///
    /// Score probabilities come back keyed by level index; when the program
    /// named its levels they are re-keyed by name so `l is unchanged` and
    /// `l.probabilities.unchanged` work (spec section 6.4).
    ///
    /// # Errors
    ///
    /// A message naming what was missing or malformed.
    pub fn parse_response(
        body: &Json,
        request: &JevRequest,
    ) -> Result<(Vec<JevAnswer>, Option<u64>), String> {
        let answers = body
            .get("answers")
            .and_then(Json::as_object)
            .ok_or("the response has no `answers` object")?;
        let mut out = Vec::with_capacity(request.questions.len());
        for question in &request.questions {
            let id = question.id();
            let answer = answers
                .get(id)
                .ok_or_else(|| format!("the response has no answer for `{id}`"))?;
            out.push(parse_answer(id, question, answer)?);
        }
        let tokens = body.get("usage").and_then(|usage| {
            let input = usage.get("input_tokens").and_then(Json::as_u64);
            let output = usage.get("output_tokens").and_then(Json::as_u64);
            match (input, output) {
                (None, None) => None,
                (input, output) => Some(input.unwrap_or(0) + output.unwrap_or(0)),
            }
        });
        Ok((out, tokens))
    }

    fn parse_answer(id: &str, question: &Question, answer: &Json) -> Result<JevAnswer, String> {
        let number = |key: &str| {
            answer
                .get(key)
                .and_then(Json::as_f64)
                .ok_or_else(|| format!("answer `{id}` has no numeric `{key}`"))
        };
        let probabilities = || -> Result<BTreeMap<String, f64>, String> {
            answer
                .get("probabilities")
                .and_then(Json::as_object)
                .ok_or_else(|| format!("answer `{id}` has no `probabilities`"))?
                .iter()
                .map(|(k, v)| {
                    v.as_f64()
                        .map(|p| (k.clone(), p))
                        .ok_or_else(|| format!("answer `{id}` probability `{k}` is not a number"))
                })
                .collect()
        };
        match question {
            Question::Noul { .. } => Ok(JevAnswer::Noul {
                id: id.to_string(),
                prob: number("noul")?,
            }),
            Question::Choice { .. } => {
                let probabilities = probabilities()?;
                Ok(JevAnswer::Choice {
                    id: id.to_string(),
                    label: answer
                        .get("choice")
                        .and_then(Json::as_str)
                        .ok_or_else(|| format!("answer `{id}` has no `choice`"))?
                        .to_string(),
                    confidence: number("confidence")
                        .unwrap_or_else(|_| confidence_of(&probabilities)),
                    probabilities,
                })
            }
            Question::Score { levels, .. } => {
                let score = number("score")?;
                let names: Vec<Option<&str>> = levels.iter().map(|l| l.name.as_deref()).collect();
                let named = !names.is_empty() && names.iter().all(Option::is_some);
                let probabilities: BTreeMap<String, f64> = probabilities()?
                    .into_iter()
                    .map(|(key, p)| {
                        let renamed = if named {
                            key.parse::<usize>()
                                .ok()
                                .and_then(|i| names.get(i).copied().flatten())
                                .map(str::to_string)
                        } else {
                            None
                        };
                        (renamed.unwrap_or(key), p)
                    })
                    .collect();
                Ok(JevAnswer::Score {
                    id: id.to_string(),
                    level: nearest_level(score, levels.len()),
                    score,
                    confidence: number("confidence")
                        .unwrap_or_else(|_| confidence_of(&probabilities)),
                    probabilities,
                })
            }
        }
    }

    /// The nearest whole level index to a score (spec section 6.4).
    pub fn nearest_level(score: f64, levels: usize) -> u32 {
        let top = levels.saturating_sub(1) as f64;
        score.round().clamp(0.0, top) as u32
    }

    /// How peaked a distribution is, on the scale the API uses when it
    /// reports `confidence`: `(n * peak - 1) / (n - 1)`, which is 1 when all
    /// the mass is on one option and 0 when it is spread evenly
    /// (<https://docs.typesafe.ai/confidence.md>). Only used when a response
    /// omits its own `confidence`.
    pub fn confidence_of(probabilities: &BTreeMap<String, f64>) -> f64 {
        let n = probabilities.len();
        if n < 2 {
            return if n == 1 { 1.0 } else { 0.0 };
        }
        let peak = probabilities.values().copied().fold(0.0, f64::max);
        ((n as f64 * peak - 1.0) / (n as f64 - 1.0)).clamp(0.0, 1.0)
    }

    /// The message of an error body, whichever of the usual keys it uses,
    /// else the raw body.
    pub fn error_message(status: u16, body: &str) -> String {
        let message = serde_json::from_str::<Json>(body).ok().and_then(|json| {
            ["message", "detail", "error"]
                .iter()
                .find_map(|key| match json.get(key) {
                    Some(Json::String(text)) => Some(text.clone()),
                    Some(Json::Object(inner)) => inner
                        .get("message")
                        .and_then(Json::as_str)
                        .map(str::to_string),
                    Some(other) if !other.is_null() => Some(other.to_string()),
                    _ => None,
                })
        });
        format!(
            "HTTP {status}: {}",
            message.unwrap_or_else(|| body.trim().to_string())
        )
    }
}

/// The real client: POSTs the [`wire`] form of a request to the endpoint with
/// `Authorization: Bearer $TYPESAFE_API_KEY`.
///
/// This type owns the crate's only tokio runtime and blocks on it, so that
/// everything above it stays synchronous, pausable and replayable.
pub struct HttpJevClient {
    endpoint: String,
    api_key: String,
    model: String,
    estimator: Box<dyn TokenEstimator>,
    price_per_million_input_usd: Option<f64>,
    http: reqwest::Client,
    runtime: tokio::runtime::Runtime,
}

impl HttpJevClient {
    /// A client for [`JEV_ENDPOINT`], reading the token from
    /// [`JEV_API_KEY_ENV`].
    ///
    /// # Errors
    ///
    /// Fails if the token is missing or the HTTP client or tokio runtime cannot
    /// be built.
    pub fn from_env(model: impl Into<String>) -> Result<Self, RuntimeError> {
        let api_key = api_key_from_env()?;
        Self::new(JEV_ENDPOINT, api_key, model)
    }

    /// A client for a profile: its endpoint, model, estimator and price, with
    /// the token from [`JEV_API_KEY_ENV`] (spec section 10.6).
    ///
    /// # Errors
    ///
    /// As [`HttpJevClient::from_env`].
    pub fn for_profile(profile: &Profile) -> Result<Self, RuntimeError> {
        let api_key = api_key_from_env()?;
        Self::for_profile_with_key(profile, api_key)
    }

    /// A client for a profile with an explicit token.
    ///
    /// # Errors
    ///
    /// Fails if the HTTP client or the tokio runtime cannot be built.
    pub fn for_profile_with_key(
        profile: &Profile,
        api_key: impl Into<String>,
    ) -> Result<Self, RuntimeError> {
        let mut client = Self::new(profile.endpoint.clone(), api_key, profile.model.clone())?;
        client.estimator = estimator_for(&profile.tokenizer);
        client.price_per_million_input_usd = Some(profile.price_per_million_input_usd);
        Ok(client)
    }

    /// A client for an explicit endpoint and token. Token usage a response
    /// does not report is estimated with `chars4`, and no price is known;
    /// [`HttpJevClient::for_profile`] fills both from the profile.
    ///
    /// # Errors
    ///
    /// Fails if the HTTP client or the tokio runtime cannot be built.
    pub fn new(
        endpoint: impl Into<String>,
        api_key: impl Into<String>,
        model: impl Into<String>,
    ) -> Result<Self, RuntimeError> {
        let http = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|error| {
                RuntimeError::new(RuntimeErrorCode::JevUnavailable, error.to_string())
            })?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| {
                RuntimeError::new(RuntimeErrorCode::JevUnavailable, error.to_string())
            })?;
        Ok(Self {
            endpoint: endpoint.into(),
            api_key: api_key.into(),
            model: model.into(),
            estimator: Box::new(CharsPerToken),
            price_per_million_input_usd: None,
            http,
            runtime,
        })
    }

    /// The model this client asks by default.
    pub fn model(&self) -> &str {
        &self.model
    }

    /// The endpoint this client posts to.
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// The usage of one request: the response's own token count when it
    /// reports one, else the estimator's, priced when a profile was given.
    fn usage(&self, body: &serde_json::Value, reported: Option<u64>) -> JevUsage {
        let tokens = reported.unwrap_or_else(|| self.estimator.estimate(&body.to_string()));
        JevUsage {
            tokens,
            usd: self
                .price_per_million_input_usd
                .map(|price| tokens as f64 / 1_000_000.0 * price),
        }
    }
}

fn api_key_from_env() -> Result<String, RuntimeError> {
    std::env::var(JEV_API_KEY_ENV).map_err(|_| {
        RuntimeError::new(
            RuntimeErrorCode::JevRejected,
            format!("{JEV_API_KEY_ENV} is not set"),
        )
    })
}

impl JevClient for HttpJevClient {
    fn send(&self, request: &JevRequest) -> Result<JevResponse, JevError> {
        let body = wire::request_body(request);
        let started = Instant::now();
        let (status, text) = self.runtime.block_on(async {
            let response = self
                .http
                .post(&self.endpoint)
                .bearer_auth(&self.api_key)
                .json(&body)
                .send()
                .await
                .map_err(|error| JevError::Unavailable(error.to_string()))?;
            let status = response.status().as_u16();
            let text = response
                .text()
                .await
                .map_err(|error| JevError::Unavailable(error.to_string()))?;
            Ok::<_, JevError>((status, text))
        })?;
        let latency_ms = started.elapsed().as_millis() as u64;
        // Spec section 12: a network failure, a 5xx or a 429 rate limit is
        // `jev_unavailable` and retryable; any other 4xx is `jev_rejected`
        // with Jev's message.
        if status >= 500 || status == 429 {
            return Err(JevError::Unavailable(wire::error_message(status, &text)));
        }
        if status >= 400 {
            return Err(JevError::Rejected(wire::error_message(status, &text)));
        }
        let json: serde_json::Value = serde_json::from_str(&text).map_err(|error| {
            JevError::Unavailable(format!("HTTP {status}: the response is not JSON: {error}"))
        })?;
        let (answers, reported) = wire::parse_response(&json, request)
            .map_err(|message| JevError::Unavailable(format!("HTTP {status}: {message}")))?;
        Ok(JevResponse {
            answers,
            usage: self.usage(&body, reported),
            latency_ms,
        })
    }
}

/// What a [`ScriptedJevClient`] answers with.
type Script = Box<dyn Fn(&JevRequest) -> Result<JevResponse, JevError> + Send + Sync>;

/// A test double: answers from a list of canned responses in order, or from a
/// closure, and remembers every request it saw so a test can assert on the
/// state and the batching that reached Jev (spec sections 6.6 and 6.9).
pub struct ScriptedJevClient {
    responses: Mutex<std::collections::VecDeque<JevResponse>>,
    script: Option<Script>,
    requests: Mutex<Vec<JevRequest>>,
}

impl ScriptedJevClient {
    /// A client that answers with `responses` in order and is unavailable
    /// once they run out.
    pub fn new(responses: Vec<JevResponse>) -> Self {
        Self {
            responses: Mutex::new(responses.into()),
            script: None,
            requests: Mutex::new(Vec::new()),
        }
    }

    /// A client that computes each response from the request.
    pub fn from_fn(
        script: impl Fn(&JevRequest) -> Result<JevResponse, JevError> + Send + Sync + 'static,
    ) -> Self {
        Self {
            responses: Mutex::new(std::collections::VecDeque::new()),
            script: Some(Box::new(script)),
            requests: Mutex::new(Vec::new()),
        }
    }

    /// Every request sent so far, in order.
    pub fn requests(&self) -> Vec<JevRequest> {
        self.requests.lock().expect("not poisoned").clone()
    }

    /// How many requests were sent.
    pub fn request_count(&self) -> usize {
        self.requests.lock().expect("not poisoned").len()
    }
}

impl JevClient for ScriptedJevClient {
    fn send(&self, request: &JevRequest) -> Result<JevResponse, JevError> {
        self.requests
            .lock()
            .expect("not poisoned")
            .push(request.clone());
        if let Some(script) = &self.script {
            return script(request);
        }
        self.responses
            .lock()
            .expect("not poisoned")
            .pop_front()
            .ok_or_else(|| {
                JevError::Unavailable("the scripted client has no more responses".into())
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::Arc;

    fn triage_request() -> JevRequest {
        JevRequest {
            state: BTreeMap::from([(
                "message".to_string(),
                json!(
                    "URGENT: prod is down, the deploy at 3pm broke checkout. Please fix the null pointer in cart.rs today."
                ),
            )]),
            model: "jev-latest".into(),
            questions: vec![
                Question::Noul {
                    id: "urgent".into(),
                    path: "message".into(),
                    condition: "needs a response within the hour".into(),
                    instruction: None,
                },
                Question::Choice {
                    id: "owner".into(),
                    path: "message".into(),
                    question: None,
                    labels: vec![
                        ChoiceLabel::described("code", "asks for a code change or reports a bug"),
                        ChoiceLabel::described("customer", "a customer asking for help or a reply"),
                        ChoiceLabel::described("schedule", "asks to find or move a meeting time"),
                        ChoiceLabel::escape("other"),
                    ],
                    instruction: None,
                },
                Question::Noul {
                    id: "risky".into(),
                    path: "message".into(),
                    condition: "acting on it could send, pay, delete or commit something".into(),
                    instruction: Some(Instruction {
                        yes: vec!["Please wire the deposit today.".into()],
                        no: vec!["Just letting you know.".into()],
                        ..Instruction::default()
                    }),
                },
                Question::Score {
                    id: "effort".into(),
                    path: "message".into(),
                    levels: vec![
                        ScoreLevel {
                            name: Some("trivial".into()),
                            situation: "answerable in one line".into(),
                        },
                        ScoreLevel {
                            name: Some("hour".into()),
                            situation: "an hour of focused work".into(),
                        },
                        ScoreLevel {
                            name: Some("project".into()),
                            situation: "a multi-day project".into(),
                        },
                    ],
                    instruction: None,
                },
            ],
        }
    }

    fn canned_body() -> serde_json::Value {
        json!({
            "model": "jev-1.13.0",
            "answers": {
                "urgent": {"type": "noul", "noul": 0.93},
                "owner": {"type": "choice", "choice": "code", "probabilities": {"code": 0.8, "customer": 0.1, "schedule": 0.05, "other": 0.05}, "confidence": 0.73},
                "risky": {"type": "noul", "noul": 0.4},
                "effort": {"type": "score", "score": 1.3, "legend": {"0": "a", "1": "b", "2": "c"}, "probabilities": {"0": 0.1, "1": 0.5, "2": 0.4}, "confidence": 0.3}
            },
            "usage": {"input_tokens": 300, "output_tokens": 20}
        })
    }

    /// A one-shot HTTP/1.1 server on a thread. `Some(status, body)` answers;
    /// `None` accepts and drops the connection. Returns the URL and the
    /// request body it received.
    fn fake_server(reply: Option<(u16, &'static str)>) -> (String, Arc<Mutex<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
        let url = format!(
            "http://{}/v1/systemone",
            listener.local_addr().expect("addr")
        );
        let received = Arc::new(Mutex::new(String::new()));
        let seen = Arc::clone(&received);
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accepts");
            let Some((status, body)) = reply else {
                drop(stream);
                return;
            };
            let mut buf = Vec::new();
            let mut chunk = [0u8; 4096];
            let (header_end, content_length) = loop {
                let n = stream.read(&mut chunk).expect("reads");
                if n == 0 {
                    break (buf.len(), 0);
                }
                buf.extend_from_slice(&chunk[..n]);
                if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&buf[..pos]).to_string();
                    let length = head
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                        })
                        .unwrap_or(0);
                    break (pos + 4, length);
                }
            };
            while buf.len() < header_end + content_length {
                let n = stream.read(&mut chunk).expect("reads");
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
            }
            *seen.lock().expect("lock") = String::from_utf8_lossy(&buf[header_end..]).to_string();
            let reason = match status {
                200 => "OK",
                400 => "Bad Request",
                503 => "Service Unavailable",
                _ => "Whatever",
            };
            let response = format!(
                "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).expect("writes");
            stream.flush().expect("flushes");
        });
        (url, received)
    }

    #[test]
    fn the_wire_form_matches_the_api_reference() {
        // docs.typesafe.ai/api.md: {state, model, questions: {id: {type,
        // instructions, criteria}}}.
        let body = wire::request_body(&triage_request());
        assert_eq!(body["model"], "jev-latest");
        assert!(body["state"]["message"].is_string());
        let questions = body["questions"].as_object().expect("map");
        assert_eq!(questions.len(), 4);
        assert_eq!(questions["urgent"]["type"], "noul");
        assert_eq!(questions["urgent"]["instructions"]["inspect"], "message");
        assert!(
            questions["urgent"]["instructions"]["question"]
                .as_str()
                .expect("text")
                .contains("`message`")
        );
        assert!(questions["urgent"].get("criteria").is_none());
        assert_eq!(
            questions["risky"]["criteria"]["true"]["examples"][0],
            "Please wire the deposit today."
        );
        assert_eq!(
            questions["risky"]["criteria"]["false"]["examples"][0],
            "Just letting you know."
        );
        assert_eq!(questions["owner"]["type"], "choice");
        assert_eq!(
            questions["owner"]["criteria"]["code"],
            "asks for a code change or reports a bug"
        );
        assert_eq!(
            questions["owner"]["criteria"]["other"],
            wire::ESCAPE_DESCRIPTION
        );
        assert_eq!(questions["effort"]["type"], "score");
        assert_eq!(
            questions["effort"]["criteria"],
            json!([
                "answerable in one line",
                "an hour of focused work",
                "a multi-day project"
            ])
        );
    }

    #[test]
    fn detail_keys_and_contrastive_labels_reach_the_instruction_object() {
        // Spec 6.8: focus/note/compare on any verb; what/not_for/examples on a
        // pick label (docs.typesafe.ai/primitives/advanced.md).
        let question = Question::Choice {
            id: "dept".into(),
            path: "ticket".into(),
            question: None,
            labels: vec![
                ChoiceLabel {
                    name: "billing".into(),
                    description: None,
                    what: Some("charges".into()),
                    not_for: Some("tracking".into()),
                    examples: vec!["I was charged twice".into()],
                },
                ChoiceLabel::escape("other"),
            ],
            instruction: Some(Instruction {
                focus: Some("the primary request".into()),
                note: Some("customers write informally".into()),
                compare: vec!["ticket.history".into()],
                ..Instruction::default()
            }),
        };
        let json = wire::question(&question);
        assert_eq!(json["instructions"]["focus"], "the primary request");
        assert_eq!(json["instructions"]["note"], "customers write informally");
        assert_eq!(json["instructions"]["compare"], json!(["ticket.history"]));
        assert_eq!(
            json["criteria"]["billing"],
            json!({"what": "charges", "not_for": "tracking", "examples": ["I was charged twice"]})
        );
    }

    #[test]
    fn a_pick_among_question_is_the_instruction() {
        let question = Question::Choice {
            id: "which".into(),
            path: "prs".into(),
            question: Some("Which pull request is safest to merge first?".into()),
            labels: vec![
                ChoiceLabel::described("i0", "Fix typo"),
                ChoiceLabel::escape("none"),
            ],
            instruction: None,
        };
        let json = wire::question(&question);
        assert_eq!(
            json["instructions"]["question"],
            "Which pull request is safest to merge first?"
        );
    }

    #[test]
    fn answers_parse_into_our_shape_with_named_levels() {
        // Spec 6.3 and 6.4: probabilities keyed by label, level as the nearest
        // whole index, level probabilities re-keyed by name.
        let (answers, tokens) =
            wire::parse_response(&canned_body(), &triage_request()).expect("parses");
        assert_eq!(tokens, Some(320));
        assert_eq!(answers.len(), 4);
        assert_eq!(
            answers[0],
            JevAnswer::Noul {
                id: "urgent".into(),
                prob: 0.93
            }
        );
        let JevAnswer::Choice {
            label,
            confidence,
            probabilities,
            ..
        } = &answers[1]
        else {
            panic!()
        };
        assert_eq!(label, "code");
        assert_eq!(*confidence, 0.73);
        assert_eq!(probabilities["other"], 0.05);
        let JevAnswer::Score {
            level,
            score,
            probabilities,
            ..
        } = &answers[3]
        else {
            panic!()
        };
        assert_eq!(*level, 1);
        assert_eq!(*score, 1.3);
        assert_eq!(probabilities["hour"], 0.5);
        assert!(!probabilities.contains_key("1"));
    }

    #[test]
    fn a_missing_answer_is_an_error() {
        let mut body = canned_body();
        body["answers"]
            .as_object_mut()
            .expect("map")
            .remove("risky");
        let error = wire::parse_response(&body, &triage_request()).expect_err("missing");
        assert!(error.contains("risky"));
    }

    #[test]
    fn nearest_level_and_fallback_confidence() {
        assert_eq!(wire::nearest_level(1.3, 3), 1);
        assert_eq!(wire::nearest_level(1.5, 3), 2);
        assert_eq!(wire::nearest_level(9.0, 3), 2);
        assert_eq!(wire::nearest_level(-1.0, 3), 0);
        let peaked = BTreeMap::from([("a".to_string(), 1.0), ("b".to_string(), 0.0)]);
        assert_eq!(wire::confidence_of(&peaked), 1.0);
        let flat = BTreeMap::from([("a".to_string(), 0.5), ("b".to_string(), 0.5)]);
        assert_eq!(wire::confidence_of(&flat), 0.0);
    }

    #[test]
    fn a_200_is_parsed_and_usage_comes_from_the_response() {
        let body = canned_body().to_string();
        let (url, received) = fake_server(Some((200, Box::leak(body.into_boxed_str()))));
        let client = HttpJevClient::new(url, "test-key", "jev-latest").expect("client");
        let response = client.send(&triage_request()).expect("200");
        assert_eq!(response.answers.len(), 4);
        assert_eq!(response.usage.tokens, 320);
        assert_eq!(response.usage.usd, None);
        let sent: serde_json::Value =
            serde_json::from_str(&received.lock().expect("lock")).expect("json body");
        assert_eq!(sent["questions"]["owner"]["type"], "choice");
    }

    #[test]
    fn a_profile_client_prices_the_usage() {
        // Spec 10.6: prices come from the profile.
        let body = canned_body().to_string();
        let (url, _) = fake_server(Some((200, Box::leak(body.into_boxed_str()))));
        let profile = Profile {
            endpoint: url,
            price_per_million_input_usd: 1.0,
            ..crate::profile::Profiles::bundled()
                .resolve("jev-latest")
                .expect("bundled")
                .clone()
        };
        let client = HttpJevClient::for_profile_with_key(&profile, "test-key").expect("client");
        let response = client.send(&triage_request()).expect("200");
        assert!((response.usage.usd.expect("priced") - 320.0 / 1_000_000.0).abs() < 1e-12);
    }

    #[test]
    fn a_400_is_rejected_with_the_message() {
        // Spec 12: 4xx is `jev_rejected`, not retryable, carrying the message.
        let (url, _) = fake_server(Some((
            400,
            r#"{"error": {"message": "criteria must have at least two options"}}"#,
        )));
        let client = HttpJevClient::new(url, "test-key", "jev-latest").expect("client");
        let error = client.send(&triage_request()).expect_err("400");
        let JevError::Rejected(message) = &error else {
            panic!("{error}")
        };
        assert!(message.contains("at least two options"), "{message}");
        let runtime: RuntimeError = error.into();
        assert_eq!(runtime.code, RuntimeErrorCode::JevRejected);
        assert!(!runtime.retryable);
    }

    #[test]
    fn a_503_is_unavailable_and_retryable() {
        let (url, _) = fake_server(Some((503, "overloaded")));
        let client = HttpJevClient::new(url, "test-key", "jev-latest").expect("client");
        let error = client.send(&triage_request()).expect_err("503");
        assert!(matches!(error, JevError::Unavailable(_)), "{error}");
        let runtime: RuntimeError = error.into();
        assert_eq!(runtime.code, RuntimeErrorCode::JevUnavailable);
        assert!(runtime.retryable);
    }

    #[test]
    fn a_429_is_unavailable_and_retryable() {
        // Spec 12: a rate limit is retried like an outage, not rejected.
        let (url, _) = fake_server(Some((429, r#"{"error": {"message": "slow down"}}"#)));
        let client = HttpJevClient::new(url, "test-key", "jev-latest").expect("client");
        let error = client.send(&triage_request()).expect_err("429");
        assert!(matches!(error, JevError::Unavailable(_)), "{error}");
        let runtime: RuntimeError = error.into();
        assert_eq!(runtime.code, RuntimeErrorCode::JevUnavailable);
        assert!(runtime.retryable);
    }

    #[test]
    fn a_dropped_connection_is_unavailable() {
        let (url, _) = fake_server(None);
        let client = HttpJevClient::new(url, "test-key", "jev-latest").expect("client");
        let error = client.send(&triage_request()).expect_err("dropped");
        assert!(matches!(error, JevError::Unavailable(_)), "{error}");
    }

    #[test]
    fn a_scripted_client_answers_in_order_and_remembers_requests() {
        let client = ScriptedJevClient::new(vec![JevResponse {
            answers: vec![JevAnswer::Noul {
                id: "urgent".into(),
                prob: 0.1,
            }],
            usage: JevUsage::default(),
            latency_ms: 1,
        }]);
        let first = client.send(&triage_request()).expect("first");
        assert_eq!(first.answers.len(), 1);
        assert!(client.send(&triage_request()).is_err());
        assert_eq!(client.request_count(), 2);
        assert_eq!(client.requests()[0].questions.len(), 4);

        let by_fn = ScriptedJevClient::from_fn(|request| {
            Ok(JevResponse {
                answers: request
                    .questions
                    .iter()
                    .map(|q| JevAnswer::Noul {
                        id: q.id().into(),
                        prob: 0.5,
                    })
                    .collect(),
                usage: JevUsage::default(),
                latency_ms: 0,
            })
        });
        assert_eq!(by_fn.send(&triage_request()).expect("fn").answers.len(), 4);
    }

    /// Run with: `set -a; . ./.env; set +a; cargo test -p jevscript-runtime -- --ignored`
    #[test]
    #[ignore = "needs TYPESAFE_API_KEY and the network"]
    fn live_inbox_triage_answers_have_the_spec_shapes() {
        let profile = crate::profile::Profiles::bundled()
            .resolve("jev-latest")
            .expect("bundled")
            .clone();
        let client = HttpJevClient::for_profile(&profile).expect("TYPESAFE_API_KEY is set");
        let response = client.send(&triage_request()).expect("live answer");
        eprintln!(
            "live response: {}",
            serde_json::to_string_pretty(&response).expect("json")
        );
        assert_eq!(response.answers.len(), 4);
        assert!(response.usage.tokens > 0);
        assert!(response.usage.usd.is_some());
        let JevAnswer::Noul { prob, .. } = &response.answers[0] else {
            panic!("urgent is a noul")
        };
        assert!((0.0..=1.0).contains(prob));
        let JevAnswer::Choice {
            label,
            probabilities,
            confidence,
            ..
        } = &response.answers[1]
        else {
            panic!("owner is a choice")
        };
        assert!(["code", "customer", "schedule", "other"].contains(&label.as_str()));
        assert_eq!(probabilities.len(), 4);
        assert!((0.0..=1.0).contains(confidence));
        let JevAnswer::Score {
            level,
            score,
            probabilities,
            ..
        } = &response.answers[3]
        else {
            panic!("effort is a score")
        };
        assert!(*level <= 2);
        assert!((0.0..=2.0).contains(score));
        assert!(
            probabilities.contains_key("trivial")
                && probabilities.contains_key("hour")
                && probabilities.contains_key("project")
        );
    }
}
