//! End-to-end fixtures for spec section 15 acceptance.

#![allow(
    dead_code,
    reason = "machine acceptance adds to this shared support module"
)]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use jevscript_runtime::capability::{
    AgentStatus, Bindings, CallArgs, Capability, CapabilityKind, Observation,
};
use jevscript_runtime::jev::{
    JevAnswer, JevClient, JevError, JevRequest, JevResponse, JevUsage, Question,
};
use jevscript_runtime::profile::{Profile, Tokenizer};
use jevscript_runtime::run::{Run, RunOptions};
use jevscript_runtime::{Handle, Pause, Resume, RuntimeError, RuntimeErrorCode, Value};

/// A model profile whose small question cap makes grouping observable.
pub fn tiny_profile() -> Profile {
    Profile {
        model: "jev-conformance-test".to_string(),
        endpoint: "http://invalid.test".to_string(),
        total_tokens: 100_000,
        state_plus_question_tokens: 50_000,
        max_questions_per_request: 4,
        max_criteria_per_question: 8,
        tokenizer: Tokenizer::Chars4,
        price_per_million_input_usd: 1.0,
        price_per_million_output_usd: 0.0,
        aliases: None,
    }
}

/// Run options selecting [`tiny_profile`] from a test-local profile bundle.
pub fn options(dir: &std::path::Path) -> RunOptions {
    let path = dir.join("profiles.json");
    std::fs::write(
        &path,
        serde_json::to_string(&vec![tiny_profile()]).expect("profile serializes"),
    )
    .expect("profile fixture is written");
    RunOptions {
        model: Some("jev-conformance-test".to_string()),
        profiles: Some(path),
        ..RunOptions::default()
    }
}

/// A fresh directory for one test's recordings and profile bundle.
pub fn temp_dir(name: &str) -> std::path::PathBuf {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock is after the Unix epoch")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "jevscript-conformance-{}-{name}-{nonce}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("temporary directory is created");
    dir
}

/// Drive a run through every pause, answering the pauses selected by `resume`.
pub fn drive(run: &mut Run, mut resume: impl FnMut(&Pause) -> Option<Resume>) -> Vec<Pause> {
    let mut pauses = Vec::new();
    for _ in 0..256 {
        let pause = run
            .next()
            .unwrap_or_else(|error| panic!("run failed: {error}"));
        let terminal = pause.is_terminal();
        pauses.push(pause.clone());
        if terminal {
            return pauses;
        }
        if let Some(payload) = resume(&pause) {
            run.resume(payload).expect("pause resumes");
        }
    }
    panic!("test run did not finish within 256 surfaced pauses")
}

/// A compact record-valued input.
pub fn record(fields: &[(&str, &str)]) -> Value {
    Value::Record(
        fields
            .iter()
            .map(|(key, value)| ((*key).to_string(), Value::Text((*value).to_string())))
            .collect(),
    )
}

/// A scripted answer table keyed by result id.
#[derive(Clone, Default)]
pub struct Answers {
    probs: BTreeMap<String, f64>,
    labels: BTreeMap<String, (String, f64)>,
    levels: BTreeMap<String, (u32, f64)>,
    each: BTreeMap<String, Vec<f64>>,
}

impl Answers {
    pub fn prob(mut self, id: &str, probability: f64) -> Self {
        self.probs.insert(id.to_string(), probability);
        self
    }

    pub fn label(mut self, id: &str, label: &str, confidence: f64) -> Self {
        self.labels
            .insert(id.to_string(), (label.to_string(), confidence));
        self
    }

    pub fn level(mut self, id: &str, level: u32, confidence: f64) -> Self {
        self.levels.insert(id.to_string(), (level, confidence));
        self
    }

    pub fn each(mut self, id: &str, probabilities: &[f64]) -> Self {
        self.each.insert(id.to_string(), probabilities.to_vec());
        self
    }

    pub fn response(&self, request: &JevRequest) -> JevResponse {
        JevResponse {
            answers: request
                .questions
                .iter()
                .map(|question| self.answer(question))
                .collect(),
            usage: JevUsage {
                tokens: 100,
                usd: None,
            },
            latency_ms: 1,
        }
    }

    fn answer(&self, question: &Question) -> JevAnswer {
        let id = question.id().to_string();
        let (base, index) = id
            .split_once('[')
            .map(|(base, suffix)| {
                (
                    base.to_string(),
                    suffix.trim_end_matches(']').parse::<usize>().ok(),
                )
            })
            .unwrap_or_else(|| (id.clone(), None));
        match question {
            Question::Noul { .. } => JevAnswer::Noul {
                id,
                prob: index
                    .and_then(|index| {
                        self.each
                            .get(&base)
                            .and_then(|values| values.get(index).copied())
                    })
                    .or_else(|| self.probs.get(&base).copied())
                    .unwrap_or(0.1),
            },
            Question::Choice { labels, .. } => {
                let (label, confidence) = self
                    .labels
                    .get(&base)
                    .cloned()
                    .unwrap_or_else(|| (labels[0].name.clone(), 0.9));
                let remainder = (1.0 - confidence) / (labels.len() - 1).max(1) as f64;
                JevAnswer::Choice {
                    id,
                    probabilities: labels
                        .iter()
                        .map(|candidate| {
                            (
                                candidate.name.clone(),
                                if candidate.name == label {
                                    confidence
                                } else {
                                    remainder
                                },
                            )
                        })
                        .collect(),
                    label,
                    confidence,
                }
            }
            Question::Score { levels, .. } => {
                let (level, confidence) = self.levels.get(&base).copied().unwrap_or((0, 0.9));
                let named = levels.iter().all(|candidate| candidate.name.is_some());
                JevAnswer::Score {
                    id,
                    level,
                    score: f64::from(level),
                    confidence,
                    probabilities: levels
                        .iter()
                        .enumerate()
                        .map(|(index, candidate)| {
                            let name = if named {
                                candidate.name.clone().expect("named level")
                            } else {
                                index.to_string()
                            };
                            let probability = if index as u32 == level {
                                confidence
                            } else {
                                (1.0 - confidence) / (levels.len() - 1).max(1) as f64
                            };
                            (name, probability)
                        })
                        .collect(),
                }
            }
        }
    }
}

impl JevClient for Answers {
    fn send(&self, request: &JevRequest) -> Result<JevResponse, JevError> {
        Ok(self.response(request))
    }
}

/// A Jev client proving replay never reaches the model boundary.
pub struct NeverJev;

impl JevClient for NeverJev {
    fn send(&self, request: &JevRequest) -> Result<JevResponse, JevError> {
        panic!(
            "replay called Jev with {} questions",
            request.questions.len()
        )
    }
}

/// Shared adapter-call evidence from a live run.
pub type CallLog = Arc<Mutex<Vec<String>>>;

pub fn call_log() -> CallLog {
    Arc::new(Mutex::new(Vec::new()))
}

pub fn logged(log: &CallLog) -> Vec<String> {
    log.lock().expect("call log is not poisoned").clone()
}

fn log_call(log: &CallLog, capability: &str, verb: &str, args: &CallArgs) {
    let positional = args.positional.iter().map(Value::to_text);
    let named = args
        .named
        .iter()
        .map(|(name, value)| format!("{name}={}", value.to_text()));
    log.lock().expect("call log is not poisoned").push(format!(
        "{capability}.{verb}({})",
        positional.chain(named).collect::<Vec<_>>().join(", ")
    ));
}

/// A scripted open tool adapter.
pub struct FakeTool {
    name: String,
    log: CallLog,
    values: BTreeMap<String, Vec<Value>>,
    calls: BTreeMap<String, usize>,
}

impl FakeTool {
    pub fn new(name: &str, log: &CallLog) -> Self {
        Self {
            name: name.to_string(),
            log: Arc::clone(log),
            values: BTreeMap::new(),
            calls: BTreeMap::new(),
        }
    }

    pub fn verb(mut self, name: &str, values: Vec<Value>) -> Self {
        self.values.insert(name.to_string(), values);
        self
    }
}

impl Capability for FakeTool {
    fn kind(&self) -> CapabilityKind {
        CapabilityKind::Tool
    }

    fn call(&mut self, verb: &str, args: &CallArgs) -> Result<Value, RuntimeError> {
        log_call(&self.log, &self.name, verb, args);
        let values = self.values.get(verb).ok_or_else(|| {
            RuntimeError::new(
                RuntimeErrorCode::VerbMissing,
                format!("{} has no `{verb}`", self.name),
            )
        })?;
        let call = self.calls.entry(verb.to_string()).or_default();
        let value = values
            .get(*call)
            .or_else(|| values.last())
            .cloned()
            .unwrap_or(Value::None);
        *call += 1;
        Ok(value)
    }
}

/// A scripted agent whose wait and observe calls consume observations in order.
pub struct FakeAgent {
    name: String,
    log: CallLog,
    observations: Vec<Observation>,
    observed: usize,
    spawned: u32,
}

impl FakeAgent {
    pub fn new(name: &str, log: &CallLog, observations: Vec<Observation>) -> Self {
        Self {
            name: name.to_string(),
            log: Arc::clone(log),
            observations,
            observed: 0,
            spawned: 0,
        }
    }

    fn next_observation(&mut self) -> Observation {
        let observation = self
            .observations
            .get(self.observed)
            .or_else(|| self.observations.last())
            .cloned()
            .unwrap_or_else(|| observation("running", ""));
        self.observed += 1;
        observation
    }
}

impl Capability for FakeAgent {
    fn kind(&self) -> CapabilityKind {
        CapabilityKind::Agent
    }

    fn call(&mut self, verb: &str, args: &CallArgs) -> Result<Value, RuntimeError> {
        log_call(&self.log, &self.name, verb, args);
        match verb {
            "spawn" => {
                self.spawned += 1;
                Ok(Value::Handle(Handle {
                    capability: self.name.clone(),
                    id: format!("agent-{}", self.spawned),
                    fields: BTreeMap::new(),
                }))
            }
            "wait" => Ok(Value::from_json(
                &serde_json::to_value(self.next_observation()).expect("observation serializes"),
            )),
            "send" | "stop" => Ok(Value::None),
            _ => Err(RuntimeError::new(
                RuntimeErrorCode::VerbMissing,
                format!("agent has no `{verb}`"),
            )),
        }
    }

    fn observe(&mut self, handle: &Handle) -> Result<Observation, RuntimeError> {
        self.log
            .lock()
            .expect("call log is not poisoned")
            .push(format!("{}.observe({})", self.name, handle.id));
        Ok(self.next_observation())
    }
}

pub fn observation(status: &str, message: &str) -> Observation {
    Observation {
        status: match status {
            "waiting" => AgentStatus::Waiting,
            "exited" => AgentStatus::Exited,
            _ => AgentStatus::Running,
        },
        last_message: message.to_string(),
        tail: message.to_string(),
        exit_code: None,
        fields: BTreeMap::new(),
    }
}

/// A person adapter; pauses are runtime-owned, and notifications are logged.
pub struct FakePerson {
    name: String,
    log: CallLog,
}

impl FakePerson {
    pub fn new(name: &str, log: &CallLog) -> Self {
        Self {
            name: name.to_string(),
            log: Arc::clone(log),
        }
    }
}

impl Capability for FakePerson {
    fn kind(&self) -> CapabilityKind {
        CapabilityKind::Person
    }

    fn call(&mut self, verb: &str, args: &CallArgs) -> Result<Value, RuntimeError> {
        log_call(&self.log, &self.name, verb, args);
        if verb == "notify" {
            Ok(Value::None)
        } else {
            Err(RuntimeError::new(
                RuntimeErrorCode::AdapterError,
                format!("`{verb}` should surface as a pause"),
            ))
        }
    }
}

/// A deterministic generation adapter.
pub struct FakeLlm {
    name: String,
    log: CallLog,
}

impl FakeLlm {
    pub fn new(name: &str, log: &CallLog) -> Self {
        Self {
            name: name.to_string(),
            log: Arc::clone(log),
        }
    }
}

impl Capability for FakeLlm {
    fn kind(&self) -> CapabilityKind {
        CapabilityKind::Llm
    }

    fn call(&mut self, verb: &str, args: &CallArgs) -> Result<Value, RuntimeError> {
        log_call(&self.log, &self.name, verb, args);
        let using = args.named("using").map(Value::to_text).unwrap_or_default();
        Ok(Value::Text(format!("draft: {using}")))
    }
}

/// An adapter that panics if replay attempts an external effect.
pub struct NeverCapability(pub CapabilityKind);

impl Capability for NeverCapability {
    fn kind(&self) -> CapabilityKind {
        self.0
    }

    fn call(&mut self, verb: &str, _args: &CallArgs) -> Result<Value, RuntimeError> {
        panic!("replay called adapter verb `{verb}`")
    }

    fn observe(&mut self, _handle: &Handle) -> Result<Observation, RuntimeError> {
        panic!("replay observed an adapter handle")
    }
}

pub fn bindings(adapters: Vec<(&str, Box<dyn Capability>)>) -> Bindings {
    adapters
        .into_iter()
        .map(|(name, adapter)| (name.to_string(), adapter))
        .collect()
}
