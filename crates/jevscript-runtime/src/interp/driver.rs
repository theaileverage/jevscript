//! The driver: where every non-deterministic effect of a run goes through
//! (spec sections 10.3 and 10.4).
//!
//! A run keeps its whole recording in memory as a log of events. The driver
//! serves that log back until it runs out; a live run then calls the world,
//! while an external replay requires an explicit changed or new host answer
//! before it may go live. That rule makes three things the same mechanism:
//!
//! - **Replay.** The log is the recording the host handed over. Every Jev
//!   request, capability call, observation, draw and pause is served from it
//!   with an identity check, and a mismatch or premature end is
//!   `replay_diverged`. Only a changed or new host answer leaves replay.
//! - **Resume.** The interpreter is a plain recursive walker with no saved
//!   continuation. After the host answers a pause, the next `Run::next` re-runs
//!   the task from the start against the log the run wrote, reaches the pause,
//!   takes the host's answer from the log and continues live. Nothing is
//!   called twice because everything before the pause is served.
//! - **Recording.** In live mode every effect is appended to the log, written
//!   to the recorder and queued for the host's event stream.
//!
//! The determinism guarantee of spec section 10.4 is exactly what the second
//! point relies on: given the same program, inputs and log, control flow up to
//! the last answer is identical.

use std::collections::BTreeMap;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use jevscript_syntax::Span;

use crate::capability::{Bindings, CallArgs, Observation};
use crate::error::{RuntimeError, RuntimeErrorCode};
use crate::jev::{HttpJevClient, JevClient, JevRequest, JevResponse, JevUsage};
use crate::pause::{Pause, PauseKind, Resume};
use crate::profile::Profile;
use crate::record::{
    DrawKind, EffectIdentity, EffectOperation, Event, EventSink, ExecutionMetadata,
    HOST_ABORT_REASON, RecordedEvent, Recorder, iso8601_from_unix_millis, iso8601_now,
    iso8601_to_unix_millis,
};
use crate::rng::Rng;
use crate::tokens::{TokenEstimator, estimator_for};
use crate::value::{Handle, Value};

/// A capability call's arguments as a `call` event records them: the
/// [`CallArgs`] as a record, so that positional and named arguments both
/// survive the round trip.
pub fn args_value(args: &CallArgs) -> Value {
    serde_json::to_value(args)
        .map(|json| Value::from_json(&json))
        .unwrap_or(Value::None)
}

/// The error an effect returns when the run has paused inside an expression.
///
/// The evaluator's effects return [`RuntimeError`], and a `confirm` or a
/// `budget` raised by `me.ask` in the middle of an expression has to unwind
/// through it somehow. The driver stashes the pause and the effect returns
/// this value; the statement interpreter asks the driver for the stashed pause
/// before it reads any error, so the message here is never shown to anyone.
pub(crate) fn interrupted() -> RuntimeError {
    RuntimeError::new(
        RuntimeErrorCode::TypeError,
        "the run paused inside an expression; this error is never surfaced",
    )
}

/// What a [`Driver`] is built from.
pub(crate) struct DriverSetup {
    /// The run's id, which every recorded event carries.
    pub run_id: String,
    /// The log to serve back before going live.
    pub log: Vec<Event>,
    /// Whether the log is a recording the host handed in.
    pub replaying: bool,
    /// Where live events are written, if anywhere.
    pub recorder: Option<Box<dyn Recorder>>,
    /// The adapters the host bound.
    pub bindings: Bindings,
    /// The model profile, for prices and the estimator.
    pub profile: Profile,
    /// The run's random source.
    pub rng: Rng,
    /// Metadata attached to the `start` recording line.
    pub execution: ExecutionMetadata,
    /// The host cancellation signal shared with the run boundary.
    pub abort_signal: Arc<AtomicBool>,
}

/// What reaching a pause does while catching up (spec sections 10.2-10.4).
pub(crate) enum PauseProgress {
    /// The recording already contains the host's answer.
    Resume(Resume, u64),
    /// The pause must be surfaced to the host.
    Surface,
    /// The host cancelled at this exact boundary.
    Abort,
}

/// The effect source of one run.
pub(crate) struct Driver<'c> {
    run_id: String,
    log: Vec<Event>,
    cursor: usize,
    recorder: Option<Box<dyn Recorder>>,
    outbox: Vec<RecordedEvent>,
    seq: u64,
    offset: u64,
    client: Option<Box<dyn JevClient + 'c>>,
    bindings: Bindings,
    profile: Profile,
    estimator: Box<dyn TokenEstimator>,
    rng: Rng,
    requests: u64,
    pending: Option<Pause>,
    surfaced_at: Option<usize>,
    /// The log index of the last pause the host has seen. A replayed pause
    /// beyond it is surfaced once, even though its answer is on record.
    shown_up_to: Option<usize>,
    /// Whether the log was handed in by the host (spec section 10.4), which
    /// changes what going live without a client or bindings means.
    replaying: bool,
    execution: ExecutionMetadata,
    event_sink: Option<EventSink>,
    abort_signal: Arc<AtomicBool>,
}

impl<'c> Driver<'c> {
    /// A driver for a run starting from `log`, which is empty for a fresh live
    /// run and the host's recording for a replay. Replay exhaustion remains a
    /// mismatch until [`Self::resume`] receives a changed or new answer.
    pub(crate) fn new(setup: DriverSetup) -> Self {
        let DriverSetup {
            run_id,
            log,
            replaying,
            recorder,
            bindings,
            profile,
            rng,
            execution,
            abort_signal,
        } = setup;
        let estimator = estimator_for(&profile.tokenizer);
        Self {
            run_id,
            log,
            cursor: 0,
            recorder,
            outbox: Vec::new(),
            seq: 0,
            offset: 0,
            client: None,
            bindings,
            profile,
            estimator,
            rng,
            requests: 0,
            pending: None,
            surfaced_at: None,
            shown_up_to: None,
            replaying,
            execution,
            event_sink: None,
            abort_signal,
        }
    }

    /// Stream each event at the moment it is appended (spec section 9.6).
    pub(crate) fn set_event_sink(&mut self, sink: EventSink) {
        self.event_sink = Some(sink);
    }

    /// Clone the host cancellation signal for a thread-safe public handle.
    pub(crate) fn abort_signal(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.abort_signal)
    }

    /// Request cancellation through the same signal used by an in-flight host.
    pub(crate) fn request_abort(&self) {
        self.abort_signal.store(true, Ordering::Release);
    }

    /// Start a pass over the log from the beginning. Every `Run::next` does
    /// this; only an originally live run calls the world after catching up.
    pub(crate) fn rewind(&mut self) {
        self.cursor = 0;
        self.requests = 0;
        self.pending = None;
    }

    /// Whether the driver is past its own catch-up log and calling the world.
    /// An external replay never becomes live merely because its log ended.
    pub(crate) fn is_live(&self) -> bool {
        !self.replaying && self.cursor >= self.log.len()
    }

    /// How many leading events of [`Self::log`] the latest pass has served or
    /// written: everything before it matched its identity check.
    pub(crate) fn cursor(&self) -> usize {
        self.cursor
    }

    /// The events written so far, in order.
    pub(crate) fn log(&self) -> &[Event] {
        &self.log
    }

    /// The pause an effect stashed before returning [`interrupted`], if any.
    pub(crate) fn take_pending(&mut self) -> Option<Pause> {
        self.pending.take()
    }

    /// Stash a pause that unwound from a nested statement list so that the
    /// expression it was raised in can return [`interrupted`].
    pub(crate) fn set_pending(&mut self, pause: Pause) {
        self.pending.get_or_insert(pause);
    }

    /// Whether the log already holds an answer or abort after the open pause.
    /// Either permits the next pass to advance without a new resume.
    pub(crate) fn can_advance_pause(&self) -> bool {
        self.surfaced_at.is_some_and(|at| {
            self.log[at + 1..]
                .iter()
                .any(|e| matches!(e, Event::Resume { .. } | Event::Abort { .. }))
        })
    }

    /// Prepare a direct host abort at the currently surfaced pause.
    ///
    /// A replay suffix is discarded, retaining only the already-recorded
    /// pause clock. A live run already has exactly that prefix in its log.
    pub(crate) fn prepare_paused_abort(&mut self) {
        if self.replaying {
            let keep = usize::min(
                self.cursor
                    + usize::from(matches!(
                        self.log.get(self.cursor),
                        Some(Event::Draw {
                            kind: DrawKind::Now,
                            ..
                        })
                    )),
                self.log.len(),
            );
            self.log.truncate(keep);
            self.replaying = false;
        }
        self.request_abort();
    }

    /// Prepare a direct host abort before the first run step.
    pub(crate) fn prepare_created_abort(&mut self) {
        self.go_live();
        self.request_abort();
    }

    /// Consume or append host cancellation at an interpreter boundary.
    pub(crate) fn abort_boundary(&mut self, span: Span) -> Result<bool, RuntimeError> {
        match self.log.get(self.cursor) {
            Some(Event::Abort { reason }) if reason == HOST_ABORT_REASON => {
                self.cursor += 1;
                return Ok(true);
            }
            Some(Event::Abort { .. }) => {
                return Err(self.diverged(span, "host cancellation with the specified reason"));
            }
            _ => {}
        }
        let requested = (self.replaying || self.cursor >= self.log.len())
            && self.abort_signal.swap(false, Ordering::AcqRel);
        if requested {
            if self.replaying {
                self.log.truncate(self.cursor);
                self.replaying = false;
            }
            self.append(Event::Abort {
                reason: HOST_ABORT_REASON.to_string(),
            })?;
            return Ok(true);
        }
        Ok(false)
    }

    /// The host answered the open pause (spec section 10.4): if the log
    /// already holds the same answer the run stays in replay; if it holds a
    /// different one the run leaves replay here and continues live; if it
    /// holds none the answer is appended.
    pub(crate) fn resume(&mut self, payload: Resume) -> Result<(), RuntimeError> {
        let Some(at) = self.surfaced_at else {
            return Ok(());
        };
        let recorded = self.log[at + 1..]
            .iter()
            .position(|e| matches!(e, Event::Resume { .. }))
            .map(|i| at + 1 + i);
        match recorded {
            Some(i)
                if self.log[i]
                    == (Event::Resume {
                        payload: payload.clone(),
                    }) =>
            {
                Ok(())
            }
            Some(i) => {
                self.log.truncate(i);
                self.replaying = false;
                self.append(Event::Resume { payload }).map(|_| ())
            }
            None => {
                self.replaying = false;
                self.append(Event::Resume { payload }).map(|_| ())
            }
        }
    }

    /// Use `client` for Jev instead of the profile's HTTP endpoint.
    pub(crate) fn set_client(&mut self, client: Box<dyn JevClient + 'c>) {
        self.client = Some(client);
    }

    /// Drain the events written since the last drain, for the host's stream.
    pub(crate) fn drain_outbox(&mut self) -> Vec<RecordedEvent> {
        std::mem::take(&mut self.outbox)
    }

    /// The byte offset the next event will be written at.
    fn next_offset(&self) -> u64 {
        self.recorder.as_ref().map_or(self.offset, |r| r.offset())
    }

    /// Append an event in live mode: to the log, the recorder and the outbox.
    fn append(&mut self, event: Event) -> Result<u64, RuntimeError> {
        let line = RecordedEvent {
            ts: iso8601_now(),
            run_id: self.run_id.clone(),
            seq: self.seq,
            execution: matches!(event, Event::Start { .. }).then(|| self.execution.clone()),
            event: event.clone(),
        };
        let written_at = match self.recorder.as_mut() {
            Some(recorder) => recorder.write(event.clone())?,
            None => self.offset,
        };
        let length = serde_json::to_string(&line)
            .map(|s| s.len() as u64 + 1)
            .unwrap_or(0);
        self.offset += length;
        self.seq += 1;
        self.log.push(event);
        // Live mode is "past the log": the cursor keeps up with what is
        // appended so that the next effect goes to the world, not the log.
        self.cursor = self.log.len();
        self.outbox.push(line);
        if let Some(sink) = self.event_sink.as_mut() {
            sink(self.outbox.last().expect("the event was just queued"))?;
        }
        Ok(written_at)
    }

    /// Close the recorder. Called when the run ends.
    pub(crate) fn close(&mut self) -> Result<(), RuntimeError> {
        match self.recorder.as_mut() {
            Some(recorder) => recorder.close(),
            None => Ok(()),
        }
    }

    fn diverged(&self, span: Span, expected: &str) -> RuntimeError {
        let found = self
            .log
            .get(self.cursor)
            .map_or_else(|| "the end of the recording".to_string(), event_name);
        RuntimeError::new(
            RuntimeErrorCode::ReplayDiverged,
            format!(
                "the program expected {expected} at event {} of the recording but found {found}",
                self.cursor
            ),
        )
        .at(span)
    }

    /// Return a recorded external failure after validating its exact identity.
    fn replayed_effect_error(
        &mut self,
        operation: EffectOperation,
        identity: &EffectIdentity,
        span: Span,
    ) -> Result<Option<RuntimeError>, RuntimeError> {
        let Some(Event::EffectError {
            operation: recorded_operation,
            identity: recorded_identity,
            code,
            message,
            retryable,
            source,
        }) = self.log.get(self.cursor)
        else {
            return Ok(None);
        };
        if *recorded_operation != operation || recorded_identity != identity {
            return Err(self.diverged(span, "the same failed external attempt"));
        }
        let error = RuntimeError::new(*code, message.clone())
            .retryable(*retryable)
            .at(*source);
        self.cursor += 1;
        Ok(Some(error))
    }

    /// Record a failed external attempt before its retry, pause or abort.
    fn append_effect_error(
        &mut self,
        operation: EffectOperation,
        identity: EffectIdentity,
        error: &RuntimeError,
    ) -> Result<(), RuntimeError> {
        self.append(Event::EffectError {
            operation,
            identity,
            code: error.code,
            message: error.message.clone(),
            retryable: error.retryable,
            source: error.span,
        })
        .map(|_| ())
    }

    /// Emit an event that carries no result: `start`, `end`, `warning`,
    /// `log`, `step`. In catch-up its deterministic payload must match
    /// exactly, and it is not appended or streamed again.
    pub(crate) fn emit(&mut self, event: Event, span: Span) -> Result<(), RuntimeError> {
        if !self.is_live() {
            if self.log.get(self.cursor) != Some(&event) {
                // A log's identity is its level, message and fields (spec 5.8).
                let expected = match &event {
                    Event::Log { level, .. } => format!(
                        "the same `{}` log (level, message and fields)",
                        level.as_str()
                    ),
                    other => format!("`{}`", event_name(other)),
                };
                return Err(self.diverged(span, &expected));
            }
            self.cursor += 1;
            if matches!(event, Event::End { .. }) && self.cursor != self.log.len() {
                return Err(self.diverged(span, "the end of the recording"));
            }
            return Ok(());
        }
        self.append(event).map(|_| ())
    }

    /// One Jev request (spec section 6). `sampled` is whether the answers
    /// event will note a draw (spec section 6.11).
    pub(crate) fn jev(
        &mut self,
        request: &JevRequest,
        sampled: bool,
        span: Span,
    ) -> Result<JevResponse, RuntimeError> {
        self.requests += 1;
        let request_id = format!("r{}", self.requests);
        let identity = EffectIdentity::Request {
            request_id: request_id.clone(),
        };
        if !self.is_live() {
            let Some(Event::Request {
                request_id: recorded_id,
                state,
                questions,
            }) = self.log.get(self.cursor)
            else {
                return Err(self.diverged(span, "a Jev request"));
            };
            let same_state = state
                .full()
                .is_some_and(|recorded| *recorded == request.state);
            if recorded_id != &request_id || *questions != request.questions || !same_state {
                return Err(self.diverged(span, "the same Jev request"));
            }
            let recorded_id = recorded_id.clone();
            self.cursor += 1;
            if let Some(error) =
                self.replayed_effect_error(EffectOperation::Request, &identity, span)?
            {
                return Err(error);
            }
            let Some(Event::Answers {
                request_id: answered,
                answers,
                usage,
                latency_ms,
                ..
            }) = self.log.get(self.cursor)
            else {
                return Err(self.diverged(span, "the answers to the request"));
            };
            if *answered != recorded_id {
                return Err(self.diverged(span, "the answers to the same request"));
            }
            let response = JevResponse {
                answers: answers.clone(),
                usage: usage.clone(),
                latency_ms: *latency_ms,
            };
            self.cursor += 1;
            return Ok(response);
        }
        self.append(Event::Request {
            request_id: request_id.clone(),
            state: request.state.clone().into(),
            questions: request.questions.clone(),
        })?;
        if self.client.is_none() {
            match HttpJevClient::for_profile(&self.profile) {
                Ok(client) => self.client = Some(Box::new(client)),
                Err(error) => {
                    let error = error.at(span);
                    self.append_effect_error(EffectOperation::Request, identity, &error)?;
                    return Err(error);
                }
            }
        }
        let client = self.client.as_ref().expect("set above");
        let response = match client.send(request) {
            Ok(response) => response,
            Err(error) => {
                let error = RuntimeError::from(error).at(span);
                self.append_effect_error(EffectOperation::Request, identity, &error)?;
                return Err(error);
            }
        };
        self.append(Event::Answers {
            request_id,
            answers: response.answers.clone(),
            usage: response.usage.clone(),
            latency_ms: response.latency_ms,
            sampled,
        })?;
        Ok(response)
    }

    /// One capability call (spec section 9.6), recorded as a `call` event.
    pub(crate) fn call(
        &mut self,
        capability: &str,
        verb: &str,
        args: &CallArgs,
        span: Span,
    ) -> Result<Value, RuntimeError> {
        let current_args = args_value(args);
        let identity = EffectIdentity::Call {
            capability: capability.to_string(),
            verb: verb.to_string(),
            args: current_args.clone(),
        };
        if !self.is_live() {
            if let Some(error) =
                self.replayed_effect_error(EffectOperation::Call, &identity, span)?
            {
                return Err(error);
            }
            let Some(Event::Call {
                capability: recorded_capability,
                verb: recorded_verb,
                args: recorded_args,
                result,
                ..
            }) = self.log.get(self.cursor)
            else {
                return Err(self.diverged(span, &format!("the call `{capability}.{verb}`")));
            };
            if recorded_capability != capability
                || recorded_verb != verb
                || serde_json::to_value(recorded_args).ok()
                    != serde_json::to_value(&current_args).ok()
            {
                return Err(self.diverged(
                    span,
                    &format!(
                        "the call `{capability}.{verb}` with arguments {current_args:?}, not `{recorded_capability}.{recorded_verb}` with {recorded_args:?}"
                    ),
                ));
            }
            let Some(result) = result.full().cloned() else {
                return Err(self.diverged(span, "a full replayed capability result"));
            };
            self.cursor += 1;
            return Ok(result);
        }
        let started = std::time::Instant::now();
        let adapter = self.adapter(capability, span)?;
        let result = match adapter.call(verb, args) {
            Ok(result) => result,
            Err(error) => {
                let error = error.at(span);
                self.append_effect_error(EffectOperation::Call, identity, &error)?;
                return Err(error);
            }
        };
        let latency_ms = started.elapsed().as_millis() as u64;
        self.append(Event::Call {
            capability: capability.to_string(),
            verb: verb.to_string(),
            args: current_args,
            result: result.clone().into(),
            latency_ms,
        })?;
        Ok(result)
    }

    /// One observation of an agent handle (spec section 9.1), recorded as an
    /// `observe` event.
    pub(crate) fn observe(
        &mut self,
        capability: &str,
        handle: &Handle,
        span: Span,
    ) -> Result<Observation, RuntimeError> {
        let identity = EffectIdentity::Observe {
            capability: capability.to_string(),
            handle: Value::Handle(handle.clone()),
        };
        if !self.is_live() {
            if let Some(error) =
                self.replayed_effect_error(EffectOperation::Observe, &identity, span)?
            {
                return Err(error);
            }
            let Some(Event::Observe {
                capability: recorded_capability,
                handle: recorded_handle,
                observation,
                ..
            }) = self.log.get(self.cursor)
            else {
                return Err(self.diverged(span, &format!("an observation of `{capability}`")));
            };
            if recorded_capability != capability
                || recorded_handle != &Value::Handle(handle.clone())
            {
                return Err(self.diverged(span, &format!("an observation of `{capability}`")));
            }
            let Some(observation) = observation.full().cloned() else {
                return Err(self.diverged(span, "a full replayed observation"));
            };
            self.cursor += 1;
            return Ok(observation);
        }
        let adapter = self.adapter(capability, span)?;
        let observation = match adapter.observe(handle) {
            Ok(observation) => observation,
            Err(error) => {
                let error = error.at(span);
                self.append_effect_error(EffectOperation::Observe, identity, &error)?;
                return Err(error);
            }
        };
        self.append(Event::Observe {
            capability: capability.to_string(),
            handle: Value::Handle(handle.clone()),
            observation: observation.clone().into(),
        })?;
        Ok(observation)
    }

    /// One `llm.write` (spec section 9.3), recorded as a `generate` event.
    pub(crate) fn generate(
        &mut self,
        capability: &str,
        instruction: &str,
        using: Option<&Value>,
        span: Span,
    ) -> Result<(String, JevUsage), RuntimeError> {
        let identity = EffectIdentity::Generate {
            capability: capability.to_string(),
            instruction: instruction.to_string(),
            using: using.cloned(),
        };
        if !self.is_live() {
            if let Some(error) =
                self.replayed_effect_error(EffectOperation::Generate, &identity, span)?
            {
                return Err(error);
            }
            let Some(Event::Generate {
                capability: recorded_capability,
                instruction: recorded_instruction,
                using: recorded_using,
                output,
                usage,
            }) = self.log.get(self.cursor)
            else {
                return Err(self.diverged(span, &format!("a generation by `{capability}`")));
            };
            if recorded_capability != capability
                || recorded_instruction != instruction
                || recorded_using.as_ref() != using
            {
                return Err(self.diverged(span, &format!("a generation by `{capability}`")));
            }
            let generated = (output.clone(), usage.clone());
            self.cursor += 1;
            return Ok(generated);
        }
        let mut args = CallArgs {
            positional: vec![Value::Text(instruction.to_string())],
            named: BTreeMap::new(),
        };
        if let Some(using) = using {
            args.named.insert("using".to_string(), using.clone());
        }
        let adapter = self.adapter(capability, span)?;
        let output = match adapter.call("write", &args) {
            Ok(output) => output.to_text(),
            Err(error) => {
                let error = error.at(span);
                self.append_effect_error(EffectOperation::Generate, identity, &error)?;
                return Err(error);
            }
        };
        let mut seen = instruction.to_string();
        if let Some(using) = using {
            seen.push_str(&using.to_text());
        }
        seen.push_str(&output);
        // The runtime prices Jev, not the host's text model, so a generation
        // reports its estimated tokens and no spend (spec section 7.1).
        let usage = JevUsage {
            tokens: self.estimator.estimate(&seen),
            usd: None,
        };
        self.append(Event::Generate {
            capability: capability.to_string(),
            instruction: instruction.to_string(),
            using: using.cloned(),
            output: output.clone(),
            usage: usage.clone(),
        })?;
        Ok((output, usage))
    }

    /// A number in `[0, 1)` from the run's random source (spec section 6.11).
    pub(crate) fn draw_random(&mut self, span: Span) -> Result<f64, RuntimeError> {
        if !self.is_live() {
            let Some(Event::Draw {
                kind: DrawKind::Random,
                value,
            }) = self.log.get(self.cursor)
            else {
                return Err(self.diverged(span, "a random draw"));
            };
            let value = value.as_f64().unwrap_or(0.0);
            self.cursor += 1;
            return Ok(value);
        }
        let value = self.rng.next_f64();
        self.append(Event::Draw {
            kind: DrawKind::Random,
            value: serde_json::json!(value),
        })?;
        Ok(value)
    }

    /// The clock as ISO text (spec section 5.7). This is the only clock the
    /// interpreter has; the `minutes` budget reads it too, so that replay
    /// measures the same minutes.
    pub(crate) fn draw_now(&mut self, span: Span) -> Result<String, RuntimeError> {
        if !self.is_live() {
            let Some(Event::Draw {
                kind: DrawKind::Now,
                value,
            }) = self.log.get(self.cursor)
            else {
                return Err(self.diverged(span, "a clock reading"));
            };
            let value = value.as_str().unwrap_or_default().to_string();
            self.cursor += 1;
            return Ok(value);
        }
        let millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let value = iso8601_from_unix_millis(millis);
        self.append(Event::Draw {
            kind: DrawKind::Now,
            value: serde_json::json!(value),
        })?;
        Ok(value)
    }

    /// Reach a pause (spec section 10.2).
    ///
    /// `Ok(Some((resume, paused_ms)))` means the host has already seen this
    /// pause and answered it, so the run continues with the answer and the
    /// time it was paused for; `Ok(None)` means the pause has just been
    /// surfaced and the interpreter must unwind. In catch-up the pause is
    /// checked against the recorded one, and the recorded payload is what
    /// the host sees, so a replayed pause is identical to the first time. A
    /// replayed pause is surfaced once even though its answer is on record
    /// (spec section 10.4).
    ///
    /// The clock is read right after a non-terminal pause is written and
    /// again when its answer is taken; the difference is time paused, which
    /// the `minutes` budget excludes (spec section 7.1).
    pub(crate) fn pause(&mut self, pause: Pause) -> Result<PauseProgress, RuntimeError> {
        let span = pause.common().source;
        if !self.is_live() {
            let Some(Event::Pause { payload, .. }) = self.log.get(self.cursor) else {
                return Err(self.diverged(span, &format!("a `{}` pause", pause.kind().as_str())));
            };
            if !same_pause(payload, &pause) {
                return Err(self.diverged(
                    span,
                    &format!(
                        "the same `{}` pause, not the recorded `{}`",
                        pause.kind().as_str(),
                        payload.kind().as_str()
                    ),
                ));
            }
            let recorded = payload.clone();
            let at = self.cursor;
            self.cursor += 1;
            if self.shown_up_to.is_none_or(|shown| at > shown) || recorded.is_terminal() {
                self.surfaced_at = Some(at);
                self.shown_up_to = Some(at);
                self.pending = Some(recorded);
                return Ok(PauseProgress::Surface);
            }
            if self.abort_boundary(span)? {
                return Ok(PauseProgress::Abort);
            }
            let before = self.clock_after_pause(span)?;
            // Messages injected while the pause was open were sent then and
            // recorded then; they are not sent again (spec section 10.2).
            while matches!(
                self.log.get(self.cursor),
                Some(Event::Call { verb, .. }) if verb == "send"
            ) && recorded.kind() == PauseKind::Waiting
            {
                self.cursor += 1;
            }
            if self.abort_boundary(span)? {
                return Ok(PauseProgress::Abort);
            }
            let Some(Event::Resume { payload }) = self.log.get(self.cursor) else {
                // Shown, but not answered yet: the host is still looking.
                self.surfaced_at = Some(at);
                self.pending = Some(recorded);
                return Ok(PauseProgress::Surface);
            };
            let payload = payload.clone();
            self.cursor += 1;
            let after = self.draw_now(span)?;
            let paused = iso8601_to_unix_millis(&after)?.saturating_sub(before);
            return Ok(PauseProgress::Resume(payload, paused));
        }
        let mut pause = pause;
        let offset = self.next_offset();
        pause.common_mut().recording_offset = offset;
        let kind = pause.kind();
        self.append(Event::Pause {
            kind,
            payload: pause.clone(),
        })?;
        let at = self.log.len() - 1;
        if !pause.is_terminal() {
            self.draw_now(span)?;
        }
        self.surfaced_at = Some(at);
        self.shown_up_to = Some(at);
        self.pending = Some(pause);
        Ok(PauseProgress::Surface)
    }

    /// The clock reading written right after a pause, as milliseconds.
    fn clock_after_pause(&mut self, span: Span) -> Result<u64, RuntimeError> {
        match self.log.get(self.cursor) {
            Some(Event::Draw {
                kind: DrawKind::Now,
                value,
            }) => {
                let millis = iso8601_to_unix_millis(value.as_str().unwrap_or_default())?;
                self.cursor += 1;
                Ok(millis)
            }
            _ => Err(self.diverged(span, "the clock reading after the pause")),
        }
    }

    /// Leave the rest of the log behind: what follows the cursor can no longer
    /// happen. Used when replay diverges, so the error pause and the `end`
    /// that follow it are recorded after the events that did match.
    pub(crate) fn go_live(&mut self) {
        self.log.truncate(self.cursor);
        self.replaying = false;
    }

    /// Send a message to a handle while a `waiting` pause is open (spec
    /// section 10.2). The call is made now and recorded now, so replay does
    /// not repeat it.
    pub(crate) fn inject(
        &mut self,
        capability: &str,
        handle: &Handle,
        message: &str,
    ) -> Result<(), RuntimeError> {
        let args = CallArgs {
            positional: vec![
                Value::Handle(handle.clone()),
                Value::Text(message.to_string()),
            ],
            named: BTreeMap::new(),
        };
        let started = std::time::Instant::now();
        let result = self
            .adapter(capability, Span::default())?
            .call("send", &args)?;
        let latency_ms = started.elapsed().as_millis() as u64;
        self.append(Event::Call {
            capability: capability.to_string(),
            verb: "send".to_string(),
            args: args_value(&args),
            result: result.into(),
            latency_ms,
        })?;
        Ok(())
    }

    fn adapter(
        &mut self,
        capability: &str,
        span: Span,
    ) -> Result<&mut Box<dyn crate::capability::Capability>, RuntimeError> {
        let replaying = self.replaying;
        self.bindings.get_mut(capability).ok_or_else(|| {
            let why = if replaying {
                format!(
                    "the recording ended and the replay was started without a binding for `{capability}`"
                )
            } else {
                format!("capability `{capability}` was not bound")
            };
            RuntimeError::new(RuntimeErrorCode::BindingMissing, why).at(span)
        })
    }
}

/// Whether a replayed pause has the same deterministic payload. The byte
/// offset is primary-file framing metadata and can differ after companion
/// hydration restores larger full payloads (spec sections 10.3 and 10.4).
fn same_pause(recorded: &Pause, raised: &Pause) -> bool {
    let mut recorded = recorded.clone();
    let mut raised = raised.clone();
    recorded.common_mut().recording_offset = 0;
    raised.common_mut().recording_offset = 0;
    recorded == raised
}

/// The name of an event, for divergence messages.
fn event_name(event: &Event) -> String {
    match event {
        Event::Start { .. } => "`start`".to_string(),
        Event::Request { .. } => "a Jev request".to_string(),
        Event::Answers { .. } => "Jev answers".to_string(),
        Event::Generate { capability, .. } => format!("a generation by `{capability}`"),
        Event::Call {
            capability, verb, ..
        } => format!("the call `{capability}.{verb}`"),
        Event::Observe { capability, .. } => format!("an observation of `{capability}`"),
        Event::EffectError { operation, .. } => {
            format!("a failed `{operation:?}` external attempt")
        }
        Event::Draw { kind, .. } => match kind {
            DrawKind::Random => "a random draw".to_string(),
            DrawKind::Now => "a clock reading".to_string(),
        },
        Event::Warning { code, .. } => format!("a `{code}` warning"),
        Event::Log { level, .. } => format!("a `{}` log", level.as_str()),
        Event::Pause { kind, .. } => format!("a `{}` pause", kind.as_str()),
        Event::Resume { .. } => "a resume".to_string(),
        Event::Abort { .. } => "host cancellation".to_string(),
        Event::Step { .. } => "a step record".to_string(),
        Event::MachineStep { machine, .. } => format!("a step of machine `{machine}`"),
        Event::End { kind, .. } => format!("the end of the run as `{}`", kind.as_str()),
    }
}
