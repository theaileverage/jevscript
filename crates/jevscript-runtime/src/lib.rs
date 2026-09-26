//! The reference Jevscript runtime.
//!
//! It executes the IR that `jevscript-compiler` emits, records every judgment,
//! generation, capability result and random draw, and replays a recording with
//! zero model calls. `spec/jevscript-language-specification.md` sections 9, 10
//! and 11 are the authority.
//!
//! The shape of the crate:
//!
//! - [`value`]: the runtime value types of spec section 4 and their JSON forms;
//! - [`eval`]: expression evaluation, builtins, and the [`eval::Effects`] trait
//!   the statement interpreter implements;
//! - [`state`]: state construction and question building (spec section 6);
//! - [`answer`]: Jev answers to values, and sampling;
//! - [`rng`]: the run's recorded random source;
//! - [`size`]: the section 6.10 size check;
//! - [`interp`]: the statement interpreter, and the driver that records live
//!   and serves a log back for resume and replay;
//! - [`judge`]: running one judgment alone (spec section 11.3);
//! - [`run`]: the state machine a host steps, and its pause and resume rules;
//! - [`pause`]: the seven pause kinds and their payloads;
//! - [`jev`]: the client that asks Jev typed questions, and its wire mapping;
//! - [`capability`]: the four capability kinds and their verbs;
//! - [`record`]: the JSONL recording format, and recording and replay;
//! - [`profile`]: model profiles — the one place token limits, request caps and
//!   prices live;
//! - [`tokens`]: token estimation, which is what makes `shape` caps mean
//!   something;
//! - [`rpc`]: the JSON-RPC method set the SDKs drive the runtime with.
//!
//! The interpreter is synchronous and is stepped by the host, so that a run can
//! be paused, recorded and replayed deterministically. The only async in the
//! crate is inside [`jev::HttpJevClient`], which owns its own tokio runtime and
//! blocks on it, and the JSON-RPC server in `jevscript-cli`.
//!
//! Status: tasks, defs, judgments, capabilities, budgets, pauses, recording
//! and replay are real ([`interp`], [`run`], [`judge`]). Machines (spec
//! section 7.8) are next: the interpreter's call stack is built so that a
//! machine frame joins task and def frames.

#![forbid(unsafe_code)]

pub mod answer;
pub mod capability;
pub mod error;
pub mod eval;
pub mod interp;
pub mod jev;
pub mod judge;
pub mod pause;
pub mod profile;
pub mod record;
pub mod rng;
pub mod rpc;
pub mod run;
pub mod size;
pub mod state;
pub mod tokens;
pub mod value;

pub use capability::{Capability, CapabilityKind, Observation};
pub use error::{RunError, RuntimeError, RuntimeErrorCode};
pub use eval::{Effects, Env, Evaluator};
pub use jev::{JevClient, JevRequest, JevResponse, Question, ScriptedJevClient};
pub use judge::{JudgmentOutcome, run_judgment, run_judgment_with_draw};
pub use pause::{Pause, PauseKind, Resume, Usage};
pub use profile::{Profile, Profiles, Tokenizer};
pub use record::{Event, LogRecord, Recorder, Replayer, StepRecord};
pub use run::{AbortHandle, Run, RunOptions, RunState};
pub use value::{Choice, Handle, Level, Value};
