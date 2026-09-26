//! Runtime errors (spec section 12).
//!
//! A runtime error surfaces to the host as an `error` pause carrying `code`,
//! `message` and `retryable`.

use serde::{Deserialize, Serialize};
use std::fmt;
use thiserror::Error;

/// A runtime error code. The list is closed and comes from the spec.
///
/// `no_enabled_events` is in the spec's table for completeness but is not here:
/// a machine with no enabled event raises an `escalate` pause with that reason,
/// not an `error` (spec section 7.8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeErrorCode {
    /// A `needs` was not bound at start.
    BindingMissing,
    /// The state plus the questions exceed the model's limits (spec 6.10).
    StateTooLarge,
    /// Network failure or 5xx from Jev.
    JevUnavailable,
    /// 4xx from Jev, with the message.
    JevRejected,
    /// A `tool` verb the adapter did not implement.
    VerbMissing,
    /// The adapter raised.
    AdapterError,
    /// A runtime type mismatch.
    TypeError,
    /// A `def` recursed past the limit.
    RecursionLimit,
    /// A replayed event did not match the next expected event (spec 10.4).
    ReplayDiverged,
    /// A `pick among` list longer than the profile's criteria cap (spec 6.4a).
    PickTooMany,
    /// No profile for the selected model id (spec section 10.6).
    ProfileMissing,
}

impl RuntimeErrorCode {
    /// The code as the spec writes it.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BindingMissing => "binding_missing",
            Self::StateTooLarge => "state_too_large",
            Self::JevUnavailable => "jev_unavailable",
            Self::JevRejected => "jev_rejected",
            Self::VerbMissing => "verb_missing",
            Self::AdapterError => "adapter_error",
            Self::TypeError => "type_error",
            Self::RecursionLimit => "recursion_limit",
            Self::ReplayDiverged => "replay_diverged",
            Self::PickTooMany => "pick_too_many",
            Self::ProfileMissing => "profile_missing",
        }
    }

    /// Whether a host may resume the run after this error.
    ///
    /// `adapter_error` is the one code the adapter decides, so it has no fixed
    /// answer here and [`RuntimeError`] carries the flag the adapter supplied.
    pub const fn default_retryable(self) -> bool {
        matches!(self, Self::JevUnavailable)
    }
}

impl fmt::Display for RuntimeErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A runtime error, with the source location it happened at.
#[derive(Debug, Clone, PartialEq, Error)]
#[error("{code}: {message}")]
pub struct RuntimeError {
    /// Which rule was broken.
    pub code: RuntimeErrorCode,
    /// What went wrong.
    pub message: String,
    /// Whether a host may resume.
    pub retryable: bool,
    /// Where in the `.jev` file it happened.
    pub span: jevscript_syntax::Span,
}

impl RuntimeError {
    /// A runtime error with the code's default retryability.
    pub fn new(code: RuntimeErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            retryable: code.default_retryable(),
            span: jevscript_syntax::Span::default(),
        }
    }

    /// The same error, reported at `span`.
    #[must_use]
    pub fn at(mut self, span: jevscript_syntax::Span) -> Self {
        self.span = span;
        self
    }

    /// The same error with the adapter's own retryability.
    #[must_use]
    pub fn retryable(mut self, retryable: bool) -> Self {
        self.retryable = retryable;
        self
    }
}

/// What can go wrong driving a run, as opposed to inside one.
///
/// These are host protocol errors: they are not in the spec's error table
/// because a correct host never causes them.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum RunError {
    /// The run is not paused, so there is nothing to resume.
    #[error("the run is not paused")]
    NotPaused,
    /// The run has ended and cannot be stepped, resumed or aborted.
    #[error("the run has ended with `{0}`")]
    Ended(&'static str),
    /// The resume payload does not fit the pause it answers.
    #[error("a `{pause}` pause cannot be resumed with this payload: {reason}")]
    WrongPayload {
        /// Which pause was open.
        pause: &'static str,
        /// Why the payload does not fit.
        reason: String,
    },
    /// Something inside the run failed.
    #[error(transparent)]
    Runtime(#[from] RuntimeError),
}
