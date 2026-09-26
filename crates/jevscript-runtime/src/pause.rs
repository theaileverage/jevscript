//! Pauses: the only thing a host observes of a run (spec section 10.2).
//!
//! Every pause carries `kind`, `run_id`, `step`, `task`, `source` and
//! `recording_offset`. The kind-specific fields, and the payload each kind is
//! resumed with, are fixed by the spec's table.

use jevscript_ir::BudgetKey;
use jevscript_syntax::Span;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::error::RuntimeErrorCode;
use crate::value::Value;

/// The seven pause kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PauseKind {
    /// A person was asked something, or a gate asked for confirmation.
    Confirm,
    /// A person was handed the run. Terminal unless the host resumes explicitly.
    Escalate,
    /// Waiting on a capability. Informational; auto-resumes.
    Waiting,
    /// A budget was exceeded. Resumable if the host extends it.
    Budget,
    /// A runtime or adapter error. Resumable only for retryable codes.
    Error,
    /// The run ended on `stop`. Terminal.
    Stopped,
    /// The task finished. Terminal.
    Done,
}

impl PauseKind {
    /// The kind as the spec writes it.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Confirm => "confirm",
            Self::Escalate => "escalate",
            Self::Waiting => "waiting",
            Self::Budget => "budget",
            Self::Error => "error",
            Self::Stopped => "stopped",
            Self::Done => "done",
        }
    }

    /// Whether the run is over when this pause is reached.
    ///
    /// `escalate` is terminal by default but a host may resume it explicitly,
    /// and `error` is terminal unless the code is retryable, so neither is
    /// settled by the kind alone.
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Stopped | Self::Done)
    }
}

/// What a run reports usage as on `done` (spec section 10.2).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Usage {
    /// Model requests: Jev requests plus `llm` generations.
    pub calls: u64,
    /// Tokens across every request.
    pub tokens: u64,
    /// Estimated spend, from recorded usage and the runtime's price table.
    pub usd: f64,
    /// Wall-clock minutes, excluding time spent paused.
    pub minutes: f64,
    /// Iterations of the outermost loop.
    pub steps: u64,
    /// What spawned agents reported spending on their own models, summed from
    /// their observation records. For information only: the runtime never sees
    /// those calls and never gates on this (spec section 7.1).
    #[serde(default)]
    pub adapter_usd: f64,
    /// What spawned agents reported using in tokens, on the same terms.
    #[serde(default)]
    pub adapter_tokens: u64,
}

/// A pause and its payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Pause {
    /// Resumes with `{ answer, text? }`.
    Confirm {
        /// The fields every pause carries.
        #[serde(flatten)]
        common: PauseCommon,
        /// What the person is being asked.
        message: String,
        /// The options offered. Defaults to `["yes", "no"]`.
        options: Vec<String>,
        /// Whatever context the program attached.
        context: BTreeMap<String, Value>,
    },
    /// Resumes with `{ resume: true }`, or the host aborts.
    Escalate {
        /// The fields every pause carries.
        #[serde(flatten)]
        common: PauseCommon,
        /// Why the run escalated.
        reason: String,
        /// Whatever context the program attached.
        context: BTreeMap<String, Value>,
    },
    /// Auto-resumes. The host may inject a message while it is open.
    Waiting {
        /// The fields every pause carries.
        #[serde(flatten)]
        common: PauseCommon,
        /// The capability being waited on.
        on: String,
        /// What is being waited for.
        condition: String,
        /// How long the wait may run.
        timeout_minutes: f64,
    },
    /// Resumes with `{ extend: { key: n } }`.
    ///
    /// `common.task` names the task or machine whose limit was hit; extending
    /// raises only that one, and the run continues if every task on the stack
    /// still has room (spec section 7.1).
    Budget {
        /// The fields every pause carries.
        #[serde(flatten)]
        common: PauseCommon,
        /// Which budget ran out.
        key: BudgetKey,
        /// How much was used.
        used: f64,
        /// What the limit was.
        limit: f64,
    },
    /// Resumes with `{ retry: true }` if `retryable`.
    Error {
        /// The fields every pause carries.
        #[serde(flatten)]
        common: PauseCommon,
        /// Which error.
        code: RuntimeErrorCode,
        /// What went wrong.
        message: String,
        /// Whether the host may retry.
        retryable: bool,
    },
    /// Terminal.
    Stopped {
        /// The fields every pause carries.
        #[serde(flatten)]
        common: PauseCommon,
        /// Why the run stopped.
        reason: String,
    },
    /// Terminal.
    Done {
        /// The fields every pause carries.
        #[serde(flatten)]
        common: PauseCommon,
        /// Every declared output. One never assigned is `none`.
        outputs: BTreeMap<String, Value>,
        /// True only if the task's `verify` condition was true at the end.
        verified: bool,
        /// Calls, tokens, estimated USD, minutes and steps.
        usage: Usage,
    },
}

/// The fields every pause carries (spec section 10.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PauseCommon {
    /// Which run.
    pub run_id: String,
    /// Which step of it.
    pub step: u64,
    /// Which task. A pause raised inside a machine carries
    /// `machine:<qualified name>` (spec section 7.8).
    pub task: String,
    /// The machine state the pause was raised in, when it was raised inside a
    /// machine (spec section 7.8).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    /// Where in the `.jev` file.
    pub source: Span,
    /// The byte offset in the recording this pause was written at.
    pub recording_offset: u64,
}

impl Pause {
    /// Which kind this pause is.
    pub const fn kind(&self) -> PauseKind {
        match self {
            Pause::Confirm { .. } => PauseKind::Confirm,
            Pause::Escalate { .. } => PauseKind::Escalate,
            Pause::Waiting { .. } => PauseKind::Waiting,
            Pause::Budget { .. } => PauseKind::Budget,
            Pause::Error { .. } => PauseKind::Error,
            Pause::Stopped { .. } => PauseKind::Stopped,
            Pause::Done { .. } => PauseKind::Done,
        }
    }

    /// The fields every pause carries.
    pub const fn common(&self) -> &PauseCommon {
        match self {
            Pause::Confirm { common, .. }
            | Pause::Escalate { common, .. }
            | Pause::Waiting { common, .. }
            | Pause::Budget { common, .. }
            | Pause::Error { common, .. }
            | Pause::Stopped { common, .. }
            | Pause::Done { common, .. } => common,
        }
    }

    /// The fields every pause carries, mutably. The interpreter fills in
    /// `recording_offset` once it knows where the pause lands.
    pub const fn common_mut(&mut self) -> &mut PauseCommon {
        match self {
            Pause::Confirm { common, .. }
            | Pause::Escalate { common, .. }
            | Pause::Waiting { common, .. }
            | Pause::Budget { common, .. }
            | Pause::Error { common, .. }
            | Pause::Stopped { common, .. }
            | Pause::Done { common, .. } => common,
        }
    }

    /// Whether this particular pause ends the run.
    ///
    /// `escalate` ends it unless the host resumes explicitly, and `error` ends
    /// it unless the code is retryable.
    pub const fn is_terminal(&self) -> bool {
        match self {
            Pause::Stopped { .. } | Pause::Done { .. } => true,
            Pause::Error { retryable, .. } => !*retryable,
            _ => false,
        }
    }

    /// The options a `confirm` pause defaults to.
    pub fn default_options() -> Vec<String> {
        vec!["yes".to_string(), "no".to_string()]
    }
}

/// What a host resumes a pause with (spec section 10.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Resume {
    /// Answers a `confirm`.
    Answer {
        /// Which option was chosen.
        answer: String,
        /// Free text the host added.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
    },
    /// Continues past an `escalate`.
    Continue {
        /// Always `true`.
        resume: bool,
    },
    /// Raises a budget and continues.
    Extend {
        /// The keys to raise and their new limits.
        extend: BTreeMap<BudgetKey, f64>,
    },
    /// Retries after a retryable `error`.
    Retry {
        /// Always `true`.
        retry: bool,
    },
    /// Resumes a `waiting` pause, which needs nothing.
    Nothing {},
}
