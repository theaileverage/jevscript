//! Recording and replay (spec sections 10.3 and 10.4).
//!
//! A recording is a JSONL file: one event per line, each with `ts`, `run_id`,
//! `seq` and `event`. Every judgment, generation, capability result and random
//! draw goes in, so a recording replays with zero model calls and identical
//! control flow.
//!
//! Determinism guarantee: given the same program, inputs and recording, control
//! flow is identical up to the first host answer that differs. `ts` is the one
//! field that is never replayed: it is metadata from the system clock, and the
//! clock a program observes through `now()` is a `draw` event instead.

use jevscript_syntax::Span;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use crate::capability::Observation;
use crate::error::{RuntimeError, RuntimeErrorCode};
use crate::jev::{JevAnswer, JevUsage, Question};
use crate::pause::{Pause, PauseKind, Resume, Usage};
use crate::profile::Profile;
use crate::rpc::Sample;
use crate::tokens::{CharsPerToken, TokenEstimator};
use crate::value::Value;

/// A step record, built by the runtime from its own calls (spec section 7.5).
///
/// The runtime writes these, not any model, so nothing an agent prints can
/// steer them. `trail <n>` reads the last `n` of them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StepRecord {
    /// Which step.
    pub step: u64,
    /// The capability verb.
    pub action: String,
    /// The capability or handle.
    pub target: String,
    /// The arguments' text form, capped at 200 tokens.
    pub args: String,
    /// Whether the next observation differed from the previous one, by hashing
    /// the text form of the observation that followed the action.
    pub changed: bool,
    /// How the step turned out.
    pub outcome: StepOutcome,
}

/// How a step turned out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[allow(missing_docs, reason = "each variant is the outcome it names")]
pub enum StepOutcome {
    Observed,
    Error,
    NoChange,
}

/// One evaluated `log` (spec section 5.8), in full.
///
/// The interpreter builds this; the recording holds it as [`Event::Log`],
/// whose message and field values a redacting recorder replaces with
/// redaction markers. A standalone judgment run returns these directly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LogRecord {
    /// The level the program wrote.
    pub level: jevscript_ir::LogLevel,
    /// The text form of the logged value (spec section 4.2).
    pub message: String,
    /// The structured fields, by name.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub fields: BTreeMap<String, Value>,
    /// The task, machine or judgment it ran under, named as a pause names it.
    pub task: String,
    /// Where the `log` was written.
    pub source: Span,
}

impl LogRecord {
    /// The recording event for this log, stored in full.
    pub fn into_event(self) -> Event {
        Event::Log {
            level: self.level,
            message: Recorded::Full(self.message),
            fields: self
                .fields
                .into_iter()
                .map(|(name, value)| (name, Recorded::Full(value)))
                .collect(),
            task: self.task,
            source: self.source,
        }
    }
}

/// Which external operation failed (spec section 10.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectOperation {
    /// A request to Jev.
    Request,
    /// A capability verb call.
    Call,
    /// An agent observation.
    Observe,
    /// An `llm.write` generation.
    Generate,
}

/// The exact identity of one failed external attempt (spec section 10.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum EffectIdentity {
    /// The request whose inputs are in the preceding `request` event.
    Request {
        /// The request id.
        request_id: String,
    },
    /// A capability verb and its arguments.
    Call {
        /// Which capability.
        capability: String,
        /// Which verb.
        verb: String,
        /// The exact argument value.
        args: Value,
    },
    /// An observation of a particular handle.
    Observe {
        /// Which capability.
        capability: String,
        /// Which handle.
        handle: Value,
    },
    /// A generation instruction and context.
    Generate {
        /// Which capability.
        capability: String,
        /// The instruction.
        instruction: String,
        /// The optional context.
        using: Option<Value>,
    },
}

/// What a redacted payload is stored as (spec section 10.3): enough to replay
/// control flow and to prove what was sent, not enough to read it again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Redacted {
    /// The SHA-256 of the payload's canonical JSON, as lowercase hex.
    pub hash: String,
    /// The estimated token count of that JSON.
    pub tokens: u64,
}

/// A payload that a host may have asked to redact (spec section 10.3).
///
/// Ordinary full payloads keep their natural JSON shape. The two ambiguous
/// object shapes are escaped as `{"$recorded_full": payload}` so legal program
/// data can never be mistaken for recording metadata (spec section 10.3).
#[derive(Debug, Clone, PartialEq)]
pub enum Recorded<T> {
    /// A hash and a token count instead of the payload.
    Redacted {
        /// The stand-in.
        redacted: Redacted,
    },
    /// The payload in full.
    Full(T),
}

impl<T: Serialize> Serialize for Recorded<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let value = match self {
            Self::Redacted { redacted } => serde_json::json!({ "redacted": redacted }),
            Self::Full(payload) => {
                let value = serde_json::to_value(payload).map_err(serde::ser::Error::custom)?;
                if redacted_marker(&value).is_some() || is_full_envelope(&value) {
                    serde_json::json!({ "$recorded_full": value })
                } else {
                    value
                }
            }
        };
        value.serialize(serializer)
    }
}

impl<'de, T: DeserializeOwned> Deserialize<'de> for Recorded<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        if let Some(payload) = full_envelope_payload(&value) {
            return serde_json::from_value(payload.clone())
                .map(Self::Full)
                .map_err(serde::de::Error::custom);
        }
        if let Some(redacted) = redacted_marker(&value) {
            return Ok(Self::Redacted { redacted });
        }
        serde_json::from_value(value)
            .map(Self::Full)
            .map_err(serde::de::Error::custom)
    }
}

fn redacted_marker(value: &serde_json::Value) -> Option<Redacted> {
    let root = value.as_object()?;
    if root.len() != 1 {
        return None;
    }
    let metadata = root.get("redacted")?.as_object()?;
    if metadata.len() != 2
        || !metadata.get("hash")?.is_string()
        || metadata.get("tokens")?.as_u64().is_none()
    {
        return None;
    }
    serde_json::from_value(root.get("redacted")?.clone()).ok()
}

fn is_full_envelope(value: &serde_json::Value) -> bool {
    full_envelope_payload(value).is_some()
}

fn full_envelope_payload(value: &serde_json::Value) -> Option<&serde_json::Value> {
    let root = value.as_object()?;
    (root.len() == 1)
        .then(|| root.get("$recorded_full"))
        .flatten()
}

impl<T> Recorded<T> {
    /// The payload, if it was stored in full.
    pub const fn full(&self) -> Option<&T> {
        match self {
            Recorded::Full(payload) => Some(payload),
            Recorded::Redacted { .. } => None,
        }
    }

    /// Whether the payload was redacted.
    pub const fn is_redacted(&self) -> bool {
        matches!(self, Recorded::Redacted { .. })
    }
}

impl<T> From<T> for Recorded<T> {
    fn from(payload: T) -> Self {
        Recorded::Full(payload)
    }
}

/// The event kinds a recording holds (spec section 10.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    /// The run began.
    Start {
        /// Which program.
        program: String,
        /// Which task.
        task: String,
        /// The inputs it was started with.
        inputs: BTreeMap<String, Value>,
        /// Names and kinds only; never the adapters themselves.
        bindings: Vec<BindingRecord>,
        /// The IR version the program was compiled to.
        ir_version: String,
    },
    /// What was sent to Jev.
    Request {
        /// Ties the request to its answers.
        request_id: String,
        /// The state, exactly as section 6.9 describes it, or its redaction.
        state: Recorded<BTreeMap<String, serde_json::Value>>,
        /// Every question in the request.
        questions: Vec<Question>,
    },
    /// What came back.
    Answers {
        /// Which request.
        request_id: String,
        /// One answer per question.
        answers: Vec<JevAnswer>,
        /// What it cost.
        usage: JevUsage,
        /// How long it took.
        latency_ms: u64,
        /// Whether a label or level in this response was drawn from the
        /// distribution rather than taken as the argmax (spec section 6.11).
        /// The draw itself is a `draw` event, so a sampled run replays exactly.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        sampled: bool,
    },
    /// An `llm` generation.
    Generate {
        /// Which capability.
        capability: String,
        /// The instruction it was given.
        instruction: String,
        /// The only context it saw.
        using: Option<Value>,
        /// What it wrote.
        output: String,
        /// What it cost.
        usage: JevUsage,
    },
    /// A capability call and its result.
    Call {
        /// Which capability.
        capability: String,
        /// Which verb.
        verb: String,
        /// What it was called with.
        args: Value,
        /// What it returned, or its redaction for an agent `wait` call.
        ///
        /// The untagged full form preserves the original JSON recording shape.
        result: Recorded<Value>,
        /// How long it took.
        latency_ms: u64,
    },
    /// An observation of an agent.
    Observe {
        /// Which capability.
        capability: String,
        /// Which handle.
        handle: Value,
        /// What was observed, or its redaction.
        observation: Recorded<Observation>,
    },
    /// A failed external attempt, recorded before retry, pause or abort.
    EffectError {
        /// Which kind of external operation failed.
        operation: EffectOperation,
        /// The operation's exact replay identity.
        identity: EffectIdentity,
        /// The runtime error code.
        code: RuntimeErrorCode,
        /// The runtime error message.
        message: String,
        /// Whether the host may retry the operation.
        retryable: bool,
        /// Where the operation occurred in the program.
        source: Span,
    },
    /// A draw from the clock or the random source, so replay can reproduce it.
    Draw {
        /// Which source.
        kind: DrawKind,
        /// What it gave.
        value: serde_json::Value,
    },
    /// Something the runtime wants the host to know but did not fail on, such as
    /// a `shape` field that was truncated.
    Warning {
        /// Which warning.
        code: String,
        /// What happened.
        message: String,
        /// Where in the `.jev` file.
        source: Span,
    },
    /// A `log` the program evaluated (spec section 5.8). It never reaches Jev
    /// and costs nothing; replay checks it like a `warning` and does not
    /// stream it to the host again.
    Log {
        /// Which level.
        level: jevscript_ir::LogLevel,
        /// The logged value's text form, or its redaction.
        message: Recorded<String>,
        /// The structured fields, each value in full or redacted.
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        fields: BTreeMap<String, Recorded<Value>>,
        /// The task, machine or judgment it ran under.
        task: String,
        /// Where in the `.jev` file.
        source: Span,
    },
    /// A pause was surfaced to the host.
    Pause {
        /// Which kind.
        kind: PauseKind,
        /// The whole pause.
        payload: Pause,
    },
    /// The host answered a pause.
    Resume {
        /// What it answered with.
        payload: Resume,
    },
    /// The host cancelled the run (spec sections 9.6 and 10.3).
    Abort {
        /// Why the host cancelled it.
        reason: String,
    },
    /// One step of a task (spec section 7.5).
    Step {
        /// The step record.
        record: StepRecord,
    },
    /// One step of a machine (spec section 7.8).
    ///
    /// Written per step, before the transition is applied, so a recording shows
    /// exactly which events were on the menu and which one Jev chose.
    MachineStep {
        /// The machine's qualified name.
        machine: String,
        /// The state the step started in.
        state: String,
        /// The event names Jev was offered, guards already applied. `stay` is
        /// always among them.
        enabled: Vec<String>,
        /// The event Jev chose, or `stay`.
        chosen: String,
        /// Label to probability, over exactly the enabled events plus `stay`.
        probabilities: BTreeMap<String, f64>,
        /// How peaked the distribution was, which is what the step's gate reads.
        confidence: f64,
        /// The state the machine moved to. Unchanged when the choice was `stay`
        /// or when a gate declined the transition.
        to: String,
    },
    /// The run ended.
    End {
        /// Which terminal pause ended it.
        kind: PauseKind,
        /// Every declared output.
        outputs: BTreeMap<String, Value>,
        /// Whether a declared `verify` was true at the end.
        verified: bool,
        /// What the whole run cost.
        usage: Usage,
    },
}

/// The fixed reason recorded for host cancellation (spec section 10.3).
pub const HOST_ABORT_REASON: &str = "aborted by host";

/// Which non-deterministic source a `draw` event came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DrawKind {
    /// `random()`.
    Random,
    /// `now()`.
    Now,
}

/// A binding as it is recorded: name and kind only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BindingRecord {
    /// The capability's program-scoped name.
    pub name: String,
    /// Its kind.
    pub kind: String,
}

/// The execution metadata embedded on a `start` line (spec section 10.3).
///
/// It is carried by [`RecordedEvent`] rather than [`Event::Start`] so the
/// interpreter can keep emitting the established start shape while the run
/// driver, which owns the resolved profile and effective sampling option,
/// makes the on-disk event self-contained.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExecutionMetadata {
    /// The complete linked program the runtime executes.
    pub ir: jevscript_ir::Ir,
    /// The concrete profile selected for this run, after aliases and overlays.
    pub profile: Profile,
    /// The effective run sampling option; the default is explicitly `false`.
    pub sample: Sample,
}

/// One line of a recording.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordedEvent {
    /// When it was written, as ISO text.
    pub ts: String,
    /// Which run.
    pub run_id: String,
    /// Its position in the recording, from 0.
    pub seq: u64,
    /// Self-contained execution metadata, present only on the `start` line.
    #[serde(default, flatten, skip_serializing_if = "Option::is_none")]
    pub execution: Option<ExecutionMetadata>,
    /// The event itself.
    #[serde(flatten)]
    pub event: Event,
}

/// Whether payloads are stored in full.
///
/// `Redact` stores a hash and a token count instead of state and observation
/// payloads in the primary file. A private full companion makes the pair a
/// replay artifact without exposing those payloads in the primary (spec 10.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Redaction {
    /// Store state and observations in full. The default.
    #[default]
    Full,
    /// Store a hash and a token count instead.
    Redact,
}

/// Writes a recording.
pub trait Recorder: Send {
    /// Append an event, returning the byte offset it was written at. That
    /// offset is what a pause reports as `recording_offset`.
    ///
    /// # Errors
    ///
    /// Fails if the recording cannot be written.
    fn write(&mut self, event: Event) -> Result<u64, RuntimeError>;

    /// The offset the next event will be written at.
    fn offset(&self) -> u64;

    /// Flush and close.
    ///
    /// # Errors
    ///
    /// Fails if the recording cannot be flushed.
    fn close(&mut self) -> Result<(), RuntimeError>;
}

/// A host callback that receives each event when it is appended (spec 9.6).
pub type EventSink = Box<dyn FnMut(&RecordedEvent) -> Result<(), RuntimeError> + Send>;

/// Reads a recording back.
///
/// Each replayed event must match the next expected event's kind and identity —
/// request shape, capability and verb. A mismatch is
/// [`crate::error::RuntimeErrorCode::ReplayDiverged`], naming the source
/// location.
pub trait Replayer: Send {
    /// The next event, or `None` at the end of the recording.
    ///
    /// # Errors
    ///
    /// Fails if the recording cannot be read or a line does not parse.
    fn next(&mut self) -> Result<Option<RecordedEvent>, RuntimeError>;

    /// The next event without consuming it.
    ///
    /// # Errors
    ///
    /// Fails if the recording cannot be read or a line does not parse.
    fn peek(&mut self) -> Result<Option<&RecordedEvent>, RuntimeError>;
}

/// The redaction of a payload: the SHA-256 of its canonical JSON and the
/// estimated tokens of that JSON (spec section 10.3).
pub fn redact<T: Serialize>(payload: &T, estimator: &dyn TokenEstimator) -> Redacted {
    let json = serde_json::to_string(payload).unwrap_or_default();
    let hash = Sha256::digest(json.as_bytes());
    Redacted {
        hash: hash.iter().map(|b| format!("{b:02x}")).collect(),
        tokens: estimator.estimate(&json),
    }
}

/// A recorder that writes one new JSONL recording to a path.
pub struct JsonlRecorder {
    path: PathBuf,
    run_id: String,
    redaction: Redaction,
    estimator: Box<dyn TokenEstimator>,
    file: Option<File>,
    companion_file: Option<File>,
    offset: u64,
    seq: u64,
    execution: Option<ExecutionMetadata>,
    agent_capabilities: BTreeSet<String>,
}

impl JsonlRecorder {
    /// Create a recorder for `run_id`, refusing an existing primary or replay
    /// companion before the run can execute (spec section 10.3).
    ///
    /// # Errors
    ///
    /// Fails without changing either pre-existing path when the new recording
    /// pair cannot be created safely.
    pub fn new(
        path: impl Into<PathBuf>,
        run_id: impl Into<String>,
        redaction: Redaction,
    ) -> Result<Self, RuntimeError> {
        let path = path.into();
        let file = create_new_file(&path, false)?;
        let companion_file = if redaction == Redaction::Redact {
            let companion = companion_path(&path);
            match create_new_file(&companion, true) {
                Ok(file) => Some(file),
                Err(error) => {
                    drop(file);
                    let _ = std::fs::remove_file(&path);
                    return Err(error);
                }
            }
        } else {
            None
        };
        Ok(Self {
            path,
            run_id: run_id.into(),
            redaction,
            estimator: Box::new(CharsPerToken),
            file: Some(file),
            companion_file,
            offset: 0,
            seq: 0,
            execution: None,
            agent_capabilities: BTreeSet::new(),
        })
    }

    /// Attach the metadata a `start` line needs for standalone replay.
    #[must_use]
    pub fn with_execution(mut self, execution: ExecutionMetadata) -> Self {
        self.execution = Some(execution);
        self
    }

    /// Use the profile's estimator for the token count of a redacted payload.
    #[must_use]
    pub fn with_estimator(mut self, estimator: Box<dyn TokenEstimator>) -> Self {
        self.estimator = estimator;
        self
    }

    /// Where the recording is written.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Which run this recorder writes for.
    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    /// Whether payloads are stored in full.
    pub fn redaction(&self) -> Redaction {
        self.redaction
    }

    fn open(&mut self) -> Result<&mut File, RuntimeError> {
        self.file.as_mut().ok_or_else(|| {
            RuntimeError::new(
                RuntimeErrorCode::AdapterError,
                format!("recording {} is already closed", self.path.display()),
            )
        })
    }

    fn open_companion(&mut self) -> Result<&mut File, RuntimeError> {
        let path = companion_path(&self.path);
        self.companion_file.as_mut().ok_or_else(|| {
            RuntimeError::new(
                RuntimeErrorCode::AdapterError,
                format!("replay companion {} is already closed", path.display()),
            )
        })
    }

    /// Apply the host's redaction choice to the payloads section 10.3 names.
    fn redacted(&self, event: Event) -> Event {
        if self.redaction == Redaction::Full {
            return event;
        }
        redact_event(event, self.estimator.as_ref(), &self.agent_capabilities)
    }
}

fn redact_event(
    event: Event,
    estimator: &dyn TokenEstimator,
    agent_capabilities: &BTreeSet<String>,
) -> Event {
    match event {
        Event::Request {
            request_id,
            state: Recorded::Full(state),
            questions,
        } => Event::Request {
            request_id,
            state: Recorded::Redacted {
                redacted: redact(&state, estimator),
            },
            questions,
        },
        Event::Observe {
            capability,
            handle,
            observation: Recorded::Full(observation),
        } => Event::Observe {
            capability,
            handle,
            observation: Recorded::Redacted {
                redacted: redact(&observation, estimator),
            },
        },
        Event::Call {
            capability,
            verb,
            args,
            result: Recorded::Full(result),
            latency_ms,
        } if verb == "wait" && agent_capabilities.contains(&capability) => Event::Call {
            capability,
            verb,
            args,
            result: Recorded::Redacted {
                redacted: redact(&result, estimator),
            },
            latency_ms,
        },
        // A log can carry state or an observation, and nothing records where
        // its values came from, so all of them are redacted (spec 5.8, 10.3).
        Event::Log {
            level,
            message,
            fields,
            task,
            source,
        } => Event::Log {
            level,
            message: redact_recorded(message, estimator),
            fields: fields
                .into_iter()
                .map(|(name, value)| (name, redact_recorded(value, estimator)))
                .collect(),
            task,
            source,
        },
        other => other,
    }
}

fn redact_recorded<T: Serialize>(
    payload: Recorded<T>,
    estimator: &dyn TokenEstimator,
) -> Recorded<T> {
    match payload {
        Recorded::Full(full) => Recorded::Redacted {
            redacted: redact(&full, estimator),
        },
        redacted => redacted,
    }
}

impl Recorder for JsonlRecorder {
    fn write(&mut self, event: Event) -> Result<u64, RuntimeError> {
        if let Event::Start { bindings, .. } = &event {
            self.agent_capabilities = bindings
                .iter()
                .filter(|binding| binding.kind == "agent")
                .map(|binding| binding.name.clone())
                .collect();
        }
        let ts = iso8601_now();
        let execution = matches!(event, Event::Start { .. })
            .then(|| self.execution.clone())
            .flatten();
        let full_line = RecordedEvent {
            ts: ts.clone(),
            run_id: self.run_id.clone(),
            seq: self.seq,
            execution: execution.clone(),
            event: event.clone(),
        };
        let line = RecordedEvent {
            ts,
            run_id: self.run_id.clone(),
            seq: self.seq,
            execution,
            event: self.redacted(event),
        };
        let mut text = serde_json::to_string(&line).map_err(|error| {
            RuntimeError::new(
                RuntimeErrorCode::AdapterError,
                format!("could not serialize a recording event: {error}"),
            )
        })?;
        text.push('\n');
        let path = self.path.clone();
        let file = self.open()?;
        file.write_all(text.as_bytes())
            .map_err(|error| io_error("write", &path, &error))?;
        if self.redaction == Redaction::Redact {
            let mut companion_text = serde_json::to_string(&full_line).map_err(|error| {
                RuntimeError::new(
                    RuntimeErrorCode::AdapterError,
                    format!("could not serialize a replay companion event: {error}"),
                )
            })?;
            companion_text.push('\n');
            let companion_path = companion_path(&self.path);
            self.open_companion()?
                .write_all(companion_text.as_bytes())
                .map_err(|error| io_error("write", &companion_path, &error))?;
        }
        let started_at = self.offset;
        self.offset += text.len() as u64;
        self.seq += 1;
        Ok(started_at)
    }

    fn offset(&self) -> u64 {
        self.offset
    }

    fn close(&mut self) -> Result<(), RuntimeError> {
        if let Some(mut file) = self.file.take() {
            file.flush()
                .map_err(|error| io_error("flush", &self.path, &error))?;
        }
        if let Some(mut file) = self.companion_file.take() {
            let path = companion_path(&self.path);
            file.flush()
                .map_err(|error| io_error("flush", &path, &error))?;
        }
        Ok(())
    }
}

/// The private full companion paired with a redacted primary recording
/// (spec section 10.3).
pub fn companion_path(path: &Path) -> PathBuf {
    let mut full = path.as_os_str().to_os_string();
    full.push(".replay.jsonl");
    PathBuf::from(full)
}

fn create_new_file(path: &Path, private: bool) -> Result<File, RuntimeError> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    if private {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options
        .open(path)
        .map_err(|error| io_error("create", path, &error))?;
    #[cfg(unix)]
    if private {
        use std::os::unix::fs::PermissionsExt;
        if let Err(error) = file.set_permissions(std::fs::Permissions::from_mode(0o600)) {
            drop(file);
            let _ = std::fs::remove_file(path);
            return Err(io_error("secure", path, &error));
        }
    }
    #[cfg(not(unix))]
    let _ = private;
    Ok(file)
}

/// Restore redacted payloads from the private full companion after validating
/// run id, sequence, event identity, hashes and token counts (spec 10.3).
///
/// # Errors
///
/// Returns `replay_diverged` before execution if the companion is missing,
/// truncated, contains redactions, or does not match the primary recording.
pub fn hydrate_recording(
    primary_path: &Path,
    mut primary: Vec<RecordedEvent>,
    estimator: &dyn TokenEstimator,
) -> Result<Vec<RecordedEvent>, RuntimeError> {
    if !primary.iter().any(|line| event_is_redacted(&line.event)) {
        return Ok(primary);
    }
    let path = companion_path(primary_path);
    let mut replayer = JsonlReplayer::new(&path);
    let mut companion = Vec::new();
    while let Some(line) = replayer.next()? {
        companion.push(line);
    }
    if primary.len() != companion.len() {
        return Err(companion_diverged(
            &path,
            format!(
                "has {} events but the primary has {}",
                companion.len(),
                primary.len()
            ),
        ));
    }
    let mut agent_capabilities = BTreeSet::new();
    for (index, (stored, full)) in primary.iter_mut().zip(companion).enumerate() {
        if stored.run_id != full.run_id || stored.seq != full.seq {
            return Err(companion_diverged(
                &path,
                format!("event {index} has a different run id or sequence"),
            ));
        }
        if stored.execution != full.execution || event_is_redacted(&full.event) {
            return Err(companion_diverged(
                &path,
                format!("event {index} is not the matching full event"),
            ));
        }
        if let Event::Start { bindings, .. } = &full.event {
            agent_capabilities = bindings
                .iter()
                .filter(|binding| binding.kind == "agent")
                .map(|binding| binding.name.clone())
                .collect();
        }
        let expected = redact_event(full.event.clone(), estimator, &agent_capabilities);
        if stored.event != expected {
            return Err(companion_diverged(
                &path,
                format!("event {index} fails identity or payload validation"),
            ));
        }
        stored.event = full.event;
    }
    Ok(primary)
}

fn event_is_redacted(event: &Event) -> bool {
    match event {
        Event::Request { state, .. } => state.is_redacted(),
        Event::Call { result, .. } => result.is_redacted(),
        Event::Observe { observation, .. } => observation.is_redacted(),
        Event::Log {
            message, fields, ..
        } => message.is_redacted() || fields.values().any(Recorded::is_redacted),
        _ => false,
    }
}

fn companion_diverged(path: &Path, reason: String) -> RuntimeError {
    RuntimeError::new(
        RuntimeErrorCode::ReplayDiverged,
        format!("replay companion {} {reason}", path.display()),
    )
}

/// A replayer that reads JSONL from a path, one line at a time.
pub struct JsonlReplayer {
    path: PathBuf,
    reader: Option<BufReader<File>>,
    line_number: u64,
    peeked: Option<RecordedEvent>,
}

impl JsonlReplayer {
    /// A replayer that will read `path`.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            reader: None,
            line_number: 0,
            peeked: None,
        }
    }

    /// Where the recording is read from.
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn read_line(&mut self) -> Result<Option<RecordedEvent>, RuntimeError> {
        if self.reader.is_none() {
            let file = File::open(&self.path).map_err(|error| {
                RuntimeError::new(
                    RuntimeErrorCode::ReplayDiverged,
                    format!(
                        "could not open the recording {}: {error}",
                        self.path.display()
                    ),
                )
            })?;
            self.reader = Some(BufReader::new(file));
        }
        let reader = self.reader.as_mut().expect("just opened");
        let mut line = String::new();
        let read = reader.read_line(&mut line).map_err(|error| {
            RuntimeError::new(
                RuntimeErrorCode::ReplayDiverged,
                format!(
                    "could not read the recording {}: {error}",
                    self.path.display()
                ),
            )
        })?;
        if read == 0 {
            return Ok(None);
        }
        self.line_number += 1;
        let text = line.trim_end_matches(['\n', '\r']);
        serde_json::from_str(text).map(Some).map_err(|error| {
            RuntimeError::new(
                RuntimeErrorCode::ReplayDiverged,
                format!(
                    "line {} of the recording {} is not a recorded event: {error}",
                    self.line_number,
                    self.path.display()
                ),
            )
        })
    }
}

impl Replayer for JsonlReplayer {
    fn next(&mut self) -> Result<Option<RecordedEvent>, RuntimeError> {
        if let Some(event) = self.peeked.take() {
            return Ok(Some(event));
        }
        self.read_line()
    }

    fn peek(&mut self) -> Result<Option<&RecordedEvent>, RuntimeError> {
        if self.peeked.is_none() {
            self.peeked = self.read_line()?;
        }
        Ok(self.peeked.as_ref())
    }
}

fn io_error(what: &str, path: &Path, error: &std::io::Error) -> RuntimeError {
    RuntimeError::new(
        RuntimeErrorCode::AdapterError,
        format!("could not {what} the recording {}: {error}", path.display()),
    )
}

/// The system clock as ISO-8601 UTC text with millisecond precision.
///
/// This is the `ts` of a recorded event: metadata for a reader, never a value a
/// program can observe, which is why it may come from the real clock even in
/// replay.
pub fn iso8601_now() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    iso8601_from_unix_millis(now.as_millis() as u64)
}

/// ISO-8601 UTC text for a count of milliseconds since the Unix epoch.
pub fn iso8601_from_unix_millis(millis: u64) -> String {
    let secs = millis / 1000;
    let millis = millis % 1000;
    let days = secs / 86_400;
    let rem = secs % 86_400;
    let (hour, minute, second) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let (year, month, day) = civil_from_days(days as i64);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
}

/// The inverse of [`iso8601_from_unix_millis`]: milliseconds since the Unix
/// epoch for text in exactly that form, which is what the interpreter's clock
/// draws are recorded as (spec section 10.3). The `minutes` budget subtracts
/// one such draw from another, so replay needs the number back, not the text.
///
/// # Errors
///
/// [`RuntimeErrorCode::ReplayDiverged`] if the text is not a clock draw this
/// runtime wrote.
pub fn iso8601_to_unix_millis(text: &str) -> Result<u64, RuntimeError> {
    let bad = || {
        RuntimeError::new(
            RuntimeErrorCode::ReplayDiverged,
            format!("`{text}` is not a recorded clock reading"),
        )
    };
    let digits = |from: usize, to: usize| -> Result<i64, RuntimeError> {
        text.get(from..to)
            .filter(|s| s.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|s| s.parse().ok())
            .ok_or_else(bad)
    };
    if text.len() != 24 || !text.ends_with('Z') {
        return Err(bad());
    }
    let (year, month, day) = (digits(0, 4)?, digits(5, 7)?, digits(8, 10)?);
    let (hour, minute, second, millis) = (
        digits(11, 13)?,
        digits(14, 16)?,
        digits(17, 19)?,
        digits(20, 23)?,
    );
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return Err(bad());
    }
    let days = days_from_civil(year, month as u32, day as u32);
    let secs = days * 86_400 + hour * 3600 + minute * 60 + second;
    u64::try_from(secs * 1000 + millis).map_err(|_| bad())
}

/// Days since 1970-01-01 of a proleptic Gregorian date (Howard Hinnant's
/// `days_from_civil`).
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = if month > 2 { month - 3 } else { month + 9 } as i64;
    let doy = (153 * mp + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Days since 1970-01-01 to a proleptic Gregorian date (Howard Hinnant's
/// `civil_from_days`).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::AgentStatus;
    use crate::pause::{PauseCommon, PauseKind};
    use serde_json::json;

    fn temp_path(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("jevscript-record-{}-{}", std::process::id(), name));
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir.join("run.jsonl")
    }

    fn observation() -> Observation {
        Observation {
            status: AgentStatus::Running,
            last_message: "on it".into(),
            tail: "$ cargo test".into(),
            exit_code: None,
            fields: BTreeMap::new(),
        }
    }

    fn every_event() -> Vec<Event> {
        vec![
            Event::Start {
                program: "fix_issue".into(),
                task: "main".into(),
                inputs: BTreeMap::from([("issue".to_string(), Value::Text("#12".into()))]),
                bindings: vec![BindingRecord {
                    name: "claude".into(),
                    kind: "agent".into(),
                }],
                ir_version: "0.1".into(),
            },
            Event::Request {
                request_id: "r1".into(),
                state: BTreeMap::from([("obs".to_string(), json!({"summary": "done"}))]).into(),
                questions: vec![Question::Noul {
                    id: "claims_done".into(),
                    path: "obs.summary".into(),
                    condition: "states the work is complete".into(),
                    instruction: None,
                }],
            },
            Event::Answers {
                request_id: "r1".into(),
                answers: vec![JevAnswer::Noul {
                    id: "claims_done".into(),
                    prob: 0.9,
                }],
                usage: JevUsage {
                    tokens: 12,
                    usd: Some(0.0),
                },
                latency_ms: 40,
                sampled: true,
            },
            Event::Generate {
                capability: "writer".into(),
                instruction: "summarize".into(),
                using: Some(Value::Text("ctx".into())),
                output: "short".into(),
                usage: JevUsage::default(),
            },
            Event::Call {
                capability: "tree".into(),
                verb: "create".into(),
                args: Value::List(vec![Value::Text("branch".into())]),
                result: Value::Prob(0.5).into(),
                latency_ms: 3,
            },
            Event::Call {
                capability: "claude".into(),
                verb: "wait".into(),
                args: Value::Text("idle".into()),
                result: Value::Record(BTreeMap::from([
                    ("status".into(), Value::Text("exited".into())),
                    (
                        "last_message".into(),
                        Value::Text("private wait result".into()),
                    ),
                ]))
                .into(),
                latency_ms: 4,
            },
            Event::Call {
                capability: "tree".into(),
                verb: "wait".into(),
                args: Value::None,
                result: Value::Text("ordinary tool wait result".into()).into(),
                latency_ms: 2,
            },
            Event::Observe {
                capability: "claude".into(),
                handle: Value::Text("dev".into()),
                observation: observation().into(),
            },
            Event::EffectError {
                operation: EffectOperation::Call,
                identity: EffectIdentity::Call {
                    capability: "tree".into(),
                    verb: "flaky".into(),
                    args: Value::List(Vec::new()),
                },
                code: RuntimeErrorCode::AdapterError,
                message: "flaked".into(),
                retryable: true,
                source: Span::default(),
            },
            Event::Draw {
                kind: DrawKind::Random,
                value: json!(0.25),
            },
            Event::Warning {
                code: "truncated".into(),
                message: "tail cut".into(),
                source: Span::default(),
            },
            Event::Pause {
                kind: PauseKind::Stopped,
                payload: Pause::Stopped {
                    common: PauseCommon {
                        run_id: "run".into(),
                        step: 1,
                        task: "main".into(),
                        state: None,
                        source: Span::default(),
                        recording_offset: 0,
                    },
                    reason: "enough".into(),
                },
            },
            Event::Resume {
                payload: Resume::Continue { resume: true },
            },
            Event::Abort {
                reason: HOST_ABORT_REASON.into(),
            },
            Event::Step {
                record: StepRecord {
                    step: 3,
                    action: "send".into(),
                    target: "dev".into(),
                    args: "Tests fail".into(),
                    changed: true,
                    outcome: StepOutcome::Observed,
                },
            },
            Event::MachineStep {
                machine: "review".into(),
                state: "open".into(),
                enabled: vec!["approve".into(), "stay".into()],
                chosen: "approve".into(),
                probabilities: BTreeMap::from([("approve".to_string(), 0.9)]),
                confidence: 0.8,
                to: "merged".into(),
            },
            Event::End {
                kind: PauseKind::Done,
                outputs: BTreeMap::from([("pr".to_string(), Value::Number(7.0))]),
                verified: true,
                usage: Usage::default(),
            },
        ]
    }

    #[test]
    fn every_event_round_trips_through_a_file_with_exact_offsets() {
        // Spec 10.3: one line per event with ts, run_id and seq; the offset a
        // write returns is where the line starts, which is what a pause
        // reports as `recording_offset`.
        let path = temp_path("roundtrip");
        let _ = std::fs::remove_file(&path);
        let events = every_event();
        let mut recorder =
            JsonlRecorder::new(&path, "run-1", Redaction::Full).expect("creates recorder");
        let mut offsets = Vec::new();
        for event in events.clone() {
            offsets.push(recorder.write(event).expect("writes"));
        }
        let end = recorder.offset();
        recorder.close().expect("closes");

        let bytes = std::fs::read(&path).expect("reads back");
        assert_eq!(end, bytes.len() as u64);
        for (i, offset) in offsets.iter().enumerate() {
            assert!(
                i == 0 || bytes[*offset as usize - 1] == b'\n',
                "line {i} starts after a newline"
            );
            assert_eq!(
                bytes[*offset as usize], b'{',
                "line {i} starts at its offset"
            );
        }

        let mut replayer = JsonlReplayer::new(&path);
        for (i, expected) in events.iter().enumerate() {
            let peeked = replayer
                .peek()
                .expect("peeks")
                .expect("has an event")
                .clone();
            let event = replayer.next().expect("reads").expect("has an event");
            assert_eq!(peeked, event);
            assert_eq!(event.seq, i as u64);
            assert_eq!(event.run_id, "run-1");
            assert!(event.ts.ends_with('Z'), "{}", event.ts);
            assert_eq!(&event.event, expected, "event {i}");
        }
        assert_eq!(replayer.next().expect("reads"), None);
        assert_eq!(replayer.peek().expect("peeks"), None);
    }

    #[test]
    fn redaction_writes_a_private_full_companion_and_hydrates_exactly() {
        // Spec 10.3: the primary hides state, observations and wait results;
        // the private companion restores the exact values for replay.
        let path = temp_path("redact");
        let _ = std::fs::remove_file(&path);
        let companion = companion_path(&path);
        let _ = std::fs::remove_file(&companion);
        let expected = every_event();
        let mut recorder =
            JsonlRecorder::new(&path, "run-2", Redaction::Redact).expect("creates recorder");
        for event in expected.clone() {
            recorder.write(event).expect("writes");
        }
        recorder.close().expect("closes");

        let text = std::fs::read_to_string(&path).expect("reads back");
        assert!(!text.contains("\"summary\""), "state leaked: {text}");
        assert!(!text.contains("cargo test"), "observation leaked: {text}");
        assert!(!text.contains("private wait result"), "wait leaked: {text}");
        assert!(
            text.contains("ordinary tool wait result"),
            "tool result was redacted: {text}"
        );
        assert!(text.contains("\"claims_done\""), "questions stay: {text}");
        let full = std::fs::read_to_string(&companion).expect("reads companion");
        assert!(full.contains("private wait result"), "{full}");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&companion)
                    .expect("companion metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }

        let mut replayer = JsonlReplayer::new(&path);
        let mut primary = Vec::new();
        let mut saw = 0;
        while let Some(event) = replayer.next().expect("reads") {
            match &event.event {
                Event::Request { state, .. } => {
                    let Recorded::Redacted { redacted } = state else {
                        panic!("state was not redacted");
                    };
                    assert_eq!(redacted.hash.len(), 64);
                    assert!(redacted.tokens > 0);
                    saw += 1;
                }
                Event::Call {
                    capability,
                    verb,
                    result,
                    ..
                } if capability == "claude" && verb == "wait" => {
                    assert!(result.is_redacted());
                    saw += 1;
                }
                Event::Call {
                    capability,
                    verb,
                    result,
                    ..
                } if capability == "tree" && verb == "wait" => {
                    assert_eq!(
                        result.full(),
                        Some(&Value::Text("ordinary tool wait result".into()))
                    );
                }
                Event::Observe { observation, .. } => {
                    assert!(observation.is_redacted());
                    saw += 1;
                }
                _ => {}
            }
            primary.push(event);
        }
        assert_eq!(saw, 3);
        let hydrated = hydrate_recording(&path, primary, &CharsPerToken).expect("hydrates");
        assert_eq!(
            hydrated
                .into_iter()
                .map(|line| line.event)
                .collect::<Vec<_>>(),
            expected
        );
    }

    #[test]
    fn recorder_refuses_existing_primary_and_companion_paths_without_changes() {
        // Spec 10.3: both members of a redacted recording are new files; a
        // collision leaves every pre-existing member untouched.
        let primary_exists = temp_path("existing-primary");
        let _ = std::fs::remove_file(&primary_exists);
        let primary_companion = companion_path(&primary_exists);
        let _ = std::fs::remove_file(&primary_companion);
        std::fs::write(&primary_exists, "existing primary").expect("writes fixture");
        let error = JsonlRecorder::new(&primary_exists, "run", Redaction::Redact)
            .err()
            .expect("existing primary is refused");
        assert_eq!(error.code, RuntimeErrorCode::AdapterError);
        assert_eq!(
            std::fs::read_to_string(&primary_exists).expect("reads fixture"),
            "existing primary"
        );
        assert!(!primary_companion.exists());

        let companion_exists = temp_path("existing-companion");
        let _ = std::fs::remove_file(&companion_exists);
        let companion = companion_path(&companion_exists);
        let _ = std::fs::remove_file(&companion);
        std::fs::write(&companion, "existing companion").expect("writes fixture");
        let error = JsonlRecorder::new(&companion_exists, "run", Redaction::Redact)
            .err()
            .expect("existing companion is refused");
        assert_eq!(error.code, RuntimeErrorCode::AdapterError);
        assert!(!companion_exists.exists(), "new primary was rolled back");
        assert_eq!(
            std::fs::read_to_string(&companion).expect("reads fixture"),
            "existing companion"
        );

        let both_exist = temp_path("both-exist");
        let both_companion = companion_path(&both_exist);
        let _ = std::fs::remove_file(&both_exist);
        let _ = std::fs::remove_file(&both_companion);
        std::fs::write(&both_exist, "primary before").expect("writes primary");
        std::fs::write(&both_companion, "companion before").expect("writes companion");
        assert!(JsonlRecorder::new(&both_exist, "run", Redaction::Redact).is_err());
        assert_eq!(
            std::fs::read_to_string(&both_exist).expect("reads primary"),
            "primary before"
        );
        assert_eq!(
            std::fs::read_to_string(&both_companion).expect("reads companion"),
            "companion before"
        );
    }

    #[test]
    fn redacted_replay_rejects_a_missing_or_tampered_companion() {
        // Spec 10.3: either file alone is not the replay artifact, and hashes
        // and event identity are checked before execution.
        let path = temp_path("companion-validation");
        let _ = std::fs::remove_file(&path);
        let companion = companion_path(&path);
        let _ = std::fs::remove_file(&companion);
        let mut recorder =
            JsonlRecorder::new(&path, "run-4", Redaction::Redact).expect("creates recorder");
        for event in every_event() {
            recorder.write(event).expect("writes");
        }
        recorder.close().expect("closes");

        let read_primary = || {
            let mut reader = JsonlReplayer::new(&path);
            let mut lines = Vec::new();
            while let Some(line) = reader.next().expect("reads primary") {
                lines.push(line);
            }
            lines
        };
        let full = std::fs::read_to_string(&companion).expect("reads companion");
        std::fs::remove_file(&companion).expect("removes companion");
        let missing = hydrate_recording(&path, read_primary(), &CharsPerToken)
            .expect_err("missing companion diverges");
        assert_eq!(missing.code, RuntimeErrorCode::ReplayDiverged);

        std::fs::write(
            &companion,
            full.replace("private wait result", "different wait result"),
        )
        .expect("tampers companion");
        let tampered = hydrate_recording(&path, read_primary(), &CharsPerToken)
            .expect_err("tampered companion diverges");
        assert_eq!(tampered.code, RuntimeErrorCode::ReplayDiverged);
        assert!(
            tampered.message.contains("validation"),
            "{}",
            tampered.message
        );
    }

    #[test]
    fn recorded_payloads_escape_both_metadata_collisions_losslessly() {
        // Spec 10.3: legal state and call data that resembles recording
        // metadata is full data, not a redaction marker or an escape marker.
        let state = BTreeMap::from([(
            "redacted".to_string(),
            json!({"hash": "user data", "tokens": 7}),
        )]);
        let recorded_state = Recorded::Full(state.clone());
        let encoded = serde_json::to_value(&recorded_state).expect("serializes state");
        assert_eq!(encoded["$recorded_full"], json!(state));
        let decoded: Recorded<BTreeMap<String, serde_json::Value>> =
            serde_json::from_value(encoded).expect("deserializes state");
        assert_eq!(decoded, recorded_state);

        let result = Value::Record(BTreeMap::from([(
            "$recorded_full".to_string(),
            Value::Text("ordinary result".into()),
        )]));
        let call = Event::Call {
            capability: "tool".into(),
            verb: "read".into(),
            args: Value::None,
            result: result.clone().into(),
            latency_ms: 0,
        };
        let encoded = serde_json::to_value(&call).expect("serializes call");
        assert_eq!(encoded["result"]["$recorded_full"], json!(result));
        let decoded: Event = serde_json::from_value(encoded).expect("deserializes call");
        assert_eq!(decoded, call);
    }

    #[test]
    fn redacted_objects_with_extra_keys_are_ordinary_full_data() {
        // Spec 10.3: only the exact redaction metadata object is reserved.
        let state = BTreeMap::from([
            (
                "redacted".to_string(),
                json!({"hash": "user data", "tokens": 7, "note": "not metadata"}),
            ),
            ("other".to_string(), json!(true)),
        ]);
        let recorded = Recorded::Full(state);
        let encoded = serde_json::to_value(&recorded).expect("serializes");
        assert!(encoded.get("$recorded_full").is_none(), "{encoded}");
        let decoded: Recorded<BTreeMap<String, serde_json::Value>> =
            serde_json::from_value(encoded).expect("deserializes");
        assert_eq!(decoded, recorded);
    }

    #[test]
    fn a_truncated_last_line_is_replay_diverged_naming_the_line() {
        let path = temp_path("truncated");
        let _ = std::fs::remove_file(&path);
        let mut recorder =
            JsonlRecorder::new(&path, "run-3", Redaction::Full).expect("creates recorder");
        recorder.write(every_event().remove(0)).expect("writes");
        recorder.close().expect("closes");
        let mut bytes = std::fs::read(&path).expect("reads");
        bytes.truncate(bytes.len() - 10);
        bytes.extend_from_slice(b"\n");
        std::fs::write(&path, &bytes).expect("rewrites");

        let error = JsonlReplayer::new(&path).next().expect_err("rejects");
        assert_eq!(error.code, RuntimeErrorCode::ReplayDiverged);
        assert!(error.message.contains("line 1"), "{}", error.message);
    }

    #[test]
    fn a_missing_recording_is_replay_diverged() {
        let error = JsonlReplayer::new("/nonexistent/run.jsonl")
            .next()
            .expect_err("rejects");
        assert_eq!(error.code, RuntimeErrorCode::ReplayDiverged);
    }

    #[test]
    fn the_timestamp_is_iso8601_utc() {
        assert_eq!(iso8601_from_unix_millis(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(
            iso8601_from_unix_millis(1_790_000_000_123),
            "2026-09-21T14:13:20.123Z"
        );
        assert_eq!(
            iso8601_from_unix_millis(951_782_400_000),
            "2000-02-29T00:00:00.000Z"
        );
    }

    #[test]
    fn a_clock_reading_round_trips_through_its_text() {
        // Spec 10.3: the clock a program observes is a `draw`, and the
        // `minutes` budget needs the number back from the recorded text.
        for millis in [0, 1_790_000_000_123, 951_782_400_000, 4_102_444_800_999] {
            let text = iso8601_from_unix_millis(millis);
            assert_eq!(
                iso8601_to_unix_millis(&text).expect("parses"),
                millis,
                "{text}"
            );
        }
        assert_eq!(
            iso8601_to_unix_millis("2026-09-21")
                .expect_err("rejects")
                .code,
            RuntimeErrorCode::ReplayDiverged
        );
        assert!(iso8601_to_unix_millis("2026-13-21T00:00:00.000Z").is_err());
    }

    #[test]
    fn redaction_is_a_sha256_of_the_canonical_json() {
        // `printf '%s' '{"a":1}' | shasum -a 256`
        let redacted = redact(&json!({"a": 1}), &CharsPerToken);
        assert_eq!(
            redacted.hash,
            "015abd7f5cc57a2dd94b7590f04ad8084273905ee33ec5cebeae62276a97f862"
        );
        assert_eq!(redacted.tokens, CharsPerToken.estimate(r#"{"a":1}"#));
    }

    #[test]
    fn recording_json_round_trips_usage_floats_bit_exactly() {
        // Spec 10.4: terminal usage is part of exact replay identity. This
        // value previously changed by one bit through serde_json without its
        // `float_roundtrip` feature.
        let live_minutes = 7.0_f64 / 60_000.0;
        assert_eq!(live_minutes.to_bits(), 0x3f1e_955e_124b_a3b4);
        let text = serde_json::to_string(&live_minutes).expect("serializes usage");
        let replayed: f64 = serde_json::from_str(&text).expect("deserializes usage");
        assert_eq!(replayed.to_bits(), live_minutes.to_bits(), "{text}");
    }
}
