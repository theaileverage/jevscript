//! The JSON-RPC 2.0 method set (spec section 11.5).
//!
//! Version 0.1 runs the runtime as a local process — `jevscript serve` — and the
//! SDKs speak JSON-RPC to it over stdio, one JSON object per line. Adapters
//! live in the host process, which is why capabilities are reached by a request
//! back to the host rather than by the runtime linking against anything.
//!
//! The method names here are the contract. In-process embedding through
//! `napi-rs` or `PyO3`, or a WebAssembly build, is a later version and exposes
//! the same calls.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::capability::Observation;
use crate::pause::{Pause, Resume};
use crate::record::RecordedEvent;
use crate::value::Value;

/// Methods the host calls on the runtime.
pub mod method {
    /// Compile a program and hold it.
    pub const PROGRAM_LOAD: &str = "program.load";
    /// Run one judgment against a state. Needs no bindings.
    pub const JUDGMENT_RUN: &str = "judgment.run";
    /// Start a task.
    pub const TASK_START: &str = "task.start";
    /// Advance to the next pause.
    pub const RUN_NEXT: &str = "run.next";
    /// Answer the open pause.
    pub const RUN_RESUME: &str = "run.resume";
    /// Give up on a run.
    pub const RUN_ABORT: &str = "run.abort";
    /// Send a message to a capability during a `waiting` pause.
    pub const RUN_INJECT: &str = "run.inject";

    /// Every method the host may call, for dispatch and for the
    /// `method_not_found` message.
    pub const ALL: [&str; 7] = [
        PROGRAM_LOAD,
        JUDGMENT_RUN,
        TASK_START,
        RUN_NEXT,
        RUN_RESUME,
        RUN_ABORT,
        RUN_INJECT,
    ];
}

/// Requests and notifications the runtime sends back to the host.
pub mod host_method {
    /// Run one capability verb in the host process.
    pub const CAPABILITY_CALL: &str = "capability.call";
    /// Observe an agent handle in the host process.
    pub const CAPABILITY_OBSERVE: &str = "capability.observe";
    /// A recording event, as a notification with no reply.
    pub const EVENT: &str = "event";
}

/// `program.load { source | path }`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProgramLoadParams {
    /// The program's source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// A path to read the source from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Module search roots used while linking (spec section 3.9).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
}

/// What `program.load` answers with: enough for a host to describe a program
/// without executing it (spec section 11.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProgramLoadResult {
    /// The handle later calls pass.
    pub program_id: String,
    /// The program's name.
    pub name: String,
    /// Its declared inputs.
    pub inputs: Vec<InputMetadata>,
    /// Its declared capabilities, with kinds.
    pub needs: Vec<CapabilityMetadata>,
    /// Its judgments, with their answer spaces and shape hashes.
    pub judgments: Vec<JudgmentMetadata>,
}

/// One declared input without compiler-only source positions (spec 11.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InputMetadata {
    /// The input name.
    pub name: String,
    /// Its declared host-facing shape.
    pub shape: jevscript_ir::Shape,
}

impl From<&jevscript_ir::Input> for InputMetadata {
    fn from(input: &jevscript_ir::Input) -> Self {
        Self {
            name: input.name.clone(),
            shape: input.shape.clone(),
        }
    }
}

/// One declared capability without compiler-only signatures or spans (spec 11.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CapabilityMetadata {
    /// The program-scoped capability name.
    pub name: String,
    /// Its fixed capability kind.
    pub kind: jevscript_ir::CapabilityKind,
}

impl From<&jevscript_ir::Need> for CapabilityMetadata {
    fn from(need: &jevscript_ir::Need) -> Self {
        Self {
            name: need.name.clone(),
            kind: need.kind,
        }
    }
}

/// A judgment's stable host metadata and answer spaces (spec sections 11.1 and 11.4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JudgmentMetadata {
    /// The linked judgment name.
    pub name: String,
    /// The state parameters accepted by standalone judgment execution.
    pub params: Vec<String>,
    /// One answer space per declared result.
    pub results: Vec<JudgmentResultMetadata>,
    /// The shape hash a host can pin.
    pub shape_hash: String,
}

impl From<&jevscript_ir::Judgment> for JudgmentMetadata {
    fn from(judgment: &jevscript_ir::Judgment) -> Self {
        Self {
            name: judgment.name.clone(),
            params: judgment.params.clone(),
            results: judgment
                .results
                .iter()
                .map(JudgmentResultMetadata::from)
                .collect(),
            shape_hash: judgment.shape_hash.clone(),
        }
    }
}

/// One named judgment result and only the answer-space data hosts consume (spec 11.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JudgmentResultMetadata {
    /// The result name.
    pub name: String,
    /// Whether the question is repeated for each subject item.
    pub each: bool,
    /// The possible answer shape, flattened to the section 11.2 wire object.
    #[serde(flatten)]
    pub answer: AnswerSpace,
}

impl From<&jevscript_ir::JudgmentResult> for JudgmentResultMetadata {
    fn from(result: &jevscript_ir::JudgmentResult) -> Self {
        use jevscript_ir::JudgeVerb;
        let answer = match &result.question.verb {
            JudgeVerb::Feels { .. } => AnswerSpace::Feels,
            JudgeVerb::Pick { labels } => AnswerSpace::Pick {
                labels: labels.iter().map(|label| label.name.clone()).collect(),
            },
            JudgeVerb::Rate { levels } => AnswerSpace::Rate {
                levels: levels
                    .iter()
                    .enumerate()
                    .map(|(index, level)| LevelMetadata {
                        index,
                        name: level.name.clone(),
                    })
                    .collect(),
            },
            JudgeVerb::PickAmong { allow_none, .. } => AnswerSpace::PickAmong {
                allow_none: *allow_none,
            },
        };
        Self {
            name: result.name.clone(),
            each: result.question.each,
            answer,
        }
    }
}

/// A judgment result's public answer space, stripped of executable IR (spec 11.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "verb", rename_all = "snake_case")]
pub enum AnswerSpace {
    /// A probability in `[0, 1]`.
    Feels,
    /// One of the declared labels.
    Pick {
        /// Label names in declaration order.
        labels: Vec<String>,
    },
    /// A level index, with optional stable names in declaration order.
    Rate {
        /// Levels in declaration order.
        levels: Vec<LevelMetadata>,
    },
    /// A choice over runtime list items rather than fixed labels.
    PickAmong {
        /// Whether the `none` answer is available.
        allow_none: bool,
    },
}

/// One public rate level in a judgment answer space (spec section 11.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LevelMetadata {
    /// The zero-based level index.
    pub index: usize,
    /// The stable name, when the level is named.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// `judgment.run { program_id, name, state, model? }`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JudgmentRunParams {
    /// Which program.
    pub program_id: String,
    /// Which judgment.
    pub name: String,
    /// The state its parameters are filled from.
    pub state: BTreeMap<String, serde_json::Value>,
    /// A Jev model id, overriding the runtime's default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// A profiles file to layer before resolving `model` (spec section 10.6).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profiles: Option<String>,
}

/// What `judgment.run` answers with: exactly one Jev request's answers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JudgmentRunResult {
    /// One answer per question, keyed by result name.
    pub answers: BTreeMap<String, Value>,
    /// The judgment's `log` lines, in the order they ran (spec section 5.8).
    /// Absent when it wrote none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub logs: Vec<crate::record::LogRecord>,
}

/// One binding, as `task.start` carries it: name and kind only. The adapter
/// itself stays in the host process.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BindingParam {
    /// The capability's program-scoped name.
    pub name: String,
    /// Its kind.
    pub kind: String,
    /// A `tool` adapter's manifest, checked before the run starts
    /// (spec section 9.4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest: Option<crate::capability::ToolManifest>,
}

/// `task.start { program_id, name, inputs, bindings, record?, replay?, redact?, model? }`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskStartParams {
    /// Which program.
    pub program_id: String,
    /// Which task.
    pub name: String,
    /// The inputs, matching the program's `in` declarations.
    #[serde(default)]
    pub inputs: BTreeMap<String, Value>,
    /// The capabilities the host has bound.
    #[serde(default)]
    pub bindings: Vec<BindingParam>,
    /// Where to write a recording.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub record: Option<String>,
    /// A recording to replay instead of calling anything.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replay: Option<String>,
    /// Whether to store a hash and a token count instead of payloads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redact: Option<bool>,
    /// A Jev model id, overriding the runtime's default. It also selects the
    /// model profile (spec section 10.6).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Draw labels and levels from Jev's distribution instead of taking the
    /// argmax (spec section 6.11).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample: Option<Sample>,
    /// A profiles file to layer over the bundled ones (spec section 10.6).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profiles: Option<String>,
    /// Module search roots for `use` paths that are not relative
    /// (spec section 3.9).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
}

/// `sample: bool | { seed: number }` (spec section 6.11).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Sample {
    /// `true` samples with an unseeded draw; `false` is the default argmax.
    On(bool),
    /// A seeded draw, so a sampled run can be reproduced without a recording.
    Seeded {
        /// The seed.
        seed: u64,
    },
}

impl Default for Sample {
    fn default() -> Self {
        Self::On(false)
    }
}

/// What `task.start` answers with.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskStartResult {
    /// The handle later calls pass.
    pub run_id: String,
}

/// `run.next { run_id }`, `run.abort { run_id }`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunParams {
    /// Which run.
    pub run_id: String,
}

/// `run.resume { run_id, payload }`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunResumeParams {
    /// Which run.
    pub run_id: String,
    /// What the host answers with.
    pub payload: Resume,
}

/// `run.inject { run_id, capability, message }`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunInjectParams {
    /// Which run.
    pub run_id: String,
    /// Which capability the run is waiting on.
    pub capability: String,
    /// What to send it.
    pub message: String,
}

/// What `run.next` answers with.
pub type RunNextResult = Pause;

/// What `run.resume`, `run.abort` and `run.inject` answer with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ok {
    /// Always `true`.
    pub ok: bool,
}

impl Default for Ok {
    fn default() -> Self {
        Self { ok: true }
    }
}

/// `capability.call { run_id, capability, verb, args }`, runtime to host.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CapabilityCallParams {
    /// Which run.
    pub run_id: String,
    /// Which capability.
    pub capability: String,
    /// Which verb.
    pub verb: String,
    /// Its arguments.
    pub args: crate::capability::CallArgs,
}

/// `capability.observe { run_id, capability, handle }`, runtime to host.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CapabilityObserveParams {
    /// Which run.
    pub run_id: String,
    /// Which capability.
    pub capability: String,
    /// Which handle.
    pub handle: crate::value::Handle,
}

/// What the host answers `capability.observe` with.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CapabilityObserveResult {
    /// What was observed.
    pub observation: Observation,
}

/// The host-side adapter error payload of spec sections 11.5 and 12.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdapterErrorResult {
    /// What the adapter reported.
    pub message: String,
    /// Whether the run may retry the failed operation.
    #[serde(default)]
    pub retryable: bool,
}

/// `event { run_id, event }`, runtime to host, a notification with no reply.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventParams {
    /// Which run.
    pub run_id: String,
    /// The recording event, as it was written.
    pub event: RecordedEvent,
}
