//! The statement interpreter (spec sections 5, 7, 8 and 9).
//!
//! A run is a plain recursive walk over the IR. Every effect goes through the
//! [`driver`], which records it live or serves it from the log, and every pause
//! unwinds the walk as a [`Halt`]. There is no saved continuation: after the
//! host answers, the next `Run::next` walks the task again from the start,
//! with everything before the pause served from the log, and picks up where it
//! left off. That is the same mechanism as replay (spec section 10.4), so a
//! recording is what a run *is*, not a side channel.
//!
//! What lives here:
//!
//! - statements: assignment with block scoping, `if`, `for`, `loop`, `until`
//!   with `verify`, `continue`, `break`, `return`, `stop`, `escalate`,
//!   `gate` and `shape`;
//! - judgment execution by request group (spec section 6.6), named judgment
//!   calls, and `focus`;
//! - defs, tasks and machines as call frames, with the recursion limit,
//!   machine menus and nested budgets (spec sections 7.1, 7.8 and 8);
//! - capability verbs and the pauses they raise (spec section 9), and the
//!   `trail` records the runtime writes from its own calls (spec section 7.5).

pub(crate) mod driver;
mod machine;

use std::collections::{BTreeMap, BTreeSet};

use jevscript_ir::{
    Budget, BudgetKey, CapabilityKind, Def, DefParam, Expr, Ir, Judge, JudgeVerb, Machine,
    ReturnType, ShapeField, ShapePolicy, Stmt, Subject, Task, TextPart, Thresholds, Verdict,
};
use jevscript_syntax::Span;

use crate::answer;
use crate::capability::{CallArgs, Observation};
use crate::error::{RuntimeError, RuntimeErrorCode};
use crate::eval::{CallTarget, Effects, Env, Evaluator, bind_names, chunk, short_hash};
use crate::jev::{JevRequest, JevResponse};
use crate::pause::{Pause, PauseCommon, Resume, Usage};
use crate::profile::Profile;
use crate::record::{
    BindingRecord, Event, LogRecord, StepOutcome, StepRecord, iso8601_to_unix_millis,
};
use crate::state::{self, Ask, PlannedKind};
use crate::tokens::TokenEstimator;
use crate::value::{Handle, PauseResult, Value};

use self::driver::{Driver, PauseProgress, interrupted};

/// How deep defs may recurse before the run pauses with `recursion_limit`
/// (spec section 8). Deep enough for any prelude-style recursion such as
/// `focus_impl`, shallow enough that a runaway def is caught long before the
/// Rust stack is.
pub const RECURSION_LIMIT: usize = 64;

/// The `calls` budget a task gets when it declares none (spec section 7.1).
/// The compiler fills it in; the runtime applies it too so that a hand-built
/// IR is bounded the same way.
pub const DEFAULT_CALLS: f64 = 50.0;

/// A step record's `args` are the text form of the call's arguments capped at
/// this many tokens (spec section 7.5).
const TRAIL_ARGS_TOKENS: u64 = 200;

/// The chunk size, keep threshold and fallback count of `focus`, which are the
/// numbers written in the prelude def of spec section 7.3.
const FOCUS_CHUNK_TOKENS: u64 = 1500;
const FOCUS_KEEP_ABOVE: f64 = 0.6;
const FOCUS_TOP: usize = 3;

/// The message of the `confirm` a gate raises when its arm was not written
/// (spec section 7.6).
pub const GATE_CONFIRM_MESSAGE: &str = "Gate asked for confirmation";

/// The reasons the runtime writes when a gate's default arm ends the run.
pub const GATE_DECLINED_REASON: &str = "Gate confirmation declined";
/// See [`GATE_DECLINED_REASON`].
pub const GATE_STOP_REASON: &str = "Gate stopped";
/// See [`GATE_DECLINED_REASON`].
pub const GATE_ESCALATE_REASON: &str = "Gate escalated";

/// The warning code written when a `shape` field is cut to its cap (spec 7.2).
pub const TRUNCATED_WARNING: &str = "truncated";

/// The walk has stopped at a pause that the driver has already recorded and
/// surfaced. It unwinds to the top, where the run reports it.
#[derive(Debug)]
pub(crate) enum Halt {
    /// The pause the host now sees. Boxed so that every `Exec` result stays
    /// small on the way up the walk.
    Pause(Box<Pause>),
}

impl Halt {
    fn pause(pause: Pause) -> Self {
        Halt::Pause(Box::new(pause))
    }
}

type Exec<T> = Result<T, Halt>;

/// How a statement list ended.
enum Flow {
    Next,
    Continue,
    Break,
    Return(Value),
}

/// The budget state of one task on the call stack (spec section 7.1).
struct TaskFrame {
    budget: Budget,
    thresholds: Thresholds,
    has_verify: bool,
    /// Whether execution reached the verify test at all (spec section 7.4):
    /// a `return` before the loop leaves the task unverified without ever
    /// reading the condition.
    verify_reached: bool,
    verified: bool,
    gate_done: bool,
    calls: f64,
    usd: f64,
    steps: f64,
    started_ms: u64,
    paused_ms: u64,
    trail: Vec<StepRecord>,
    pending_steps: Vec<StepRecord>,
    next_step: u64,
}

impl TaskFrame {
    fn limit(&self, key: BudgetKey) -> Option<f64> {
        self.budget.get(key)
    }

    fn set_limit(&mut self, key: BudgetKey, limit: f64) {
        match key {
            BudgetKey::Calls => self.budget.calls = Some(limit),
            BudgetKey::Minutes => self.budget.minutes = Some(limit),
            BudgetKey::Usd => self.budget.usd = Some(limit),
            BudgetKey::Steps => self.budget.steps = Some(limit),
        }
    }

    fn used(&self, key: BudgetKey, now_ms: u64) -> f64 {
        match key {
            BudgetKey::Calls => self.calls,
            BudgetKey::Usd => self.usd,
            BudgetKey::Steps => self.steps,
            BudgetKey::Minutes => self.elapsed_minutes(now_ms),
        }
    }

    fn add(&mut self, key: BudgetKey, amount: f64) {
        match key {
            BudgetKey::Calls => self.calls += amount,
            BudgetKey::Usd => self.usd += amount,
            BudgetKey::Steps => self.steps += amount,
            BudgetKey::Minutes => {}
        }
    }

    /// Wall-clock minutes since the task started, excluding time paused.
    fn elapsed_minutes(&self, now_ms: u64) -> f64 {
        now_ms
            .saturating_sub(self.started_ms)
            .saturating_sub(self.paused_ms) as f64
            / 60_000.0
    }
}

enum FrameKind {
    Task(Box<TaskFrame>),
    Machine(Box<TaskFrame>),
    Def,
}

/// One task, machine or def call frame (spec sections 7, 7.8 and 8).
struct Frame {
    /// Stable identity for this dynamic frame during one deterministic walk.
    id: u64,
    /// The unit's qualified name, which a pause reports as its `task`.
    unit: String,
    /// The module alias the unit came from; empty for the root program.
    module: String,
    kind: FrameKind,
    /// The current state while this is a machine frame, for pause payloads.
    state: Option<String>,
    loop_depth: u32,
}

/// What one unit resolves to.
enum Unit<'a> {
    Judgment(&'a jevscript_ir::Judgment),
    Def(&'a Def),
    Task(&'a Task),
    Machine(&'a Machine),
}

/// What a pass of the interpreter ends with.
pub(crate) struct Outcome {
    /// The pause surfaced to the host.
    pub pause: Pause,
    /// What the run has cost so far.
    pub usage: Usage,
    /// The handle a `waiting` pause is on, for `inject`.
    pub waiting: Option<(String, Handle)>,
}

/// One pass of the interpreter over one run.
pub(crate) struct Interpreter<'a, 'c> {
    ir: &'a Ir,
    profile: &'a Profile,
    driver: &'a mut Driver<'c>,
    estimator: Box<dyn TokenEstimator>,
    run_id: String,
    task: String,
    inputs: BTreeMap<String, Value>,
    sample_run: bool,
    frames: Vec<Frame>,
    next_frame_id: u64,
    usage: Usage,
    span: Span,
    last_observed: BTreeMap<String, String>,
    waiting: Option<(String, Handle)>,
}

impl<'a, 'c> Interpreter<'a, 'c> {
    /// An interpreter for one pass over `task`.
    pub(crate) fn new(
        ir: &'a Ir,
        profile: &'a Profile,
        driver: &'a mut Driver<'c>,
        run_id: String,
        task: String,
        inputs: BTreeMap<String, Value>,
        sample_run: bool,
    ) -> Self {
        driver.rewind();
        let estimator = crate::tokens::estimator_for(&profile.tokenizer);
        Self {
            ir,
            profile,
            driver,
            estimator,
            run_id,
            task,
            inputs,
            sample_run,
            frames: Vec::new(),
            next_frame_id: 0,
            usage: Usage::default(),
            span: Span::default(),
            last_observed: BTreeMap::new(),
            waiting: None,
        }
    }

    /// Walk the task until it pauses.
    pub(crate) fn run(mut self) -> Outcome {
        let pause = match self.run_task() {
            Ok(pause) => pause,
            Err(Halt::Pause(pause)) => *pause,
        };
        let pause = self.driver.take_pending().unwrap_or(pause);
        Outcome {
            pause,
            usage: self.usage,
            waiting: self.waiting,
        }
    }

    fn run_task(&mut self) -> Exec<Pause> {
        let ir = self.ir;
        let Some(task) = ir.task(&self.task) else {
            let error = RuntimeError::new(
                RuntimeErrorCode::TypeError,
                format!("`{}` declares no task `{}`", ir.program, self.task),
            );
            return Err(self.end_with_error(error));
        };
        self.span = task.span;
        let start = Event::Start {
            program: ir.program.clone(),
            task: task.name.clone(),
            inputs: self.inputs.clone(),
            bindings: ir
                .needs
                .iter()
                .map(|need| BindingRecord {
                    name: need.name.clone(),
                    kind: kind_name(need.kind).to_string(),
                })
                .collect(),
            ir_version: ir.ir_version.clone(),
        };
        if let Err(error) = self.driver.emit(start, task.span) {
            return Err(self.end_with_error(error));
        }
        let module = self.module_of(&task.name);
        let mut env = self.env_for(&module);
        // A declared `out` is a variable of the program, so an assignment to
        // it from inside a block updates it rather than making a block-local
        // (spec sections 3.3 and 5.3; 14.1 assigns `pr_url` inside an `if`).
        // Unassigned, it is `none` on `done` (spec section 7).
        for out in &ir.outputs {
            env.define(out.name.clone(), Value::None);
        }
        if let Err(Halt::Pause(pause)) = self.cancellation_boundary() {
            self.end(&env, &pause)?;
            return Err(Halt::Pause(pause));
        }
        // A task other than `main` takes its parameters from the inputs by
        // name (spec section 7: `main` has none and reads `in` directly).
        for param in &task.params {
            let value = self.inputs.get(&param.name).cloned();
            match (value, &param.default) {
                (Some(value), _) => env.define(param.name.clone(), value),
                (None, Some(default)) => {
                    let value = self.eval(&mut env, default)?;
                    env.define(param.name.clone(), value);
                }
                (None, None) => env.define(param.name.clone(), Value::None),
            }
        }
        let result = self
            .enter_task(task, module)
            .and_then(|()| self.exec_block(&mut env, &task.body));
        let flow = match result {
            Ok(flow) => flow,
            Err(Halt::Pause(pause)) => {
                if pause.is_terminal() {
                    self.end(&env, &pause)?;
                }
                return Err(Halt::Pause(pause));
            }
        };
        let _returned = match flow {
            Flow::Return(value) => value,
            _ => Value::None,
        };
        // Spec 7.4: `done` carries `verified: true` only if the declared
        // condition is true at the end. The loop's own test is what ended
        // the loop; what happened after it is why the condition is read
        // again here, in the task's final environment. A `return` that
        // never reached the loop leaves the task unverified, and the skipped
        // condition is not read: it may name loop-time locals or call the
        // world.
        let reached = self.task_frame().is_some_and(|f| f.verify_reached);
        let verified = match verify_expression(&task.body) {
            Some(test) if reached => self.truthy(&mut env, test)?,
            _ => false,
        };
        // Spec 7.1 and 9.6: `minutes` keeps running while the runtime
        // blocks, so the budget is checked once more before the run ends.
        self.check(BudgetKey::Minutes, 0.0)?;
        let now = self.clock_ms()?;
        self.settle_usage(now);
        let outputs = self.outputs(&env);
        let pause = Pause::Done {
            common: self.common(task.span),
            outputs,
            verified,
            usage: self.usage.clone(),
        };
        let halt = self.end_with(pause);
        let Halt::Pause(pause) = &halt;
        let pause = (**pause).clone();
        self.end(&env, &pause)?;
        Err(halt)
    }

    /// Write the `end` event for a terminal pause (spec section 10.3).
    fn end(&mut self, env: &Env, pause: &Pause) -> Exec<()> {
        let (usage, verified) = match pause {
            Pause::Done {
                usage, verified, ..
            } => (usage.clone(), *verified),
            Pause::Error {
                code: RuntimeErrorCode::ReplayDiverged,
                ..
            } => (
                self.usage.clone(),
                self.task_frame().is_some_and(|f| f.verified),
            ),
            Pause::Stopped { reason, .. } if reason == crate::record::HOST_ABORT_REASON => (
                self.usage.clone(),
                self.task_frame().is_some_and(|f| f.verified),
            ),
            _ => {
                let now = self.clock_ms()?;
                self.settle_usage(now);
                (
                    self.usage.clone(),
                    self.task_frame().is_some_and(|f| f.verified),
                )
            }
        };
        let event = Event::End {
            kind: pause.kind(),
            outputs: self.outputs(env),
            verified,
            usage,
        };
        let span = pause.common().source;
        if let Err(error) = self.driver.emit(event, span) {
            return Err(self.end_with_error(error));
        }
        if let Err(error) = self.driver.close() {
            return Err(self.end_with_error(error));
        }
        Ok(())
    }

    /// Every declared `out`, `none` when unassigned (spec section 7).
    fn outputs(&self, env: &Env) -> BTreeMap<String, Value> {
        self.ir
            .outputs
            .iter()
            .map(|out| {
                (
                    out.name.clone(),
                    env.get(&out.name).cloned().unwrap_or(Value::None),
                )
            })
            .collect()
    }

    /// Fill `minutes` and `steps` from the root frame.
    fn settle_usage(&mut self, now_ms: u64) {
        if let Some(Frame {
            kind: FrameKind::Task(frame),
            ..
        }) = self.frames.first()
        {
            self.usage.minutes = frame.elapsed_minutes(now_ms);
            self.usage.steps = frame.steps as u64;
        }
    }

    /* ---------------------------------------------------------------------- */
    /* Frames                                                                  */
    /* ---------------------------------------------------------------------- */

    fn enter_task(&mut self, task: &Task, module: String) -> Exec<()> {
        let started_ms = self.clock_ms()?;
        let mut budget = task.budget.clone();
        if budget.calls.is_none() {
            budget.calls = Some(DEFAULT_CALLS);
        }
        let id = self.allocate_frame_id();
        self.frames.push(Frame {
            id,
            unit: task.name.clone(),
            module,
            kind: FrameKind::Task(Box::new(TaskFrame {
                budget,
                thresholds: task.thresholds.clone(),
                has_verify: declares_verify(&task.body),
                verify_reached: false,
                verified: false,
                gate_done: false,
                calls: 0.0,
                usd: 0.0,
                steps: 0.0,
                started_ms,
                paused_ms: 0,
                trail: Vec::new(),
                pending_steps: Vec::new(),
                next_step: 1,
            })),
            state: None,
            loop_depth: 0,
        });
        Ok(())
    }

    /// Assign a deterministic identity to one dynamic call frame. Qualified
    /// unit names are not unique when tasks or machines recurse (spec 7.1).
    fn allocate_frame_id(&mut self) -> u64 {
        let id = self.next_frame_id;
        self.next_frame_id += 1;
        id
    }

    fn task_frame(&self) -> Option<&TaskFrame> {
        self.frames
            .iter()
            .rev()
            .find_map(|frame| match &frame.kind {
                FrameKind::Task(task) => Some(task.as_ref()),
                FrameKind::Machine(_) | FrameKind::Def => None,
            })
    }

    fn task_frame_mut(&mut self) -> Option<&mut TaskFrame> {
        self.frames
            .iter_mut()
            .rev()
            .find_map(|frame| match &mut frame.kind {
                FrameKind::Task(task) => Some(task.as_mut()),
                FrameKind::Machine(_) | FrameKind::Def => None,
            })
    }

    /// The task a pause is reported under: the innermost task on the stack.
    fn current_task(&self) -> String {
        self.frames
            .iter()
            .rev()
            .find(|frame| matches!(frame.kind, FrameKind::Task(_) | FrameKind::Machine(_)))
            .map_or_else(
                || self.task.clone(),
                |frame| match frame.kind {
                    FrameKind::Machine(_) => format!("machine:{}", frame.unit),
                    FrameKind::Task(_) | FrameKind::Def => frame.unit.clone(),
                },
            )
    }

    fn common(&self, span: Span) -> PauseCommon {
        self.common_for(self.current_task(), span)
    }

    fn common_for(&self, task: String, span: Span) -> PauseCommon {
        let step = match self.frames.first() {
            Some(Frame {
                kind: FrameKind::Task(frame),
                ..
            }) => frame.steps as u64,
            _ => 0,
        };
        let state = self.frames.iter().rev().find_map(|frame| {
            let reported = match frame.kind {
                FrameKind::Machine(_) => format!("machine:{}", frame.unit),
                FrameKind::Task(_) | FrameKind::Def => frame.unit.clone(),
            };
            (reported == task).then(|| frame.state.clone()).flatten()
        });
        PauseCommon {
            run_id: self.run_id.clone(),
            step,
            task,
            state,
            source: span,
            recording_offset: 0,
        }
    }

    /// Build a pause for one exact dynamic frame while preserving the unit
    /// name exposed by the host contract (spec sections 7.1 and 10.2).
    fn common_for_frame(&self, frame_id: u64, task: String, span: Span) -> PauseCommon {
        let mut common = self.common_for(task, span);
        common.state = self
            .frames
            .iter()
            .find(|frame| frame.id == frame_id)
            .and_then(|frame| frame.state.clone());
        common
    }

    /// The module alias a qualified unit name came from (spec section 3.9).
    fn module_of(&self, unit: &str) -> String {
        match unit.rsplit_once('.') {
            Some((alias, _))
                if alias == "std" || self.ir.modules.iter().any(|m| m.alias == alias) =>
            {
                alias.to_string()
            }
            _ => String::new(),
        }
    }

    /// The capability names a unit of `module` can see: the root's `needs`,
    /// or the inner names of the module's `with` mapping.
    fn capabilities_of(&self, module: &str) -> Vec<String> {
        if module.is_empty() {
            return self.ir.needs.iter().map(|n| n.name.clone()).collect();
        }
        self.ir
            .modules
            .iter()
            .find(|m| m.alias == module)
            .map(|m| m.mapping.iter().map(|x| x.inner.clone()).collect())
            .unwrap_or_default()
    }

    fn env_for(&self, module: &str) -> Env {
        Env::new()
            .with_inputs(self.inputs.clone())
            .with_capabilities(self.capabilities_of(module))
    }

    /// The root program's name for a capability the current unit calls
    /// (spec section 3.9: a library reaches only what it was handed).
    fn outer_capability(&self, inner: &str) -> String {
        let module = self.frames.last().map(|f| f.module.as_str()).unwrap_or("");
        if module.is_empty() {
            return inner.to_string();
        }
        self.ir
            .modules
            .iter()
            .find(|m| m.alias == module)
            .and_then(|m| m.mapping.iter().find(|x| x.inner == inner))
            .map_or_else(|| inner.to_string(), |x| x.outer.clone())
    }

    /// Resolve a unit name as written: in the current module first, then as
    /// given, then in the prelude (spec section 3.9).
    fn resolve(&self, name: &str) -> Option<Unit<'a>> {
        let module = self
            .frames
            .last()
            .map(|f| f.module.clone())
            .unwrap_or_default();
        let mut candidates = Vec::new();
        if !module.is_empty() && !name.contains('.') {
            candidates.push(format!("{module}.{name}"));
        }
        candidates.push(name.to_string());
        if !name.contains('.') {
            candidates.push(format!("std.{name}"));
        } else if let Some(bare) = name.strip_prefix("std.") {
            candidates.push(bare.to_string());
        }
        let ir = self.ir;
        candidates.iter().find_map(|candidate| {
            ir.judgment(candidate)
                .map(Unit::Judgment)
                .or_else(|| ir.defs.iter().find(|d| d.name == *candidate).map(Unit::Def))
                .or_else(|| ir.task(candidate).map(Unit::Task))
                .or_else(|| ir.machine(candidate).map(Unit::Machine))
        })
    }

    /* ---------------------------------------------------------------------- */
    /* Pauses, errors and the clock                                            */
    /* ---------------------------------------------------------------------- */

    /// Reach a pause. `Ok` carries the host's answer when the log already had
    /// it; `Err` unwinds because the host is now looking at the pause.
    fn pause(&mut self, pause: Pause) -> Exec<Resume> {
        match self.driver.pause(pause.clone()) {
            Ok(PauseProgress::Resume(resume, paused)) => {
                for frame in &mut self.frames {
                    if let FrameKind::Task(task) | FrameKind::Machine(task) = &mut frame.kind {
                        task.paused_ms += paused;
                    }
                }
                Ok(resume)
            }
            Ok(PauseProgress::Surface) => Err(Halt::pause(pause)),
            Ok(PauseProgress::Abort) => Err(self.end_with(Pause::Stopped {
                common: self.common(self.span),
                reason: crate::record::HOST_ABORT_REASON.to_string(),
            })),
            Err(error) => Err(self.end_with_error(error)),
        }
    }

    /// Stop at a host-cancellation boundary, whether live or replayed (spec
    /// sections 9.6, 10.3 and 10.4).
    fn cancellation_boundary(&mut self) -> Exec<()> {
        match self.driver.abort_boundary(self.span) {
            Ok(false) => Ok(()),
            Ok(true) => Err(self.end_with(Pause::Stopped {
                common: self.common(self.span),
                reason: crate::record::HOST_ABORT_REASON.to_string(),
            })),
            Err(error) => Err(self.end_with_error(error)),
        }
    }

    /// Raise a runtime error as an `error` pause (spec section 12). `Ok` only
    /// when the host retried, which only a retryable code allows.
    fn raise(&mut self, error: RuntimeError) -> Exec<Resume> {
        if error.code == RuntimeErrorCode::ReplayDiverged {
            self.driver.go_live();
        }
        let span = if error.span == Span::default() {
            self.span
        } else {
            error.span
        };
        let pause = Pause::Error {
            common: self.common(span),
            code: error.code,
            message: error.message,
            retryable: error.retryable,
        };
        self.pause(pause)
    }

    /// A pause that ends the run.
    fn end_with(&mut self, pause: Pause) -> Halt {
        match self.pause(pause.clone()) {
            Err(halt) => halt,
            Ok(_) => Halt::pause(pause),
        }
    }

    /// An error the run cannot continue past, whatever its code says.
    fn end_with_error(&mut self, error: RuntimeError) -> Halt {
        if error.code == RuntimeErrorCode::ReplayDiverged {
            self.driver.go_live();
        }
        let span = if error.span == Span::default() {
            self.span
        } else {
            error.span
        };
        let pause = Pause::Error {
            common: self.common(span),
            code: error.code,
            message: error.message,
            retryable: false,
        };
        // The driver may refuse (a recording that cannot be written); the
        // pause is still what the host sees.
        let _ = self.driver.pause(pause.clone());
        Halt::pause(pause)
    }

    /// Lift an effect's result to the statement level: a stashed pause takes
    /// precedence over the error that carried it, and any other error ends the
    /// run. Retryable errors are handled where they arise, at the effect, so
    /// that only the failed call is re-executed; one that reaches this level
    /// cannot be retried without re-running effects before it.
    fn lift<T>(&mut self, result: Result<T, RuntimeError>) -> Exec<T> {
        match result {
            Ok(value) => Ok(value),
            Err(error) => match self.driver.take_pending() {
                Some(pause) => Err(Halt::pause(pause)),
                None => Err(self.end_with_error(error)),
            },
        }
    }

    /// The other direction: a halt inside an effect becomes the sentinel error
    /// the evaluator unwinds with.
    fn halted(&mut self, halt: Halt) -> RuntimeError {
        let Halt::Pause(pause) = halt;
        self.driver.set_pending(*pause);
        interrupted()
    }

    /// The clock, through the recorded `draw` source (spec section 10.3).
    fn clock_ms(&mut self) -> Exec<u64> {
        let span = self.span;
        let text = match self.driver.draw_now(span) {
            Ok(text) => text,
            Err(error) => return Err(self.end_with_error(error)),
        };
        match iso8601_to_unix_millis(&text) {
            Ok(millis) => Ok(millis),
            Err(error) => Err(self.end_with_error(error)),
        }
    }

    /* ---------------------------------------------------------------------- */
    /* Budgets (spec section 7.1)                                               */
    /* ---------------------------------------------------------------------- */

    /// Pause while any task on the stack would be over its limit for `key`
    /// after `amount` more, innermost first. The pause names the task whose
    /// limit was hit, and extending raises only that one.
    fn check(&mut self, key: BudgetKey, amount: f64) -> Exec<()> {
        // The clock is read only when some task on the stack can run out of
        // minutes; a recording of a task without that budget carries no
        // clock draws beyond its start and end.
        let needs_clock = self.frames.iter().any(
                |f| matches!(&f.kind, FrameKind::Task(t) | FrameKind::Machine(t) if t.limit(BudgetKey::Minutes).is_some()),
        );
        if key == BudgetKey::Minutes && !needs_clock {
            return Ok(());
        }
        let now = if needs_clock { self.clock_ms()? } else { 0 };
        loop {
            let hit = self
                .frames
                .iter()
                .rev()
                .find_map(|frame| match &frame.kind {
                    FrameKind::Task(task) | FrameKind::Machine(task) => {
                        let over = |k: BudgetKey, extra: f64| {
                            task.limit(k)
                                .filter(|limit| task.used(k, now) + extra > *limit)
                                .map(|limit| {
                                    let reported = match frame.kind {
                                        FrameKind::Machine(_) => format!("machine:{}", frame.unit),
                                        FrameKind::Task(_) | FrameKind::Def => frame.unit.clone(),
                                    };
                                    (frame.id, reported, k, task.used(k, now), limit)
                                })
                        };
                        over(key, amount).or_else(|| over(BudgetKey::Minutes, 0.0))
                    }
                    FrameKind::Def => None,
                });
            let Some((frame_id, reported, key, used, limit)) = hit else {
                return Ok(());
            };
            let pause = Pause::Budget {
                common: self.common_for_frame(frame_id, reported, self.span),
                key,
                used,
                limit,
            };
            if let Resume::Extend { extend } = self.pause(pause)?
                && let Some(frame) = self.frames.iter_mut().find(|f| f.id == frame_id)
                && let FrameKind::Task(task) | FrameKind::Machine(task) = &mut frame.kind
            {
                for (key, limit) in extend {
                    task.set_limit(key, limit);
                }
            }
        }
    }

    /// Count `amount` of `key` against every task on the stack.
    fn add(&mut self, key: BudgetKey, amount: f64) {
        for frame in &mut self.frames {
            if let FrameKind::Task(task) | FrameKind::Machine(task) = &mut frame.kind {
                task.add(key, amount);
            }
        }
    }

    /// One iteration of a loop: the outermost loop of a task counts as a step.
    fn step(&mut self) -> Exec<()> {
        let outermost = matches!(
            self.frames.last(),
            Some(Frame {
                kind: FrameKind::Task(_),
                loop_depth: 1,
                ..
            })
        );
        if !outermost {
            return Ok(());
        }
        self.check(BudgetKey::Steps, 1.0)?;
        if let Some(Frame {
            kind: FrameKind::Task(task),
            ..
        }) = self.frames.last_mut()
        {
            task.steps += 1.0;
        }
        Ok(())
    }

    fn enter_loop(&mut self) {
        if let Some(frame) = self.frames.last_mut() {
            frame.loop_depth += 1;
        }
    }

    fn exit_loop(&mut self) {
        if let Some(frame) = self.frames.last_mut() {
            frame.loop_depth = frame.loop_depth.saturating_sub(1);
        }
    }

    /* ---------------------------------------------------------------------- */
    /* Statements                                                              */
    /* ---------------------------------------------------------------------- */

    fn eval(&mut self, env: &mut Env, expr: &Expr) -> Exec<Value> {
        let profile = self.profile;
        let result = {
            let mut evaluator = Evaluator::new(self, profile);
            evaluator.eval(expr, env)
        };
        self.lift(result)
    }

    fn eval_text(&mut self, env: &mut Env, expr: &Expr) -> Exec<String> {
        self.eval(env, expr).map(|v| v.to_text())
    }

    fn truthy(&mut self, env: &mut Env, expr: &Expr) -> Exec<bool> {
        self.eval(env, expr).map(|v| v.is_truthy())
    }

    fn eval_number(&mut self, env: &mut Env, expr: &Expr, what: &str) -> Exec<f64> {
        let value = self.eval(env, expr)?;
        match value.as_number() {
            Some(n) => Ok(n),
            None => {
                let error = Value::type_error(&format!("a number for {what}"), &value);
                Err(self.end_with_error(error))
            }
        }
    }

    /// Run a statement list in the current scope.
    fn exec_block(&mut self, env: &mut Env, stmts: &'a [Stmt]) -> Exec<Flow> {
        let mut ready: BTreeMap<u32, BTreeMap<String, Value>> = BTreeMap::new();
        for (i, _) in stmts.iter().enumerate() {
            match self.exec_stmt(env, stmts, i, &mut ready)? {
                Flow::Next => {}
                other => return Ok(other),
            }
        }
        Ok(Flow::Next)
    }

    /// Run a statement list as a block: its variables are its own (spec 5.3).
    fn exec_body(&mut self, env: &mut Env, stmts: &'a [Stmt]) -> Exec<Flow> {
        env.push_scope();
        let result = self.exec_block(env, stmts);
        env.pop_scope();
        result
    }

    fn exec_stmt(
        &mut self,
        env: &mut Env,
        stmts: &'a [Stmt],
        index: usize,
        ready: &mut BTreeMap<u32, BTreeMap<String, Value>>,
    ) -> Exec<Flow> {
        let stmt = &stmts[index];
        self.span = stmt_span(stmt);
        match stmt {
            Stmt::Assign {
                root,
                path,
                value: Expr::Judge(judge),
                ..
            } if path.is_empty() => {
                // Spec 6.6: the first judgment of a request group reached in a
                // statement list sends the whole group; the others take their
                // answers when execution reaches them.
                let group = judge.request_group;
                let value = match ready.get_mut(&group).and_then(|m| m.remove(root)) {
                    Some(value) => value,
                    None => {
                        let asks: Vec<Ask<'_>> = stmts[index..]
                            .iter()
                            .filter_map(|s| match s {
                                Stmt::Assign {
                                    root,
                                    path,
                                    value: Expr::Judge(j),
                                    ..
                                } if path.is_empty() && j.request_group == group => Some(Ask {
                                    name: root.as_str(),
                                    judge: j,
                                }),
                                _ => None,
                            })
                            .collect();
                        let result = self.ask(env, &asks, None);
                        let mut values = self.lift(result)?;
                        let value = values.remove(root).unwrap_or(Value::None);
                        ready.insert(group, values);
                        value
                    }
                };
                env.assign(root, value);
                Ok(Flow::Next)
            }
            Stmt::Assign {
                root, path, value, ..
            } => {
                let value = self.eval(env, value)?;
                if path.is_empty() {
                    env.assign(root, value);
                } else {
                    let result = env.assign_field(root, path, value);
                    self.lift(result)?;
                }
                Ok(Flow::Next)
            }
            Stmt::Expr { expr, .. } => {
                self.eval(env, expr)?;
                Ok(Flow::Next)
            }
            Stmt::If {
                branches,
                otherwise,
                ..
            } => {
                for branch in branches {
                    if self.truthy(env, &branch.test)? {
                        return self.exec_body(env, &branch.body);
                    }
                }
                match otherwise {
                    Some(body) => self.exec_body(env, body),
                    None => Ok(Flow::Next),
                }
            }
            Stmt::For {
                names,
                iterable,
                body,
                ..
            } => {
                let items = match self.eval(env, iterable)? {
                    Value::List(items) => items,
                    other => {
                        let error = Value::type_error("a list to iterate", &other);
                        return Err(self.end_with_error(error));
                    }
                };
                self.enter_loop();
                let mut result = Ok(Flow::Next);
                for item in items {
                    if let Err(halt) = self.step() {
                        result = Err(halt);
                        break;
                    }
                    env.push_scope();
                    let bound = bind_names(env, names, item);
                    let iteration = match self.lift(bound) {
                        Ok(()) => self.exec_block(env, body),
                        Err(halt) => Err(halt),
                    };
                    env.pop_scope();
                    match iteration {
                        Ok(Flow::Break) => break,
                        Ok(Flow::Return(value)) => {
                            result = Ok(Flow::Return(value));
                            break;
                        }
                        Ok(Flow::Next | Flow::Continue) => {}
                        Err(halt) => {
                            result = Err(halt);
                            break;
                        }
                    }
                }
                self.exit_loop();
                result
            }
            Stmt::Loop { max, body, .. } => {
                self.enter_loop();
                let mut result = Ok(Flow::Next);
                let mut i = 0.0;
                while i < *max {
                    if let Err(halt) = self.step() {
                        result = Err(halt);
                        break;
                    }
                    match self.exec_body(env, body) {
                        Ok(Flow::Break) => break,
                        Ok(Flow::Return(value)) => {
                            result = Ok(Flow::Return(value));
                            break;
                        }
                        Ok(Flow::Next | Flow::Continue) => {}
                        Err(halt) => {
                            result = Err(halt);
                            break;
                        }
                    }
                    i += 1.0;
                }
                self.exit_loop();
                result
            }
            Stmt::Until {
                test,
                verify,
                max,
                body,
                ..
            } => {
                // Spec 5.5: the condition is tested before each iteration and
                // once more after the last; spec 7.4: a `verify` condition
                // that becomes true proves the task's goal.
                self.enter_loop();
                let mut result = Ok(Flow::Next);
                let mut i = 0.0;
                loop {
                    if *verify && let Some(task) = self.task_frame_mut() {
                        task.verify_reached = true;
                    }
                    match self.truthy(env, test) {
                        Ok(true) => {
                            if *verify && let Some(task) = self.task_frame_mut() {
                                task.verified = true;
                            }
                            break;
                        }
                        Ok(false) => {}
                        Err(halt) => {
                            result = Err(halt);
                            break;
                        }
                    }
                    if i >= *max {
                        break;
                    }
                    if let Err(halt) = self.step() {
                        result = Err(halt);
                        break;
                    }
                    match self.exec_body(env, body) {
                        Ok(Flow::Break) => break,
                        Ok(Flow::Return(value)) => {
                            result = Ok(Flow::Return(value));
                            break;
                        }
                        Ok(Flow::Next | Flow::Continue) => {}
                        Err(halt) => {
                            result = Err(halt);
                            break;
                        }
                    }
                    i += 1.0;
                }
                self.exit_loop();
                result
            }
            Stmt::Gate {
                risk,
                confidence,
                done,
                arms,
                span,
            } => self.exec_gate(
                env,
                risk.as_ref(),
                confidence.as_ref(),
                done.as_ref(),
                arms,
                *span,
            ),
            Stmt::Shape {
                target,
                strict,
                fields,
                ..
            } => {
                let mut record = BTreeMap::new();
                for field in fields {
                    let value = self.eval(env, &field.value)?;
                    let value = match field.max {
                        Some(max) => self.cap(env, field, value, max, *strict)?,
                        None => value,
                    };
                    record.insert(field.name.clone(), value);
                }
                env.assign(target, Value::Record(record));
                Ok(Flow::Next)
            }
            Stmt::Return { value, .. } => {
                let value = match value {
                    Some(expr) => self.eval(env, expr)?,
                    None => Value::None,
                };
                Ok(Flow::Return(value))
            }
            Stmt::Continue { .. } => Ok(Flow::Continue),
            Stmt::Break { .. } => Ok(Flow::Break),
            Stmt::Stop { reason, span } => {
                let reason = self.eval_text(env, reason)?;
                Err(self.stop(reason, *span))
            }
            Stmt::Escalate { reason, span } => {
                let reason = self.eval_text(env, reason)?;
                self.pause(Pause::Escalate {
                    common: self.common(*span),
                    reason,
                    context: BTreeMap::new(),
                })?;
                Ok(Flow::Next)
            }
        }
    }

    fn stop(&mut self, reason: String, span: Span) -> Halt {
        let pause = Pause::Stopped {
            common: self.common(span),
            reason,
        };
        self.end_with(pause)
    }

    /// A gate (spec section 7.6): the verdict, in the order written there,
    /// then the arm for it or the arm's default.
    fn exec_gate(
        &mut self,
        env: &mut Env,
        risk: Option<&Expr>,
        confidence: Option<&Expr>,
        done: Option<&Expr>,
        arms: &'a [jevscript_ir::GateArm],
        span: Span,
    ) -> Exec<Flow> {
        let (Some(risk), Some(confidence)) = (risk, confidence) else {
            let error = RuntimeError::new(
                RuntimeErrorCode::TypeError,
                "a gate needs both `risk` and `confidence`",
            )
            .at(span);
            return Err(self.end_with_error(error));
        };
        let risk = self.eval_number(env, risk, "`gate risk`")?;
        let confidence = self.eval_number(env, confidence, "`gate confidence`")?;
        let done = match done {
            Some(expr) => Some(self.eval_number(env, expr, "`gate done`")?),
            None => None,
        };
        let thresholds = self
            .task_frame()
            .map(|f| f.thresholds.clone())
            .unwrap_or_default();
        let mut verdict = Verdict::Proceed;
        if thresholds.stop_confidence.is_some_and(|t| confidence < t) {
            verdict = Verdict::Stop;
        } else if thresholds.risk_confirm.is_some_and(|t| risk >= t) {
            verdict = Verdict::Confirm;
        } else if thresholds.min_confidence.is_some_and(|t| confidence < t) {
            verdict = Verdict::Escalate;
        }
        if let (Some(done), Some(threshold)) = (done, thresholds.done)
            && done >= threshold
        {
            verdict = Verdict::Proceed;
            if let Some(task) = self.task_frame_mut() {
                task.gate_done = true;
            }
        }
        let arm = arms.iter().find(|arm| arm.verdict == verdict);
        match arm {
            Some(arm) => match self.exec_body(env, &arm.body)? {
                Flow::Next => {}
                other => return Ok(other),
            },
            None => match verdict {
                Verdict::Proceed => {}
                Verdict::Confirm => {
                    let mut context = BTreeMap::from([
                        ("risk".to_string(), Value::Number(risk)),
                        ("confidence".to_string(), Value::Number(confidence)),
                    ]);
                    if let Some(done) = done {
                        context.insert("done".to_string(), Value::Number(done));
                    }
                    let resume = self.pause(Pause::Confirm {
                        common: self.common(span),
                        message: GATE_CONFIRM_MESSAGE.to_string(),
                        options: Pause::default_options(),
                        context,
                    })?;
                    let yes = matches!(&resume, Resume::Answer { answer, .. } if answer == "yes");
                    if !yes {
                        return Err(self.stop(GATE_DECLINED_REASON.to_string(), span));
                    }
                }
                Verdict::Escalate => {
                    self.pause(Pause::Escalate {
                        common: self.common(span),
                        reason: GATE_ESCALATE_REASON.to_string(),
                        context: BTreeMap::from([
                            ("risk".to_string(), Value::Number(risk)),
                            ("confidence".to_string(), Value::Number(confidence)),
                        ]),
                    })?;
                }
                Verdict::Stop => return Err(self.stop(GATE_STOP_REASON.to_string(), span)),
            },
        }
        // Spec 7.6: `gate.done` ends a task that has no `verify`.
        let ends = self
            .task_frame()
            .is_some_and(|f| f.gate_done && !f.has_verify);
        if ends {
            return Ok(Flow::Return(Value::None));
        }
        Ok(Flow::Next)
    }

    /// Apply a `shape` field's cap (spec section 7.2).
    fn cap(
        &mut self,
        env: &mut Env,
        field: &ShapeField,
        value: Value,
        max: f64,
        strict: bool,
    ) -> Exec<Value> {
        let tokens = self.estimator.estimate(&value.to_text());
        if tokens as f64 <= max {
            return Ok(value);
        }
        match &field.policy {
            ShapePolicy::Focus { on } => {
                let purpose = self.eval_text(env, on)?;
                let result = self.focus_text(value.to_text(), purpose, max);
                self.lift(result).map(Value::Text)
            }
            ShapePolicy::Head | ShapePolicy::Tail => {
                if strict {
                    let error = RuntimeError::new(
                        RuntimeErrorCode::StateTooLarge,
                        format!(
                            "`shape strict` field `{}` is {tokens} tokens, over its cap of {max}",
                            field.name
                        ),
                    )
                    .at(field.span);
                    return Err(self.end_with_error(error));
                }
                let tail = matches!(field.policy, ShapePolicy::Tail);
                let kept = truncate_value(&value, max as u64, tail, self.estimator.as_ref());
                let warning = Event::Warning {
                    code: TRUNCATED_WARNING.to_string(),
                    message: format!(
                        "shape field `{}` was cut from {tokens} tokens to its cap of {max} ({})",
                        field.name,
                        if tail { "tail" } else { "head" }
                    ),
                    source: field.span,
                };
                if let Err(error) = self.driver.emit(warning, field.span) {
                    return Err(self.end_with_error(error));
                }
                Ok(kept)
            }
        }
    }

    /* ---------------------------------------------------------------------- */
    /* Judgments (spec section 6)                                               */
    /* ---------------------------------------------------------------------- */

    /// Ask a batch of judgments over `env` and return their values by name.
    /// `params` is the parameter list of a named judgment, whose state is
    /// every parameter in full (spec section 6.9); `None` for an inline
    /// group, whose state is exactly its subjects.
    fn ask(
        &mut self,
        env: &mut Env,
        asks: &[Ask<'_>],
        params: Option<&[String]>,
    ) -> Result<BTreeMap<String, Value>, RuntimeError> {
        let plan = match params {
            Some(params) => state::build_judgment_request(asks, params, env, self.profile)?,
            None => state::build_request(asks, env, self.profile)?,
        };
        let sample_run = self.sample_run;
        let mut answers = Vec::new();
        for request in &plan.requests {
            // Spec 6.11: only a `pick` or `rate` that samples marks the
            // answers event, and only the request that carries it.
            let sampled = request.questions.iter().any(|question| {
                let name = question.id().split('[').next().unwrap_or_default();
                plan.asks.iter().any(|ask| {
                    ask.name == name
                        && !matches!(ask.kind, PlannedKind::Feels)
                        && (sample_run || ask.sample)
                })
            });
            let response = self.jev_request(request, sampled)?;
            answers.extend(response.answers);
        }
        let span = self.span;
        let driver: &mut Driver<'c> = self.driver;
        let mut draw = || driver.draw_random(span);
        let answered = answer::answers_to_values(&plan, &answers, sample_run, &mut draw)?;
        Ok(answered.values)
    }

    /// One request, charged as one call, retried on the host's say-so.
    fn jev_request(
        &mut self,
        request: &JevRequest,
        sampled: bool,
    ) -> Result<JevResponse, RuntimeError> {
        loop {
            if let Err(halt) = self.check(BudgetKey::Calls, 1.0) {
                return Err(self.halted(halt));
            }
            match self.driver.jev(request, sampled, self.span) {
                Ok(response) => {
                    let tokens = response.usage.tokens;
                    let usd = response
                        .usage
                        .usd
                        .unwrap_or_else(|| self.profile.usd_for(tokens));
                    self.usage.calls += 1;
                    self.usage.tokens += tokens;
                    self.usage.usd += usd;
                    self.add(BudgetKey::Calls, 1.0);
                    self.add(BudgetKey::Usd, usd);
                    if let Err(halt) = self.cancellation_boundary() {
                        return Err(self.halted(halt));
                    }
                    // `usd` is known only now, and `minutes` ran while the
                    // request was out (spec 7.1, 9.6); both are checked
                    // before the next effect.
                    if let Err(halt) = self.check(BudgetKey::Usd, 0.0) {
                        return Err(self.halted(halt));
                    }
                    return Ok(response);
                }
                Err(error) => {
                    if let Err(halt) = self.cancellation_boundary() {
                        return Err(self.halted(halt));
                    }
                    if error.retryable {
                        match self.raise(error) {
                            Ok(_) => {
                                self.after_blocking()?;
                                continue;
                            }
                            Err(halt) => return Err(self.halted(halt)),
                        }
                    }
                    return Err(error);
                }
            }
        }
    }

    /// Call a named judgment (spec section 6.7): one request over exactly its
    /// parameters, filled from a record or from named arguments.
    fn call_judgment(
        &mut self,
        judgment: &'a jevscript_ir::Judgment,
        args: CallArgs,
    ) -> Result<Value, RuntimeError> {
        let mut env = Env::new().with_inputs(self.inputs.clone());
        let by_record = match (args.positional.as_slice(), args.named.is_empty()) {
            ([Value::Record(fields)], true) => Some(fields.clone()),
            _ => None,
        };
        for (i, param) in judgment.params.iter().enumerate() {
            let value = match &by_record {
                Some(fields) => fields.get(param).cloned(),
                None => args
                    .named
                    .get(param)
                    .cloned()
                    .or_else(|| args.positional.get(i).cloned()),
            };
            let Some(value) = value else {
                return Err(RuntimeError::new(
                    RuntimeErrorCode::TypeError,
                    format!("judgment `{}` needs a value for `{param}`", judgment.name),
                ));
            };
            env.define(param.clone(), value);
        }
        let asks: Vec<Ask<'_>> = judgment
            .results
            .iter()
            .map(|result| Ask {
                name: result.name.as_str(),
                judge: &result.question,
            })
            .collect();
        let values = self.ask(&mut env, &asks, Some(&judgment.params))?;
        // Log lines run once the answers are in, each seeing the results
        // written above it (spec section 5.8).
        let profile = self.profile;
        let mut bound = 0;
        for log in &judgment.logs {
            let after = (log.after as usize).min(judgment.results.len());
            for result in &judgment.results[bound.min(after)..after] {
                let value = values.get(&result.name).cloned().unwrap_or(Value::None);
                env.define(result.name.clone(), value);
            }
            bound = bound.max(after);
            Evaluator::new(self, profile).eval(&log.log, &mut env)?;
        }
        Ok(Value::Record(values))
    }

    /// `focus` (spec section 7.3), executed directly with the semantics of
    /// the prelude def `focus_impl`: chunk, ask relevance per chunk in one
    /// request, keep what passes, and recurse until the text fits. Like the
    /// def, it pauses with `recursion_limit` if the text will not shrink.
    fn focus_text(
        &mut self,
        text: String,
        purpose: String,
        limit: f64,
    ) -> Result<String, RuntimeError> {
        let mut text = text;
        let span = self.span;
        for _ in 0..RECURSION_LIMIT {
            if self.estimator.estimate(&text) as f64 <= limit {
                return Ok(text);
            }
            let chunks = chunk(&text, FOCUS_CHUNK_TOKENS, self.estimator.as_ref());
            let judge = Judge {
                each: true,
                subject: Subject {
                    root: "chunks".to_string(),
                    path: Vec::new(),
                    state_path: "chunks".to_string(),
                    span,
                },
                verb: JudgeVerb::Feels {
                    condition: Expr::Text {
                        parts: vec![TextPart::Literal {
                            value: format!("contains information relevant to: {purpose}"),
                        }],
                        span,
                    },
                },
                detail: None,
                request_group: 0,
                span,
            };
            let mut env = Env::new();
            env.define(
                "chunks",
                Value::List(chunks.iter().cloned().map(Value::Text).collect()),
            );
            let asks = [Ask {
                name: "keep",
                judge: &judge,
            }];
            let values = self.ask(&mut env, &asks, None)?;
            let keep: Vec<f64> = match values.get("keep") {
                Some(Value::List(items)) => items.iter().filter_map(Value::as_number).collect(),
                _ => Vec::new(),
            };
            let mut kept: Vec<&String> = chunks
                .iter()
                .zip(&keep)
                .filter(|(_, p)| **p > FOCUS_KEEP_ABOVE)
                .map(|(c, _)| c)
                .collect();
            if kept.is_empty() {
                let mut order: Vec<usize> = (0..chunks.len().min(keep.len())).collect();
                order.sort_by(|a, b| {
                    keep[*b]
                        .partial_cmp(&keep[*a])
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
                kept = order
                    .into_iter()
                    .take(FOCUS_TOP)
                    .map(|i| &chunks[i])
                    .collect();
            }
            text = kept
                .iter()
                .map(|c| c.as_str())
                .collect::<Vec<_>>()
                .join("\n");
        }
        Err(RuntimeError::new(
            RuntimeErrorCode::RecursionLimit,
            format!(
                "`focus` could not reduce the text to {limit} tokens in {RECURSION_LIMIT} passes"
            ),
        )
        .at(span))
    }

    /* ---------------------------------------------------------------------- */
    /* Defs and tasks as calls (spec sections 7 and 8)                          */
    /* ---------------------------------------------------------------------- */

    fn bind_params(
        &mut self,
        env: &mut Env,
        unit: &str,
        params: &[DefParam],
        args: CallArgs,
    ) -> Result<(), RuntimeError> {
        let CallArgs { positional, named } = args;
        let mut positional = positional.into_iter();
        for param in params {
            let value = match named.get(&param.name) {
                Some(value) => Some(value.clone()),
                None => positional.next(),
            };
            let value = match (value, &param.default) {
                (Some(value), _) => value,
                (None, Some(default)) => {
                    let profile = self.profile;
                    let mut evaluator = Evaluator::new(self, profile);
                    evaluator.eval(default, env)?
                }
                (None, None) => {
                    return Err(RuntimeError::new(
                        RuntimeErrorCode::TypeError,
                        format!("`{unit}` needs an argument `{}`", param.name),
                    ));
                }
            };
            env.define(param.name.clone(), value);
        }
        Ok(())
    }

    fn call_def(&mut self, def: &'a Def, args: CallArgs) -> Result<Value, RuntimeError> {
        let depth = self
            .frames
            .iter()
            .filter(|f| matches!(f.kind, FrameKind::Def))
            .count();
        if depth >= RECURSION_LIMIT {
            return Err(RuntimeError::new(
                RuntimeErrorCode::RecursionLimit,
                format!("`{}` recursed past {RECURSION_LIMIT} frames", def.name),
            ));
        }
        let module = self.module_of(&def.name);
        let mut env = self.env_for(&module);
        self.bind_params(&mut env, &def.name, &def.params, args)?;
        let id = self.allocate_frame_id();
        self.frames.push(Frame {
            id,
            unit: def.name.clone(),
            module,
            kind: FrameKind::Def,
            state: None,
            loop_depth: 0,
        });
        let result = self.exec_block(&mut env, &def.body);
        self.frames.pop();
        match result {
            Ok(Flow::Return(value)) => Ok(value),
            Ok(_) => Ok(Value::None),
            Err(halt) => Err(self.halted(halt)),
        }
    }

    fn call_task(&mut self, task: &'a Task, args: CallArgs) -> Result<Value, RuntimeError> {
        let module = self.module_of(&task.name);
        let mut env = self.env_for(&module);
        self.bind_params(&mut env, &task.name, &task.params, args)?;
        if let Err(halt) = self.enter_task(task, module) {
            return Err(self.halted(halt));
        }
        let result = self.exec_block(&mut env, &task.body);
        self.frames.pop();
        match result {
            Ok(Flow::Return(value)) => Ok(value),
            Ok(_) => Ok(Value::None),
            Err(halt) => Err(self.halted(halt)),
        }
    }

    /* ---------------------------------------------------------------------- */
    /* Capabilities (spec section 9) and the trail (spec section 7.5)            */
    /* ---------------------------------------------------------------------- */

    /// One adapter call, retried on the host's say-so, with its step record.
    fn adapter_call(
        &mut self,
        capability: &str,
        verb: &str,
        args: &CallArgs,
        step_target: &str,
        step_args: &CallArgs,
    ) -> Result<Value, RuntimeError> {
        loop {
            match self.driver.call(capability, verb, args, self.span) {
                Ok(value) => {
                    self.note_step(verb, step_target, step_args, None)?;
                    self.after_blocking()?;
                    return Ok(value);
                }
                Err(error) => {
                    if let Err(halt) = self.cancellation_boundary() {
                        return Err(self.halted(halt));
                    }
                    if error.retryable {
                        match self.raise(error) {
                            Ok(_) => {
                                self.after_blocking()?;
                                continue;
                            }
                            Err(halt) => return Err(self.halted(halt)),
                        }
                    }
                    // A divergence is not something the call did; the step
                    // record would only diverge again.
                    if error.code != RuntimeErrorCode::ReplayDiverged {
                        self.note_step(verb, step_target, step_args, Some(StepOutcome::Error))?;
                    }
                    return Err(error);
                }
            }
        }
    }

    /// Write a step record for a call the runtime just made (spec 7.5). Its
    /// `changed` waits for the next observation unless the call failed.
    fn note_step(
        &mut self,
        action: &str,
        target: &str,
        args: &CallArgs,
        outcome: Option<StepOutcome>,
    ) -> Result<(), RuntimeError> {
        let args = truncate_text(
            &args_text(args),
            TRAIL_ARGS_TOKENS,
            false,
            self.estimator.as_ref(),
        );
        let Some(frame) = self.task_frame_mut() else {
            return Ok(());
        };
        let record = StepRecord {
            step: frame.next_step,
            action: action.to_string(),
            target: target.to_string(),
            args,
            changed: false,
            outcome: outcome.unwrap_or(StepOutcome::Observed),
        };
        frame.next_step += 1;
        match outcome {
            Some(_) => {
                frame.trail.push(record.clone());
                let span = self.span;
                self.driver.emit(Event::Step { record }, span)
            }
            None => {
                frame.pending_steps.push(record);
                Ok(())
            }
        }
    }

    /// An observation arrived: settle every pending step's `changed` by
    /// hashing the observation's text form against the previous one for the
    /// same handle, and sum whatever the adapter reported spending.
    fn observed(
        &mut self,
        handle: &Handle,
        observation: &Observation,
    ) -> Result<Value, RuntimeError> {
        let value = serde_json::to_value(observation)
            .map(|json| Value::from_json(&json))
            .unwrap_or(Value::None);
        let hash = short_hash(&value.to_text());
        let key = format!("{}:{}", handle.capability, handle.id);
        let changed = self.last_observed.get(&key) != Some(&hash);
        self.last_observed.insert(key, hash);
        if let Some(usage) = observation.fields.get("usage") {
            self.usage.adapter_usd += usage.get("usd").and_then(|v| v.as_f64()).unwrap_or(0.0);
            self.usage.adapter_tokens += usage.get("tokens").and_then(|v| v.as_u64()).unwrap_or(0);
        }
        let span = self.span;
        let pending = self
            .task_frame_mut()
            .map(|frame| std::mem::take(&mut frame.pending_steps))
            .unwrap_or_default();
        for mut record in pending {
            record.changed = changed;
            record.outcome = if changed {
                StepOutcome::Observed
            } else {
                StepOutcome::NoChange
            };
            if let Some(frame) = self.task_frame_mut() {
                frame.trail.push(record.clone());
            }
            self.driver.emit(Event::Step { record }, span)?;
        }
        Ok(value)
    }

    /// The clock ran while the runtime blocked on the world (spec 9.6), so
    /// `minutes` is checked before anything else happens (spec 7.1).
    fn after_blocking(&mut self) -> Result<(), RuntimeError> {
        if let Err(halt) = self.cancellation_boundary() {
            return Err(self.halted(halt));
        }
        match self.check(BudgetKey::Minutes, 0.0) {
            Ok(()) => Ok(()),
            Err(halt) => Err(self.halted(halt)),
        }
    }

    fn observe_handle(&mut self, capability: &str, handle: &Handle) -> Result<Value, RuntimeError> {
        loop {
            match self.driver.observe(capability, handle, self.span) {
                Ok(observation) => {
                    let value = self.observed(handle, &observation)?;
                    self.after_blocking()?;
                    return Ok(value);
                }
                Err(error) => {
                    if let Err(halt) = self.cancellation_boundary() {
                        return Err(self.halted(halt));
                    }
                    if error.retryable {
                        match self.raise(error) {
                            Ok(_) => {
                                self.after_blocking()?;
                                continue;
                            }
                            Err(halt) => return Err(self.halted(halt)),
                        }
                    }
                    return Err(error);
                }
            }
        }
    }

    fn call_capability_verb(
        &mut self,
        target: CallTarget,
        verb: &str,
        args: CallArgs,
    ) -> Result<Value, RuntimeError> {
        let inner = target.capability().to_string();
        let outer = self.outer_capability(&inner);
        let Some(need) = self.ir.needs.iter().find(|n| n.name == outer) else {
            return Err(RuntimeError::new(
                RuntimeErrorCode::TypeError,
                format!("`{inner}` is not a declared capability"),
            ));
        };
        let kind = need.kind;
        if !crate::capability::CapabilityKind::from(kind).accepts(verb) {
            return Err(RuntimeError::new(
                RuntimeErrorCode::TypeError,
                format!("`{inner}` is {} and has no verb `{verb}`", kind_name(kind)),
            ));
        }
        let handle = match &target {
            CallTarget::Handle(handle) => Some(handle.clone()),
            CallTarget::Capability(_) => None,
        };
        let step_target = handle
            .as_ref()
            .map_or_else(|| inner.clone(), |h| h.id.clone());
        // A verb on a handle reaches the adapter with the handle first.
        let mut sent = args.clone();
        if let Some(handle) = &handle {
            sent.positional.insert(0, Value::Handle(handle.clone()));
        }
        match (kind, verb) {
            (CapabilityKind::Agent, "observe") => {
                let handle = self.need_handle(handle, &inner, verb)?;
                self.observe_handle(&outer, &handle)
            }
            (CapabilityKind::Agent, "wait") => {
                let handle = self.need_handle(handle, &inner, verb)?;
                let condition = args
                    .positional
                    .first()
                    .map_or_else(|| "idle".to_string(), Value::to_text);
                let timeout_minutes = args
                    .named
                    .get("minutes")
                    .or_else(|| args.positional.get(1))
                    .and_then(Value::as_number)
                    .unwrap_or(0.0);
                self.waiting = Some((outer.clone(), handle.clone()));
                let pause = Pause::Waiting {
                    common: self.common(self.span),
                    on: outer.clone(),
                    condition,
                    timeout_minutes,
                };
                if let Err(halt) = self.pause(pause) {
                    return Err(self.halted(halt));
                }
                self.waiting = None;
                let observed = loop {
                    match self.driver.call(&outer, verb, &sent, self.span) {
                        Ok(value) => break value,
                        Err(error) => {
                            if let Err(halt) = self.cancellation_boundary() {
                                return Err(self.halted(halt));
                            }
                            if error.retryable {
                                match self.raise(error) {
                                    Ok(_) => {
                                        self.after_blocking()?;
                                        continue;
                                    }
                                    Err(halt) => return Err(self.halted(halt)),
                                }
                            }
                            return Err(error);
                        }
                    }
                };
                let observation: Observation = serde_json::from_value(observed.to_json())
                    .map_err(|error| {
                        RuntimeError::new(
                            RuntimeErrorCode::TypeError,
                            format!("`{inner}.wait` returned something other than an observation: {error}"),
                        )
                    })?;
                let value = self.observed(&handle, &observation)?;
                self.after_blocking()?;
                Ok(value)
            }
            (CapabilityKind::Agent, "spawn") => {
                let value = self.adapter_call(&outer, verb, &sent, &step_target, &args)?;
                match value {
                    Value::Handle(handle) => Ok(Value::Handle(Handle {
                        capability: inner,
                        ..handle
                    })),
                    other => Err(Value::type_error("a handle from `spawn`", &other)),
                }
            }
            (CapabilityKind::Person, "ask") => {
                let message = args
                    .positional
                    .first()
                    .map(Value::to_text)
                    .unwrap_or_default();
                let options = match args.named.get("options") {
                    Some(Value::List(items)) => items.iter().map(Value::to_text).collect(),
                    _ => Pause::default_options(),
                };
                let pause = Pause::Confirm {
                    common: self.common(self.span),
                    message,
                    options,
                    context: BTreeMap::new(),
                };
                match self.pause(pause) {
                    Ok(Resume::Answer { answer, text }) => {
                        Ok(Value::PauseResult(PauseResult { answer, text }))
                    }
                    Ok(_) => Ok(Value::None),
                    Err(halt) => Err(self.halted(halt)),
                }
            }
            (CapabilityKind::Person, "take_over") => {
                let pause = Pause::Escalate {
                    common: self.common(self.span),
                    reason: format!("{inner}.take_over"),
                    context: BTreeMap::new(),
                };
                match self.pause(pause) {
                    Ok(_) => Ok(Value::None),
                    Err(halt) => Err(self.halted(halt)),
                }
            }
            (CapabilityKind::Llm, "write") => {
                let instruction = args
                    .positional
                    .first()
                    .map(Value::to_text)
                    .unwrap_or_default();
                let using = args.named.get("using").cloned();
                loop {
                    if let Err(halt) = self.check(BudgetKey::Calls, 1.0) {
                        return Err(self.halted(halt));
                    }
                    match self
                        .driver
                        .generate(&outer, &instruction, using.as_ref(), self.span)
                    {
                        Ok((output, usage)) => {
                            self.usage.calls += 1;
                            self.usage.tokens += usage.tokens;
                            self.add(BudgetKey::Calls, 1.0);
                            self.note_step(verb, &step_target, &args, None)?;
                            self.after_blocking()?;
                            return Ok(Value::Text(output));
                        }
                        Err(error) => {
                            if let Err(halt) = self.cancellation_boundary() {
                                return Err(self.halted(halt));
                            }
                            if error.retryable {
                                match self.raise(error) {
                                    Ok(_) => {
                                        self.after_blocking()?;
                                        continue;
                                    }
                                    Err(halt) => return Err(self.halted(halt)),
                                }
                            }
                            return Err(error);
                        }
                    }
                }
            }
            (CapabilityKind::Tool, _) => {
                let value = self.adapter_call(&outer, verb, &sent, &step_target, &args)?;
                if let Some(signature) = need.signatures.iter().find(|s| s.name == verb)
                    && !returns_matches(signature.returns, &value)
                {
                    return Err(RuntimeError::new(
                        RuntimeErrorCode::TypeError,
                        format!(
                            "`{inner}.{verb}` is declared to return {} but the adapter returned {}",
                            return_name(signature.returns),
                            value.type_name()
                        ),
                    ));
                }
                Ok(value)
            }
            _ => {
                // `send`, `stop`, `notify`: a plain call with a step record.
                self.adapter_call(&outer, verb, &sent, &step_target, &args)
            }
        }
    }

    fn need_handle(
        &self,
        handle: Option<Handle>,
        capability: &str,
        verb: &str,
    ) -> Result<Handle, RuntimeError> {
        handle.ok_or_else(|| {
            RuntimeError::new(
                RuntimeErrorCode::TypeError,
                format!("`{verb}` is written on a handle, not on `{capability}` itself"),
            )
        })
    }
}

impl Effects for Interpreter<'_, '_> {
    fn judge(&mut self, judge: &Judge, env: &mut Env) -> Result<Value, RuntimeError> {
        let asks = [Ask {
            name: "judgment",
            judge,
        }];
        let mut values = self.ask(env, &asks, None)?;
        Ok(values.remove("judgment").unwrap_or(Value::None))
    }

    fn call_def(&mut self, name: &str, args: CallArgs) -> Result<Value, RuntimeError> {
        match self.resolve(name) {
            Some(Unit::Judgment(judgment)) => self.call_judgment(judgment, args),
            Some(Unit::Def(def)) => Interpreter::call_def(self, def, args),
            Some(Unit::Task(task)) => self.call_task(task, args),
            Some(Unit::Machine(machine)) => self.call_machine(machine, args),
            None => Err(RuntimeError::new(
                RuntimeErrorCode::TypeError,
                format!("`{name}` is not a def, judgment, task or machine of this program"),
            )),
        }
    }

    fn call_capability(
        &mut self,
        target: CallTarget,
        verb: &str,
        args: CallArgs,
    ) -> Result<Value, RuntimeError> {
        self.call_capability_verb(target, verb, args)
    }

    fn focus(&mut self, text: Value, purpose: String, max: f64) -> Result<Value, RuntimeError> {
        self.focus_text(text.to_text(), purpose, max)
            .map(Value::Text)
    }

    fn trail(&mut self, n: usize) -> Result<Value, RuntimeError> {
        let records = self
            .task_frame()
            .map(|frame| {
                let skip = frame.trail.len().saturating_sub(n);
                frame.trail[skip..].to_vec()
            })
            .unwrap_or_default();
        Ok(Value::List(
            records
                .iter()
                .map(|record| {
                    serde_json::to_value(record)
                        .map(|json| Value::from_json(&json))
                        .unwrap_or(Value::None)
                })
                .collect(),
        ))
    }

    fn draw_now(&mut self) -> Result<String, RuntimeError> {
        self.driver.draw_now(self.span)
    }

    fn draw_random(&mut self) -> Result<f64, RuntimeError> {
        self.driver.draw_random(self.span)
    }

    /// A `log` is a recorded event and nothing else (spec section 5.8): it is
    /// not a call, a step or a pause, so no budget sees it. In catch-up and
    /// replay it is checked against the log like a `warning` and not streamed
    /// to the host a second time.
    fn log(&mut self, mut record: LogRecord) -> Result<(), RuntimeError> {
        record.task = self.current_task();
        let source = record.source;
        self.driver.emit(record.into_event(), source)
    }
}

/// A Jev client lent to one standalone judgment run.
struct LentClient<'c>(&'c dyn crate::jev::JevClient);

impl crate::jev::JevClient for LentClient<'_> {
    fn send(&self, request: &JevRequest) -> Result<JevResponse, crate::jev::JevError> {
        self.0.send(request)
    }
}

/// Run a judgment's `log` lines when it runs alone (spec sections 5.8 and
/// 11.3), after its answers are in.
///
/// A log line is outside the question, so it may evaluate anything the
/// compiler accepts there: calls to defs and other judgments through
/// `client`, and `now()` and `random()` read live from the clock and a
/// clock-seeded source. There is no run, so the driver keeps its events in
/// memory only and the lines are returned instead of recorded.
///
/// # Errors
///
/// Whatever evaluating a line raises, as the error an `error` pause would
/// have carried in a run.
pub(crate) fn standalone_judgment_logs(
    ir: &Ir,
    profile: &Profile,
    client: &dyn crate::jev::JevClient,
    judgment: &jevscript_ir::Judgment,
    env: &mut Env,
    values: &BTreeMap<String, Value>,
) -> Result<Vec<LogRecord>, RuntimeError> {
    if judgment.logs.is_empty() {
        return Ok(Vec::new());
    }
    let mut driver = Driver::new(driver::DriverSetup {
        run_id: String::new(),
        log: Vec::new(),
        replaying: false,
        recorder: None,
        bindings: crate::capability::Bindings::new(),
        profile: profile.clone(),
        rng: crate::rng::Rng::from_clock(),
        execution: crate::record::ExecutionMetadata {
            ir: ir.clone(),
            profile: profile.clone(),
            sample: crate::rpc::Sample::On(false),
        },
        abort_signal: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    });
    driver.set_client(Box::new(LentClient(client)));
    let mut interpreter = Interpreter::new(
        ir,
        profile,
        &mut driver,
        String::new(),
        judgment.name.clone(),
        BTreeMap::new(),
        false,
    );
    let mut bound = 0;
    for log in &judgment.logs {
        let after = (log.after as usize).min(judgment.results.len());
        for result in &judgment.results[bound.min(after)..after] {
            let value = values.get(&result.name).cloned().unwrap_or(Value::None);
            env.define(result.name.clone(), value);
        }
        bound = bound.max(after);
        let evaluated = Evaluator::new(&mut interpreter, profile).eval(&log.log, env);
        if let Err(error) = evaluated {
            return Err(match interpreter.driver.take_pending() {
                Some(Pause::Error {
                    common,
                    code,
                    message,
                    retryable,
                }) => RuntimeError::new(code, message)
                    .retryable(retryable)
                    .at(common.source),
                Some(pause) => RuntimeError::new(
                    RuntimeErrorCode::TypeError,
                    format!(
                        "a judgment run alone cannot raise a `{}` pause",
                        pause.kind().as_str()
                    ),
                )
                .at(pause.common().source),
                None => error,
            });
        }
    }
    Ok(driver
        .log()
        .iter()
        .filter_map(|event| match event {
            Event::Log {
                level,
                message,
                fields,
                task,
                source,
            } => Some(LogRecord {
                level: *level,
                message: message.full().cloned().unwrap_or_default(),
                fields: fields
                    .iter()
                    .filter_map(|(name, value)| Some((name.clone(), value.full()?.clone())))
                    .collect(),
                task: task.clone(),
                source: *source,
            }),
            _ => None,
        })
        .collect())
}

/* -------------------------------------------------------------------------- */
/* Helpers                                                                     */
/* -------------------------------------------------------------------------- */

impl From<CapabilityKind> for crate::capability::CapabilityKind {
    fn from(kind: CapabilityKind) -> Self {
        match kind {
            CapabilityKind::Agent => Self::Agent,
            CapabilityKind::Person => Self::Person,
            CapabilityKind::Llm => Self::Llm,
            CapabilityKind::Tool => Self::Tool,
        }
    }
}

/// The kind as the spec and the recording write it.
pub(crate) const fn kind_name(kind: CapabilityKind) -> &'static str {
    match kind {
        CapabilityKind::Agent => "agent",
        CapabilityKind::Person => "person",
        CapabilityKind::Llm => "llm",
        CapabilityKind::Tool => "tool",
    }
}

const fn return_name(returns: ReturnType) -> &'static str {
    match returns {
        ReturnType::Text => "text",
        ReturnType::Number => "number",
        ReturnType::Bool => "bool",
        ReturnType::List => "list",
        ReturnType::Record => "record",
        ReturnType::Handle => "handle",
        ReturnType::None => "none",
    }
}

/// Whether an adapter's return fits a declared signature (spec section 9.4).
fn returns_matches(returns: ReturnType, value: &Value) -> bool {
    match returns {
        ReturnType::Text => matches!(value, Value::Text(_)),
        ReturnType::Number => matches!(value, Value::Number(_) | Value::Prob(_)),
        ReturnType::Bool => matches!(value, Value::Bool(_)),
        ReturnType::List => matches!(value, Value::List(_)),
        ReturnType::Record => matches!(value, Value::Record(_)),
        ReturnType::Handle => matches!(value, Value::Handle(_)),
        ReturnType::None => matches!(value, Value::None),
    }
}

/// The text form of a call's arguments for a step record: positional values
/// then `name value` pairs, the way command form writes them.
fn args_text(args: &CallArgs) -> String {
    let mut parts: Vec<String> = args.positional.iter().map(Value::to_text).collect();
    parts.extend(
        args.named
            .iter()
            .map(|(name, value)| format!("{name} {}", value.to_text())),
    );
    parts.join(", ")
}

const fn stmt_span(stmt: &Stmt) -> Span {
    match stmt {
        Stmt::Assign { span, .. }
        | Stmt::Expr { span, .. }
        | Stmt::If { span, .. }
        | Stmt::For { span, .. }
        | Stmt::Loop { span, .. }
        | Stmt::Until { span, .. }
        | Stmt::Gate { span, .. }
        | Stmt::Shape { span, .. }
        | Stmt::Return { span, .. }
        | Stmt::Continue { span }
        | Stmt::Break { span }
        | Stmt::Stop { span, .. }
        | Stmt::Escalate { span, .. } => *span,
    }
}

/// Whether a task body declares `verify` (spec section 7.4).
pub(crate) fn declares_verify(body: &[Stmt]) -> bool {
    verify_expression(body).is_some()
}

/// The one `verify` condition a task declares (spec section 7.4), which is
/// read again at the end of the task to settle `verified`. It sits on an
/// `until` directly in the task body — the compiler rejects any other
/// placement — which is what keeps its condition in task scope for that
/// final read.
pub(crate) fn verify_expression(body: &[Stmt]) -> Option<&Expr> {
    body.iter().find_map(|stmt| match stmt {
        Stmt::Until {
            verify: true, test, ..
        } => Some(test),
        _ => None,
    })
}

/// Visit every statement in `stmts`, depth first. The visitor sees each
/// statement for the slice's lifetime, so it may keep what it finds.
pub(crate) fn walk_stmts<'s>(stmts: &'s [Stmt], visit: &mut impl FnMut(&'s Stmt)) {
    for stmt in stmts {
        visit(stmt);
        match stmt {
            Stmt::If {
                branches,
                otherwise,
                ..
            } => {
                for branch in branches {
                    walk_stmts(&branch.body, visit);
                }
                if let Some(body) = otherwise {
                    walk_stmts(body, visit);
                }
            }
            Stmt::For { body, .. } | Stmt::Loop { body, .. } | Stmt::Until { body, .. } => {
                walk_stmts(body, visit);
            }
            Stmt::Gate { arms, .. } => {
                for arm in arms {
                    walk_stmts(&arm.body, visit);
                }
            }
            _ => {}
        }
    }
}

/// Visit every expression in a statement, depth first.
fn walk_stmt_exprs(stmt: &Stmt, visit: &mut impl FnMut(&Expr)) {
    match stmt {
        Stmt::Assign { value, .. } => walk_expr(value, visit),
        Stmt::Expr { expr, .. } => walk_expr(expr, visit),
        Stmt::If { branches, .. } => {
            for branch in branches {
                walk_expr(&branch.test, visit);
            }
        }
        Stmt::For { iterable, .. } => walk_expr(iterable, visit),
        Stmt::Until { test, .. } => walk_expr(test, visit),
        Stmt::Gate {
            risk,
            confidence,
            done,
            ..
        } => {
            for expr in [risk, confidence, done].into_iter().flatten() {
                walk_expr(expr, visit);
            }
        }
        Stmt::Shape { fields, .. } => {
            for field in fields {
                walk_expr(&field.value, visit);
                if let ShapePolicy::Focus { on } = &field.policy {
                    walk_expr(on, visit);
                }
            }
        }
        Stmt::Return {
            value: Some(expr), ..
        }
        | Stmt::Stop { reason: expr, .. }
        | Stmt::Escalate { reason: expr, .. } => walk_expr(expr, visit),
        Stmt::Return { value: None, .. }
        | Stmt::Loop { .. }
        | Stmt::Continue { .. }
        | Stmt::Break { .. } => {}
    }
}

fn walk_expr(expr: &Expr, visit: &mut impl FnMut(&Expr)) {
    visit(expr);
    match expr {
        Expr::Text { parts, .. } => {
            for part in parts {
                if let TextPart::Interpolation { expr } = part {
                    walk_expr(expr, visit);
                }
            }
        }
        Expr::List { items, .. } => items.iter().for_each(|e| walk_expr(e, visit)),
        Expr::Record { fields, .. } => fields.iter().for_each(|f| walk_expr(&f.value, visit)),
        Expr::Field { target, .. } => walk_expr(target, visit),
        Expr::Index { target, index, .. } => {
            walk_expr(target, visit);
            walk_expr(index, visit);
        }
        Expr::Call { callee, args, .. } => {
            walk_expr(callee, visit);
            args.iter().for_each(|a| walk_expr(&a.value, visit));
        }
        Expr::Unary { operand, .. } => walk_expr(operand, visit),
        Expr::Binary { left, right, .. } => {
            walk_expr(left, visit);
            walk_expr(right, visit);
        }
        Expr::Is { target, .. } => walk_expr(target, visit),
        Expr::Comprehension {
            expr,
            iterable,
            test,
            ..
        } => {
            walk_expr(expr, visit);
            walk_expr(iterable, visit);
            if let Some(test) = test {
                walk_expr(test, visit);
            }
        }
        Expr::Focus { text, on, .. } => {
            walk_expr(text, visit);
            walk_expr(on, visit);
        }
        Expr::Log { value, fields, .. } => {
            walk_expr(value, visit);
            fields.iter().for_each(|f| walk_expr(&f.value, visit));
        }
        Expr::Judge(judge) => {
            for step in &judge.subject.path {
                if let jevscript_ir::SubjectStep::Index { index } = step {
                    walk_expr(index, visit);
                }
            }
        }
        Expr::Number { .. }
        | Expr::Bool { .. }
        | Expr::None { .. }
        | Expr::Name { .. }
        | Expr::Trail { .. } => {}
    }
}

/// Every verb the program calls on each root capability, for the bind-time
/// manifest check (spec section 9.4). Library bodies name their inner
/// capabilities, which are mapped to the root's through the module's `with`.
pub(crate) fn verbs_referenced(ir: &Ir) -> BTreeMap<String, BTreeSet<String>> {
    let mut verbs: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let module_of = |unit: &str| -> Option<&jevscript_ir::Module> {
        unit.rsplit_once('.')
            .and_then(|(alias, _)| ir.modules.iter().find(|m| m.alias == alias))
    };
    let mut collect = |unit: &str, body: &[Stmt]| {
        let module = module_of(unit);
        let caps: BTreeSet<String> = match module {
            Some(module) => module.mapping.iter().map(|m| m.inner.clone()).collect(),
            None => ir.needs.iter().map(|n| n.name.clone()).collect(),
        };
        let outer = |inner: &str| -> String {
            module
                .and_then(|m| m.mapping.iter().find(|x| x.inner == inner))
                .map_or_else(|| inner.to_string(), |x| x.outer.clone())
        };
        walk_stmts(body, &mut |stmt| {
            walk_stmt_exprs(stmt, &mut |expr| {
                let (callee, name) = match expr {
                    Expr::Call { callee, .. } => match callee.as_ref() {
                        Expr::Field { target, name, .. } => (target.as_ref(), name),
                        _ => return,
                    },
                    Expr::Field { target, name, .. } => (target.as_ref(), name),
                    _ => return,
                };
                if let Expr::Name { name: root, .. } = callee
                    && caps.contains(root)
                {
                    verbs.entry(outer(root)).or_default().insert(name.clone());
                }
            });
        });
    };
    for task in &ir.tasks {
        collect(&task.name, &task.body);
    }
    for def in &ir.defs {
        collect(&def.name, &def.body);
    }
    for machine in &ir.machines {
        for state in &machine.states {
            for transition in &state.transitions {
                collect(&machine.name, &transition.body);
            }
        }
    }
    verbs
}

/// Cut text to at most `max` tokens from the front, or from the back when
/// `tail`. Found by binary search over characters, since any estimator grows
/// with length.
pub(crate) fn truncate_text(
    text: &str,
    max: u64,
    tail: bool,
    estimator: &dyn TokenEstimator,
) -> String {
    if estimator.estimate(text) <= max {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let (mut lo, mut hi) = (0usize, chars.len());
    while lo < hi {
        let mid = lo + (hi - lo).div_ceil(2);
        let candidate: String = if tail {
            chars[chars.len() - mid..].iter().collect()
        } else {
            chars[..mid].iter().collect()
        };
        if estimator.estimate(&candidate) <= max {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    if tail {
        chars[chars.len() - lo..].iter().collect()
    } else {
        chars[..lo].iter().collect()
    }
}

/// Cut a value to `max` tokens (spec section 7.2): text by characters, a list
/// by whole items and a record by whole fields, from the front or the back.
/// Anything else is cut as its text form.
pub(crate) fn truncate_value(
    value: &Value,
    max: u64,
    tail: bool,
    estimator: &dyn TokenEstimator,
) -> Value {
    match value {
        Value::Text(text) => Value::Text(truncate_text(text, max, tail, estimator)),
        Value::List(items) => {
            let mut kept: Vec<Value> = Vec::new();
            let order: Vec<&Value> = if tail {
                items.iter().rev().collect()
            } else {
                items.iter().collect()
            };
            for item in order {
                let mut candidate = kept.clone();
                if tail {
                    candidate.insert(0, item.clone());
                } else {
                    candidate.push(item.clone());
                }
                if estimator.estimate(&Value::List(candidate.clone()).to_text()) > max {
                    break;
                }
                kept = candidate;
            }
            Value::List(kept)
        }
        Value::Record(fields) => {
            let mut kept: BTreeMap<String, Value> = BTreeMap::new();
            let order: Vec<(&String, &Value)> = if tail {
                fields.iter().rev().collect()
            } else {
                fields.iter().collect()
            };
            for (name, item) in order {
                let mut candidate = kept.clone();
                candidate.insert(name.clone(), item.clone());
                if estimator.estimate(&Value::Record(candidate.clone()).to_text()) > max {
                    break;
                }
                kept = candidate;
            }
            Value::Record(kept)
        }
        other => Value::Text(truncate_text(&other.to_text(), max, tail, estimator)),
    }
}
