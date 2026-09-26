//! Runs: the state machine a host steps (spec section 10.1).
//!
//! ```text
//! created -> running -> paused(kind) -> running -> ... -> ended(kind)
//! ```
//!
//! `running` is transient and a host never observes it. The host observes
//! pauses: it calls [`Run::next`] to advance until the next one, [`Run::resume`]
//! to answer it, and [`Run::abort`] to give up. A run is single-threaded; a host
//! that wants parallelism starts several runs (spec section 10.5).
//!
//! A run owns its recording as an in-memory log of events. Each `next` walks
//! the task from the start with that log served back, and goes live when the
//! log runs out, so answering a pause and replaying a recording are the same
//! motion (see [`crate::interp`]). The recorder, when there is one, sees each
//! live event exactly once. A replay source never falls through to live
//! effects merely because its log is exhausted (spec section 10.4).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::{Arc, atomic::AtomicBool};

use jevscript_ir::{IR_VERSION, Ir};

use crate::capability::Bindings;
use crate::error::{RunError, RuntimeError, RuntimeErrorCode};
use crate::interp::{
    Interpreter,
    driver::{Driver, DriverSetup},
    verbs_referenced,
};
use crate::jev::JevClient;
use crate::pause::{Pause, PauseKind, Resume, Usage};
use crate::profile::{DEFAULT_MODEL, Profile, Profiles};
use crate::record::{
    Event, EventSink, ExecutionMetadata, JsonlRecorder, JsonlReplayer, RecordedEvent, Redaction,
    Replayer, hydrate_recording,
};
use crate::rng::Rng;
use crate::rpc::Sample;
use crate::tokens::estimator_for;
use crate::value::{Handle, Value};

/// Where a run is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunState {
    /// Created but not yet stepped.
    Created,
    /// Advancing. Transient: a host never observes this.
    Running,
    /// Waiting on the host.
    Paused(PauseKind),
    /// Over.
    Ended(PauseKind),
}

/// A thread-safe request to cancel a running host callback (spec section 9.6).
///
/// Calling [`AbortHandle::abort`] only sets the request. The synchronous run
/// records and settles it immediately after the callback returns.
#[derive(Clone, Debug)]
pub struct AbortHandle {
    signal: Arc<AtomicBool>,
}

impl AbortHandle {
    /// Request host cancellation at the next interpreter boundary.
    pub fn abort(&self) {
        self.signal
            .store(true, std::sync::atomic::Ordering::Release);
    }
}

impl RunState {
    /// Whether the run is over.
    pub const fn is_ended(&self) -> bool {
        matches!(self, RunState::Ended(_))
    }

    /// The pause the run is waiting on, if it is paused.
    pub const fn paused_on(&self) -> Option<PauseKind> {
        match self {
            RunState::Paused(kind) => Some(*kind),
            _ => None,
        }
    }
}

/// What a run is started with (spec section 11.2).
#[derive(Debug, Clone, Default)]
pub struct RunOptions {
    /// The inputs, matching the program's `in` declarations.
    pub inputs: BTreeMap<String, Value>,
    /// Where to write a recording.
    pub record: Option<std::path::PathBuf>,
    /// A recording to replay instead of calling anything.
    pub replay: Option<std::path::PathBuf>,
    /// Whether payloads are stored in full.
    pub redaction: Redaction,
    /// A Jev model id, overriding the runtime's default. It also selects the
    /// model profile (spec section 10.6).
    pub model: Option<String>,
    /// Draw labels and levels from Jev's distribution instead of taking the
    /// argmax (spec section 6.11). Every draw goes through the run's recorded
    /// random source, so a sampled run replays exactly.
    pub sample: Option<Sample>,
    /// A profiles file to layer over the bundled ones (spec section 10.6).
    pub profiles: Option<std::path::PathBuf>,
    /// Module search roots for `use` paths that are not relative (spec 3.9).
    pub paths: Vec<std::path::PathBuf>,
}

impl RunOptions {
    /// Whether the `sample` option turns sampling on for every `pick` and
    /// `rate` (spec section 6.11).
    pub fn samples(&self) -> bool {
        matches!(
            self.sample,
            Some(Sample::On(true)) | Some(Sample::Seeded { .. })
        )
    }
}

/// One run of one task.
///
/// Budgets nest: when a task calls another, the callee's effective limit for
/// each key is the smaller of its own and the caller's remainder, and usage
/// inside the callee counts against every task on the stack (spec section
/// 7.1). A callee can tighten a budget and can never widen it.
pub struct Run {
    id: String,
    task: String,
    ir: Ir,
    state: RunState,
    step: u64,
    current: Option<Pause>,
    usage: Usage,
    options: RunOptions,
    profile: Profile,
    driver: Driver<'static>,
    waiting: Option<(String, Handle)>,
}

impl fmt::Debug for Run {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Run")
            .field("id", &self.id)
            .field("task", &self.task)
            .field("state", &self.state)
            .field("step", &self.step)
            .finish_non_exhaustive()
    }
}

impl Run {
    /// Create a run of `task` from a compiled program, with the adapters the
    /// host bound.
    ///
    /// Every `needs` the program declares must be bound, or the run fails to
    /// start with [`RuntimeErrorCode::BindingMissing`] (spec section 3.4),
    /// unless the run replays a recording, which needs no adapters until the
    /// recording runs out (spec section 10.4). A `tool` adapter with a
    /// manifest is checked against every verb the program calls on it, and a
    /// missing one fails the start with [`RuntimeErrorCode::VerbMissing`]
    /// (spec section 9.4).
    ///
    /// Jev is reached through the client given with [`Run::with_client`], or
    /// through an HTTP client for the profile's endpoint when none was.
    ///
    /// # Errors
    ///
    /// [`RuntimeErrorCode::BindingMissing`] or [`RuntimeErrorCode::VerbMissing`]
    /// as above, [`RuntimeErrorCode::TypeError`] if the program has no such
    /// task, [`RuntimeErrorCode::ProfileMissing`] if the model has no profile,
    /// and [`RuntimeErrorCode::ReplayDiverged`] if a recording to replay cannot
    /// be read.
    pub fn create(
        ir: Ir,
        task: &str,
        options: RunOptions,
        bindings: Bindings,
    ) -> Result<Self, RuntimeError> {
        let bound: BTreeSet<&str> = bindings.keys().map(String::as_str).collect();
        Self::check_start(&ir, task, &options, &bound)?;
        let wanted = verbs_referenced(&ir);
        for (name, adapter) in &bindings {
            if let Some(manifest) = adapter.manifest() {
                let verbs = wanted.get(name).cloned().unwrap_or_default();
                let missing = manifest.missing(verbs.iter().map(String::as_str));
                if !missing.is_empty() {
                    let span = ir
                        .needs
                        .iter()
                        .find(|n| n.name == *name)
                        .map(|n| n.span)
                        .unwrap_or_default();
                    return Err(RuntimeError::new(
                        RuntimeErrorCode::VerbMissing,
                        format!(
                            "the adapter bound to `{name}` does not declare {}",
                            missing
                                .iter()
                                .map(|v| format!("`{v}`"))
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                    )
                    .at(span));
                }
            }
        }
        Self::build(ir, task, options, bindings, false)
    }

    /// The same as [`Run::create`], but taking only the names of the bound
    /// capabilities and no adapters. This is what the JSON-RPC server uses:
    /// `task.start` carries `bindings: [{name, kind}]`, and the adapters
    /// themselves stay in the host process (spec section 11.5). A run built
    /// this way can replay; a live capability call on it is `binding_missing`
    /// until a bridge to the host is bound.
    ///
    /// # Errors
    ///
    /// The same as [`Run::create`].
    pub fn create_with_bound_names(
        ir: Ir,
        task: &str,
        options: RunOptions,
        bound: &BTreeSet<&str>,
    ) -> Result<Self, RuntimeError> {
        Self::check_start(&ir, task, &options, bound)?;
        Self::build(ir, task, options, Bindings::new(), false)
    }

    /// Replay a self-contained recording without source files, ambient
    /// profiles, model calls or adapters (spec sections 10.3, 10.4 and 11.6).
    ///
    /// # Errors
    ///
    /// [`RuntimeErrorCode::ReplayDiverged`] if the recording is unreadable or
    /// lacks the linked IR, resolved profile or effective sample option added
    /// to the `start` event by spec section 10.3.
    pub fn from_recording(path: impl Into<std::path::PathBuf>) -> Result<Self, RuntimeError> {
        let options = RunOptions {
            replay: Some(path.into()),
            ..RunOptions::default()
        };
        Self::build(Ir::empty("replay"), "", options, Bindings::new(), true)
    }

    fn check_start(
        ir: &Ir,
        task: &str,
        options: &RunOptions,
        bound: &BTreeSet<&str>,
    ) -> Result<(), RuntimeError> {
        if options.record.is_some() && options.replay.is_some() {
            return Err(RuntimeError::new(
                RuntimeErrorCode::TypeError,
                "the `record` and `replay` options are mutually exclusive",
            ));
        }
        if ir.task(task).is_none() {
            return Err(RuntimeError::new(
                RuntimeErrorCode::TypeError,
                format!("`{}` declares no task `{task}`", ir.program),
            ));
        }
        if options.replay.is_some() {
            return Ok(());
        }
        for need in &ir.needs {
            if !bound.contains(need.name.as_str()) {
                return Err(RuntimeError::new(
                    RuntimeErrorCode::BindingMissing,
                    format!("capability `{}` was not bound", need.name),
                )
                .at(need.span));
            }
        }
        Ok(())
    }

    fn build(
        mut ir: Ir,
        task: &str,
        mut options: RunOptions,
        bindings: Bindings,
        require_execution: bool,
    ) -> Result<Self, RuntimeError> {
        let mut id = next_run_id();
        let mut recorded_lines = Vec::new();
        let mut recorded_task = None;
        let mut recorded_execution = None;
        let replaying = options.replay.is_some();
        if let Some(path) = &options.replay {
            let mut replayer = JsonlReplayer::new(path);
            while let Some(line) = replayer.next()? {
                if let Event::Start {
                    inputs,
                    task: recorded,
                    ..
                } = &line.event
                {
                    // Spec 11.6: `jevscript replay <recording>` has only the
                    // recording, so the inputs come from it when the host
                    // gave none. The replayed run keeps the recorded id so
                    // that its pauses are identical to the first time.
                    if options.inputs.is_empty() {
                        options.inputs = inputs.clone();
                    }
                    recorded_task = Some(recorded.clone());
                    recorded_execution = line.execution.clone();
                    id = line.run_id.clone();
                }
                recorded_lines.push(line);
            }
            validate_recording_envelope(&recorded_lines)?;
        }
        if require_execution && recorded_execution.is_none() {
            return Err(RuntimeError::new(
                RuntimeErrorCode::ReplayDiverged,
                "the recording start event lacks linked IR, resolved profile or sample metadata",
            ));
        }
        let task = match recorded_task {
            Some(recorded) if task.is_empty() || recorded == task => recorded,
            Some(recorded) => {
                return Err(RuntimeError::new(
                    RuntimeErrorCode::ReplayDiverged,
                    format!("the recording is of task `{recorded}`, not `{task}`"),
                ));
            }
            None if replaying => {
                return Err(RuntimeError::new(
                    RuntimeErrorCode::ReplayDiverged,
                    "the recording has no start event",
                ));
            }
            None => task.to_string(),
        };

        let profile = if let Some(execution) = recorded_execution {
            if execution.ir.ir_version != ir.ir_version && !require_execution {
                return Err(RuntimeError::new(
                    RuntimeErrorCode::ReplayDiverged,
                    format!(
                        "the recording IR version `{}` does not match the supplied program version `{}`",
                        execution.ir.ir_version, ir.ir_version
                    ),
                ));
            }
            if require_execution {
                ir = execution.ir;
            }
            options.sample = Some(execution.sample);
            options.model = Some(execution.profile.model.clone());
            execution.profile
        } else {
            let mut profiles = Profiles::from_env()?;
            if let Some(path) = &options.profiles {
                profiles.overlay_file(path)?;
            }
            let model = options
                .model
                .clone()
                .unwrap_or_else(|| DEFAULT_MODEL.to_string());
            profiles.resolve(&model)?.clone()
        };
        let execution = ExecutionMetadata {
            ir: ir.clone(),
            profile: profile.clone(),
            sample: options.sample.unwrap_or(Sample::On(false)),
        };
        let log = if let Some(path) = &options.replay {
            hydrate_recording(
                path,
                recorded_lines,
                estimator_for(&profile.tokenizer).as_ref(),
            )?
            .into_iter()
            .map(|line| line.event)
            .collect()
        } else {
            Vec::new()
        };
        let recorder = options
            .record
            .as_ref()
            .map(|path| {
                JsonlRecorder::new(path, id.clone(), options.redaction).map(|recorder| {
                    Box::new(
                        recorder
                            .with_estimator(estimator_for(&profile.tokenizer))
                            .with_execution(execution.clone()),
                    ) as Box<dyn crate::record::Recorder>
                })
            })
            .transpose()?;
        let abort_signal = Arc::new(AtomicBool::new(false));
        let driver = Driver::new(DriverSetup {
            run_id: id.clone(),
            log,
            replaying,
            recorder,
            bindings,
            profile: profile.clone(),
            rng: Rng::for_sample(options.sample.as_ref()),
            execution,
            abort_signal: Arc::clone(&abort_signal),
        });
        Ok(Self {
            id,
            task,
            ir,
            state: RunState::Created,
            step: 0,
            current: None,
            usage: Usage::default(),
            options,
            profile,
            driver,
            waiting: None,
        })
    }

    /// Ask Jev through `client` instead of the profile's HTTP endpoint.
    #[must_use]
    pub fn with_client(mut self, client: Box<dyn JevClient>) -> Self {
        self.driver.set_client(client);
        self
    }

    /// This run's id, which every pause and recording event carries.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Which task is running.
    pub fn task(&self) -> &str {
        &self.task
    }

    /// The compiled program.
    pub fn ir(&self) -> &Ir {
        &self.ir
    }

    /// What the run was started with.
    pub fn options(&self) -> &RunOptions {
        &self.options
    }

    /// The model profile the run selected (spec section 10.6).
    pub fn profile(&self) -> &Profile {
        &self.profile
    }

    /// Where the run is.
    pub fn state(&self) -> &RunState {
        &self.state
    }

    /// How many steps have been taken: iterations of the task's outermost
    /// loop, which is also what the `steps` budget counts (spec section 7.1).
    pub fn step(&self) -> u64 {
        self.step
    }

    /// What the run has cost so far.
    pub fn usage(&self) -> &Usage {
        &self.usage
    }

    /// The pause the run is waiting on, if it is paused.
    pub fn current_pause(&self) -> Option<&Pause> {
        self.current.as_ref()
    }

    /// The recording so far, as events in order. In a replay this starts as
    /// the recording that was handed in.
    pub fn log(&self) -> &[Event] {
        self.driver.log()
    }

    /// How many leading events of [`Self::log`] the run has executed so far.
    ///
    /// In a replay the log is the whole recording, and only this prefix has
    /// passed its identity checks (spec section 10.4); after a divergence the
    /// log is cut at the mismatch and continues with what was written live.
    pub fn replay_position(&self) -> usize {
        self.driver.cursor()
    }

    /// The events written since the last call, for `Run.events()` (spec
    /// section 11.2). Every live event is returned exactly once.
    pub fn drain_events(&mut self) -> Vec<RecordedEvent> {
        self.driver.drain_outbox()
    }

    /// Stream each newly recorded event immediately, including capability
    /// calls before the next pause is returned (spec sections 9.6 and 11.5).
    pub fn set_event_sink(&mut self, sink: EventSink) {
        self.driver.set_event_sink(sink);
    }

    /// Return a thread-safe handle that can request cancellation while a
    /// synchronous host callback is in flight (spec sections 9.6 and 10.3).
    pub fn abort_handle(&self) -> AbortHandle {
        AbortHandle {
            signal: self.driver.abort_signal(),
        }
    }

    /// Advance until the next pause.
    ///
    /// On a `waiting` pause nothing is needed first: the pause auto-resumes
    /// (spec section 10.2). On any other open pause the host must have called
    /// [`Run::resume`], except in a replay, where the recorded answer is
    /// applied unless the host answered differently (spec section 10.4).
    ///
    /// # Errors
    ///
    /// [`RunError::Ended`] if the run is over, and [`RunError::WrongPayload`]
    /// if it is waiting on an answer the host has not given.
    #[allow(
        clippy::should_implement_trait,
        reason = "`Run.next()` is the name the spec's SDK surface uses (section 11.2)"
    )]
    pub fn next(&mut self) -> Result<Pause, RunError> {
        match &self.state {
            RunState::Ended(kind) => return Err(RunError::Ended(kind.as_str())),
            RunState::Paused(kind) => {
                if !self.driver.can_advance_pause() {
                    if *kind == PauseKind::Waiting {
                        self.driver.resume(Resume::Nothing {})?;
                    } else {
                        return Err(RunError::WrongPayload {
                            pause: kind.as_str(),
                            reason: "the run is waiting to be resumed".to_string(),
                        });
                    }
                }
            }
            RunState::Created | RunState::Running => {}
        }
        Ok(self.advance())
    }

    fn advance(&mut self) -> Pause {
        self.state = RunState::Running;
        self.current = None;
        let interpreter = Interpreter::new(
            &self.ir,
            &self.profile,
            &mut self.driver,
            self.id.clone(),
            self.task.clone(),
            self.options.inputs.clone(),
            self.options.samples(),
        );
        let outcome = interpreter.run();
        self.usage = outcome.usage;
        self.waiting = outcome.waiting;
        self.pause_with(outcome.pause)
    }

    /// Answer the open pause and let the run continue.
    ///
    /// The payload must fit the pause it answers (spec section 10.2):
    /// `confirm` takes an answer, `escalate` takes `{ resume: true }`, `budget`
    /// takes `{ extend: { key: n } }`, a retryable `error` takes
    /// `{ retry: true }`, and `waiting` takes nothing.
    ///
    /// # Errors
    ///
    /// [`RunError::Ended`] if the run is over, [`RunError::NotPaused`] if it is
    /// not waiting, and [`RunError::WrongPayload`] if the payload does not fit.
    pub fn resume(&mut self, payload: Resume) -> Result<(), RunError> {
        let kind = match &self.state {
            RunState::Ended(kind) => return Err(RunError::Ended(kind.as_str())),
            RunState::Paused(kind) => *kind,
            RunState::Created | RunState::Running => return Err(RunError::NotPaused),
        };

        let retryable = matches!(
            self.current,
            Some(Pause::Error {
                retryable: true,
                ..
            })
        );
        let fits = match (kind, &payload) {
            (PauseKind::Confirm, Resume::Answer { .. }) => true,
            (PauseKind::Escalate, Resume::Continue { resume: true }) => true,
            (PauseKind::Budget, Resume::Extend { extend }) => !extend.is_empty(),
            (PauseKind::Error, Resume::Retry { retry: true }) => retryable,
            (PauseKind::Waiting, Resume::Nothing {}) => true,
            _ => false,
        };
        if !fits {
            return Err(RunError::WrongPayload {
                pause: kind.as_str(),
                reason: match kind {
                    PauseKind::Confirm => "expected `{ answer, text? }`".to_string(),
                    PauseKind::Escalate => "expected `{ resume: true }`".to_string(),
                    PauseKind::Budget => "expected `{ extend: { key: n } }`".to_string(),
                    PauseKind::Error if retryable => "expected `{ retry: true }`".to_string(),
                    PauseKind::Error => "this error is not retryable".to_string(),
                    PauseKind::Waiting => "a waiting pause takes nothing".to_string(),
                    PauseKind::Stopped | PauseKind::Done => "this pause is terminal".to_string(),
                },
            });
        }

        self.driver.resume(payload)?;
        self.current = None;
        self.state = RunState::Running;
        Ok(())
    }

    /// Send a message to a capability while the run is `waiting` (spec 10.2).
    /// The message reaches the adapter as `send` on the handle being waited
    /// on, now, and is recorded as a `call` event.
    ///
    /// # Errors
    ///
    /// [`RunError::Ended`] if the run is over, [`RunError::NotPaused`] if it
    /// is not waiting on that capability, and the adapter's own error.
    pub fn inject(&mut self, capability: &str, message: &str) -> Result<(), RunError> {
        match (&self.state, &self.current, &self.waiting) {
            (RunState::Ended(kind), _, _) => Err(RunError::Ended(kind.as_str())),
            (
                RunState::Paused(PauseKind::Waiting),
                Some(Pause::Waiting { on, .. }),
                Some((waited, handle)),
            ) if on == capability && waited == capability => {
                let handle = handle.clone();
                self.driver.inject(capability, &handle, message)?;
                Ok(())
            }
            _ => Err(RunError::NotPaused),
        }
    }

    /// Give up on the run. It ends as `stopped`.
    ///
    /// # Errors
    ///
    /// [`RunError::Ended`] if the run is already over.
    pub fn abort(&mut self) -> Result<(), RunError> {
        match &self.state {
            RunState::Ended(kind) => Err(RunError::Ended(kind.as_str())),
            RunState::Running => {
                self.driver.request_abort();
                Ok(())
            }
            RunState::Created => {
                self.driver.prepare_created_abort();
                self.advance();
                Ok(())
            }
            RunState::Paused(_) => {
                self.driver.prepare_paused_abort();
                self.advance();
                Ok(())
            }
        }
    }

    /// Record that the run has reached `pause`, and end it if the pause is
    /// terminal. This is the one place the state machine moves out of
    /// `running`.
    pub(crate) fn pause_with(&mut self, pause: Pause) -> Pause {
        let kind = pause.kind();
        self.step = pause.common().step;
        self.state = if pause.is_terminal() {
            RunState::Ended(kind)
        } else {
            RunState::Paused(kind)
        };
        self.current = Some(pause.clone());
        pause
    }
}

fn validate_recording_envelope(lines: &[RecordedEvent]) -> Result<(), RuntimeError> {
    let Some(first) = lines.first() else {
        return Err(RuntimeError::new(
            RuntimeErrorCode::ReplayDiverged,
            "the recording has no leading start event",
        ));
    };
    let Event::Start { ir_version, .. } = &first.event else {
        return Err(RuntimeError::new(
            RuntimeErrorCode::ReplayDiverged,
            "the recording does not begin with a start event",
        ));
    };
    for (index, line) in lines.iter().enumerate() {
        if line.run_id != first.run_id {
            return Err(RuntimeError::new(
                RuntimeErrorCode::ReplayDiverged,
                format!("event {index} has a different run id from the leading start event"),
            ));
        }
        if line.seq != index as u64 {
            return Err(RuntimeError::new(
                RuntimeErrorCode::ReplayDiverged,
                format!("event {index} has sequence {} instead of {index}", line.seq),
            ));
        }
        if index != 0 && matches!(line.event, Event::Start { .. }) {
            return Err(RuntimeError::new(
                RuntimeErrorCode::ReplayDiverged,
                format!("event {index} is a second start event"),
            ));
        }
        if index != 0 && line.execution.is_some() {
            return Err(RuntimeError::new(
                RuntimeErrorCode::ReplayDiverged,
                format!("event {index} carries execution metadata outside the start event"),
            ));
        }
    }
    if ir_version != IR_VERSION {
        return Err(RuntimeError::new(
            RuntimeErrorCode::ReplayDiverged,
            format!(
                "the recording IR version `{ir_version}` is not supported by runtime version `{IR_VERSION}`"
            ),
        ));
    }
    if let Some(execution) = &first.execution {
        if execution.ir.ir_version != *ir_version {
            return Err(RuntimeError::new(
                RuntimeErrorCode::ReplayDiverged,
                format!(
                    "the recording start IR version `{ir_version}` does not match the embedded IR version `{}`",
                    execution.ir.ir_version
                ),
            ));
        }
        if execution.ir.ir_version != IR_VERSION {
            return Err(RuntimeError::new(
                RuntimeErrorCode::ReplayDiverged,
                format!(
                    "the embedded IR version `{}` is not supported by runtime version `{IR_VERSION}`",
                    execution.ir.ir_version
                ),
            ));
        }
    }
    Ok(())
}

/// A fresh run id. Run ids are host-facing handles, not values a program can
/// observe, so a counter is enough and replay never needs to reproduce one:
/// a replayed run keeps the id of its recording.
fn next_run_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    format!(
        "run_{}_{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use jevscript_ir::{Budget, Span, Task, Thresholds};
    use jevscript_syntax::Span as SyntaxSpan;

    use super::*;
    use crate::error::RuntimeErrorCode;
    use crate::pause::{PauseCommon, Usage};

    fn program_with_main() -> Ir {
        let mut ir = Ir::empty("fix_issue");
        ir.tasks.push(Task {
            name: "main".to_string(),
            params: Vec::new(),
            budget: Budget {
                calls: Some(40.0),
                ..Budget::default()
            },
            thresholds: Thresholds::default(),
            body: Vec::new(),
            span: Span::default(),
        });
        ir
    }

    fn run() -> Run {
        Run::create_with_bound_names(
            program_with_main(),
            "main",
            RunOptions::default(),
            &BTreeSet::new(),
        )
        .expect("a program with no `needs` starts with no bindings")
    }

    fn common() -> PauseCommon {
        PauseCommon {
            run_id: "run_1".to_string(),
            step: 3,
            task: "main".to_string(),
            state: None,
            source: SyntaxSpan::default(),
            recording_offset: 0,
        }
    }

    fn recording_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("jevscript-run-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("creates temp dir");
        let path = dir.join("run.jsonl");
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(crate::record::companion_path(&path));
        path
    }

    fn empty_recording(name: &str) -> PathBuf {
        let path = recording_path(name);
        let mut run = Run::create_with_bound_names(
            program_with_main(),
            "main",
            RunOptions {
                record: Some(path.clone()),
                ..RunOptions::default()
            },
            &BTreeSet::new(),
        )
        .expect("starts recording");
        assert_eq!(run.next().expect("finishes").kind(), PauseKind::Done);
        path
    }

    fn recording_json(path: &std::path::Path) -> Vec<serde_json::Value> {
        std::fs::read_to_string(path)
            .expect("reads recording")
            .lines()
            .map(|line| serde_json::from_str(line).expect("valid event"))
            .collect()
    }

    fn write_recording_json(path: &std::path::Path, lines: &[serde_json::Value]) {
        let text = lines
            .iter()
            .map(|line| serde_json::to_string(line).expect("serializes event"))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        std::fs::write(path, text).expect("writes recording");
    }

    #[test]
    fn a_new_run_is_created() {
        assert_eq!(*run().state(), RunState::Created);
    }

    #[test]
    fn an_unbound_capability_fails_the_start() {
        let mut ir = program_with_main();
        ir.needs.push(jevscript_ir::Need {
            name: "claude".to_string(),
            kind: jevscript_ir::CapabilityKind::Agent,
            signatures: Vec::new(),
            span: Span::default(),
        });
        let error =
            Run::create_with_bound_names(ir, "main", RunOptions::default(), &BTreeSet::new())
                .expect_err("an unbound capability is an error");
        assert_eq!(error.code, RuntimeErrorCode::BindingMissing);
    }

    #[test]
    fn a_missing_task_fails_the_start() {
        let error = Run::create_with_bound_names(
            program_with_main(),
            "nope",
            RunOptions::default(),
            &BTreeSet::new(),
        )
        .expect_err("a missing task is an error");
        assert_eq!(error.code, RuntimeErrorCode::TypeError);
    }

    #[test]
    fn record_and_replay_are_mutually_exclusive_before_file_setup() {
        // Spec 10.1: record and replay together are a type_error before
        // execution, and no destination is created as a side effect.
        let record = recording_path("record-replay-destination");
        let replay = recording_path("record-replay-source");
        let error = Run::create_with_bound_names(
            program_with_main(),
            "main",
            RunOptions {
                record: Some(record.clone()),
                replay: Some(replay),
                ..RunOptions::default()
            },
            &BTreeSet::new(),
        )
        .expect_err("both options are refused");
        assert_eq!(error.code, RuntimeErrorCode::TypeError);
        assert!(!record.exists());
    }

    #[test]
    fn recording_setup_refuses_an_existing_destination_before_execution() {
        // Spec 10.3: reserving the file is part of run setup, before the
        // interpreter can reach a Jev or capability effect.
        let record = recording_path("existing-destination");
        std::fs::write(&record, "pre-existing bytes").expect("writes fixture");
        let error = Run::create_with_bound_names(
            program_with_main(),
            "main",
            RunOptions {
                record: Some(record.clone()),
                ..RunOptions::default()
            },
            &BTreeSet::new(),
        )
        .expect_err("existing destination is refused during setup");
        assert_eq!(error.code, RuntimeErrorCode::AdapterError);
        assert_eq!(
            std::fs::read_to_string(record).expect("reads fixture"),
            "pre-existing bytes"
        );
    }

    #[test]
    fn replay_rejects_unsupported_and_mismatched_ir_versions_before_execution() {
        // Spec 10.3: both the start version and embedded IR version must agree
        // with each other and with the version this runtime supports.
        let mismatched = empty_recording("mismatched-ir");
        let mut lines = recording_json(&mismatched);
        lines[0]["ir_version"] = serde_json::json!("999");
        write_recording_json(&mismatched, &lines);
        let error = Run::from_recording(&mismatched).expect_err("mismatch is refused");
        assert_eq!(error.code, RuntimeErrorCode::ReplayDiverged);

        let unsupported = empty_recording("unsupported-ir");
        let mut lines = recording_json(&unsupported);
        lines[0]["ir_version"] = serde_json::json!("999");
        lines[0]["ir"]["ir_version"] = serde_json::json!("999");
        write_recording_json(&unsupported, &lines);
        let error = Run::from_recording(&unsupported).expect_err("unsupported IR is refused");
        assert_eq!(error.code, RuntimeErrorCode::ReplayDiverged);
    }

    #[test]
    fn replay_rejects_invalid_start_sequence_and_run_envelopes() {
        // Spec 10.3: one leading start, one run id and contiguous sequences
        // are validated before the interpreter can replay any event.
        let bad_start = empty_recording("bad-start");
        let mut lines = recording_json(&bad_start);
        let mut duplicate = lines[0].clone();
        duplicate["seq"] = serde_json::json!(1);
        lines.insert(1, duplicate);
        for (index, line) in lines.iter_mut().enumerate().skip(2) {
            line["seq"] = serde_json::json!(index);
        }
        write_recording_json(&bad_start, &lines);
        assert_eq!(
            Run::from_recording(&bad_start)
                .expect_err("second start is refused")
                .code,
            RuntimeErrorCode::ReplayDiverged
        );

        let bad_sequence = empty_recording("bad-sequence");
        let mut lines = recording_json(&bad_sequence);
        lines[1]["seq"] = serde_json::json!(20);
        write_recording_json(&bad_sequence, &lines);
        assert_eq!(
            Run::from_recording(&bad_sequence)
                .expect_err("non-contiguous sequence is refused")
                .code,
            RuntimeErrorCode::ReplayDiverged
        );

        let bad_run = empty_recording("bad-run");
        let mut lines = recording_json(&bad_run);
        lines[1]["run_id"] = serde_json::json!("another-run");
        write_recording_json(&bad_run, &lines);
        assert_eq!(
            Run::from_recording(&bad_run)
                .expect_err("mixed run ids are refused")
                .code,
            RuntimeErrorCode::ReplayDiverged
        );
    }

    #[test]
    fn replay_surfaces_mismatched_and_trailing_terminal_events() {
        // Spec 10.4: the terminal end event is checked like every other event,
        // and a complete terminal recording has nothing after it.
        let mismatched = empty_recording("mismatched-end");
        let mut lines = recording_json(&mismatched);
        let end = lines
            .iter_mut()
            .find(|line| line["event"] == "end")
            .expect("has end event");
        end["kind"] = serde_json::json!("stopped");
        write_recording_json(&mismatched, &lines);
        let mut replay = Run::from_recording(&mismatched).expect("envelope is valid");
        let Pause::Error { code, .. } = replay.next().expect("surfaces divergence") else {
            panic!("expected replay divergence");
        };
        assert_eq!(code, RuntimeErrorCode::ReplayDiverged);

        let trailing = empty_recording("trailing-end");
        let mut lines = recording_json(&trailing);
        let mut extra = lines.last().expect("has end event").clone();
        extra["seq"] = serde_json::json!(lines.len());
        lines.push(extra);
        write_recording_json(&trailing, &lines);
        let mut replay = Run::from_recording(&trailing).expect("envelope is valid");
        let Pause::Error { code, .. } = replay.next().expect("surfaces trailing event") else {
            panic!("expected replay divergence");
        };
        assert_eq!(code, RuntimeErrorCode::ReplayDiverged);
    }

    #[test]
    fn resuming_a_run_that_is_not_paused_is_an_error() {
        let mut run = run();
        let error = run
            .resume(Resume::Nothing {})
            .expect_err("nothing to resume");
        assert_eq!(error, RunError::NotPaused);
    }

    #[test]
    fn a_confirm_takes_an_answer_and_nothing_else() {
        let mut run = run();
        run.pause_with(Pause::Confirm {
            common: common(),
            message: "Continue?".to_string(),
            options: Pause::default_options(),
            context: BTreeMap::new(),
        });
        assert_eq!(run.state().paused_on(), Some(PauseKind::Confirm));

        assert!(matches!(
            run.resume(Resume::Retry { retry: true }),
            Err(RunError::WrongPayload { .. })
        ));
        run.resume(Resume::Answer {
            answer: "yes".to_string(),
            text: None,
        })
        .expect("an answer resumes a confirm");
        assert_eq!(*run.state(), RunState::Running);
        assert!(run.current_pause().is_none());
    }

    #[test]
    fn a_budget_pause_needs_an_extension() {
        let mut run = run();
        run.pause_with(Pause::Budget {
            common: common(),
            key: jevscript_ir::BudgetKey::Calls,
            used: 40.0,
            limit: 40.0,
        });
        assert!(matches!(
            run.resume(Resume::Extend {
                extend: BTreeMap::new()
            }),
            Err(RunError::WrongPayload { .. })
        ));
        let extend = BTreeMap::from([(jevscript_ir::BudgetKey::Calls, 80.0)]);
        run.resume(Resume::Extend { extend })
            .expect("an extension resumes a budget pause");
        assert_eq!(*run.state(), RunState::Running);
    }

    #[test]
    fn only_a_retryable_error_can_be_retried() {
        let mut run = run();
        run.pause_with(Pause::Error {
            common: common(),
            code: RuntimeErrorCode::JevRejected,
            message: "bad request".to_string(),
            retryable: false,
        });
        // A non-retryable error ends the run (spec section 10.2).
        assert_eq!(*run.state(), RunState::Ended(PauseKind::Error));
        assert!(matches!(
            run.resume(Resume::Retry { retry: true }),
            Err(RunError::Ended(_))
        ));

        let mut run = self::run();
        run.pause_with(Pause::Error {
            common: common(),
            code: RuntimeErrorCode::JevUnavailable,
            message: "503".to_string(),
            retryable: true,
        });
        assert_eq!(run.state().paused_on(), Some(PauseKind::Error));
        run.resume(Resume::Retry { retry: true })
            .expect("a retryable error can be retried");
    }

    #[test]
    fn done_and_stopped_end_the_run() {
        let mut run = run();
        run.pause_with(Pause::Done {
            common: common(),
            outputs: BTreeMap::new(),
            verified: true,
            usage: Usage::default(),
        });
        assert_eq!(*run.state(), RunState::Ended(PauseKind::Done));
        assert!(matches!(run.next(), Err(RunError::Ended(_))));
        assert!(matches!(run.abort(), Err(RunError::Ended(_))));

        let mut run = self::run();
        run.pause_with(Pause::Stopped {
            common: common(),
            reason: "stuck".to_string(),
        });
        assert_eq!(*run.state(), RunState::Ended(PauseKind::Stopped));
    }

    #[test]
    fn an_escalate_is_terminal_unless_the_host_resumes_it() {
        let mut run = run();
        run.pause_with(Pause::Escalate {
            common: common(),
            reason: "unsure who owns this".to_string(),
            context: BTreeMap::new(),
        });
        // The run is held, not ended: the host decides.
        assert_eq!(run.state().paused_on(), Some(PauseKind::Escalate));
        run.resume(Resume::Continue { resume: true })
            .expect("the host may resume an escalate");
        assert_eq!(*run.state(), RunState::Running);
    }

    #[test]
    fn injecting_outside_a_waiting_pause_is_an_error() {
        let mut run = run();
        assert!(matches!(
            run.inject("claude", "hello"),
            Err(RunError::NotPaused)
        ));
        run.pause_with(Pause::Waiting {
            common: common(),
            on: "claude".to_string(),
            condition: "idle".to_string(),
            timeout_minutes: 5.0,
        });
        assert!(matches!(
            run.inject("tree", "hello"),
            Err(RunError::NotPaused)
        ));
    }

    #[test]
    fn abort_ends_the_run_as_stopped() {
        let mut run = run();
        run.abort().expect("a created run can be aborted");
        assert_eq!(*run.state(), RunState::Ended(PauseKind::Stopped));
    }

    #[test]
    fn an_empty_main_ends_done_with_no_outputs() {
        let mut run = run();
        let pause = run.next().expect("steps");
        assert_eq!(pause.kind(), PauseKind::Done, "{pause:?}");
        assert_eq!(*run.state(), RunState::Ended(PauseKind::Done));
        let Pause::Done {
            outputs, verified, ..
        } = pause
        else {
            panic!("done");
        };
        assert!(outputs.is_empty());
        assert!(!verified);
    }
}
