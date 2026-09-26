//! Compile diagnostics.
//!
//! The codes are closed and come from section 12 of
//! `spec/jevscript-language-specification.md`. Do not invent a code here; add it
//! to the spec first.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::fmt;

use crate::span::Span;

/// A compile error code (spec section 12).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// Inconsistent indentation or a tab.
    Indent,
    /// `loop` or `until` without `max`.
    UnboundedLoop,
    /// A `pick` without an `other` or `none` label.
    PickNoOther,
    /// Fewer than two or more than eight `pick` labels.
    PickArity,
    /// Fewer than two or more than ten `rate` levels.
    RateArity,
    /// A `rate` level that is only a number or a degree word.
    RateBareDegree,
    /// A judgment subject that is not a variable or field path.
    SubjectNotPath,
    /// A capability call, gate, or loop inside a `judgment`.
    JudgmentSideEffect,
    /// A capability call, gate, or pause inside a `def`.
    DefSideEffect,
    /// A verb not defined for the capability's kind (never raised for `tool`).
    VerbUnknown,
    /// More than one `verify` in a task.
    VerifyTwice,
    /// An identifier containing an uppercase letter.
    UppercaseIdentifier,
    /// A variable read before assignment.
    UnassignedRead,
    /// The `use` path did not resolve.
    UseNotFound,
    /// Modules import each other.
    UseCycle,
    /// An imported file declares `in` or `out`.
    UseHasInputs,
    /// A library `needs` has no entry in the `with` clause, or the kinds differ.
    UseNeedsUnmapped,
    /// A reference to a `_`-prefixed name in another module.
    UsePrivate,
    /// `task main` declares parameters.
    MainHasParams,
    /// A tool call with the wrong number of positional arguments for its
    /// declared signature (spec section 9.4).
    VerbArity,
    /// A `->` target that is not a declared state.
    MachineUnknownState,
    /// A machine with no terminal state.
    MachineNoDone,
    /// An `on` line without a description text.
    EventNoDescription,
    /// A `gate` inside a machine action block.
    MachineGate,
    /// `each` combined with `pick among`.
    PickAmongEach,
    /// A machine state with no path to any terminal state. A warning, not an
    /// error (spec section 7.8).
    MachineUnreachableDone,
    /// A `gate` without both `risk` and `confidence` (spec section 7.6).
    GateArgs,
    /// Two units, two states of one machine, or two events of one state with
    /// the same name.
    DuplicateName,
    /// An assignment to an input (spec section 3.2) or a capability name (3.4).
    AssignImmutable,
    /// `break` or `continue` outside a loop.
    LoopControlOutside,
    /// A `log` whose level is not `debug`, `info`, `warn` or `error`
    /// (spec section 5.8).
    LogLevel,
    /// A `log` inside a judgment expression: its subject's index, condition,
    /// labels, levels, detail or `pick among` question (spec section 5.8).
    LogInQuestion,

    /// A `prob` used bare in a condition (spec section 4.1). A warning.
    BareProbCondition,
    /// A `shape` or `observe` field without `max` whose value is not a number,
    /// bool or short list (spec section 7.2). A warning.
    UncappedField,
    /// A user unit with the same name as a prelude def (spec section 3.9). A
    /// warning.
    PreludeShadowed,
    /// A detail key in a position it does not apply to (spec section 6.8). A
    /// warning.
    DetailIgnored,
    /// A unit named `log`, which turns the `log` expression off in its file
    /// (spec section 5.8). A warning.
    LogShadowed,

    /// Source that does not match the grammar, when no more specific compile
    /// error applies (spec section 12).
    Syntax,
}

impl ErrorCode {
    /// The code as it is written in the spec and printed by the CLI.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Indent => "indent",
            Self::UnboundedLoop => "unbounded_loop",
            Self::PickNoOther => "pick_no_other",
            Self::PickArity => "pick_arity",
            Self::RateArity => "rate_arity",
            Self::RateBareDegree => "rate_bare_degree",
            Self::SubjectNotPath => "subject_not_path",
            Self::JudgmentSideEffect => "judgment_side_effect",
            Self::DefSideEffect => "def_side_effect",
            Self::VerbUnknown => "verb_unknown",
            Self::VerifyTwice => "verify_twice",
            Self::UppercaseIdentifier => "uppercase_identifier",
            Self::UnassignedRead => "unassigned_read",
            Self::UseNotFound => "use_not_found",
            Self::UseCycle => "use_cycle",
            Self::UseHasInputs => "use_has_inputs",
            Self::UseNeedsUnmapped => "use_needs_unmapped",
            Self::UsePrivate => "use_private",
            Self::MainHasParams => "main_has_params",
            Self::VerbArity => "verb_arity",
            Self::MachineUnknownState => "machine_unknown_state",
            Self::MachineNoDone => "machine_no_done",
            Self::EventNoDescription => "event_no_description",
            Self::MachineGate => "machine_gate",
            Self::PickAmongEach => "pick_among_each",
            Self::MachineUnreachableDone => "machine_unreachable_done",
            Self::GateArgs => "gate_args",
            Self::DuplicateName => "duplicate_name",
            Self::AssignImmutable => "assign_immutable",
            Self::LoopControlOutside => "loop_control_outside",
            Self::LogLevel => "log_level",
            Self::LogInQuestion => "log_in_question",
            Self::BareProbCondition => "bare_prob_condition",
            Self::UncappedField => "uncapped_field",
            Self::PreludeShadowed => "prelude_shadowed",
            Self::DetailIgnored => "detail_ignored",
            Self::LogShadowed => "log_shadowed",
            Self::Syntax => "syntax",
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// How much a diagnostic matters. Warnings do not fail a compile; `check`
/// reports both (spec section 11.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Fails the compile.
    Error,
    /// Reported, does not fail the compile.
    Warning,
}

/// A compile error or warning, with a code, a message, a source range and
/// the file it was found in (spec section 12).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Diagnostic {
    /// Which rule was broken.
    pub code: ErrorCode,
    /// Whether this fails the compile.
    pub severity: Severity,
    /// What went wrong, in one sentence.
    pub message: String,
    /// Where it went wrong.
    pub span: Span,
    /// The file the span is in, once the compiler knows it: a library's
    /// errors are reported against the library (spec section 3.9). The lexer
    /// and the parser see one source and leave it unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// Other source locations involved in this failure (spec section 12).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub related: Vec<RelatedLocation>,
}

/// Another source location that explains a diagnostic (spec section 12).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RelatedLocation {
    /// Why this location matters.
    pub message: String,
    /// The source range.
    pub span: Span,
    /// The source file, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
}

impl Diagnostic {
    /// An error-severity diagnostic.
    pub fn error(code: ErrorCode, message: impl Into<String>, span: Span) -> Self {
        Self {
            code,
            severity: Severity::Error,
            message: message.into(),
            span,
            file: None,
            related: Vec::new(),
        }
    }

    /// A warning-severity diagnostic.
    pub fn warning(code: ErrorCode, message: impl Into<String>, span: Span) -> Self {
        Self {
            code,
            severity: Severity::Warning,
            message: message.into(),
            span,
            file: None,
            related: Vec::new(),
        }
    }

    /// The same diagnostic, attributed to `file` unless it already names one.
    pub fn in_file(mut self, file: impl Into<String>) -> Self {
        if self.file.is_none() {
            self.file = Some(file.into());
        }
        self
    }

    /// Whether this diagnostic fails the compile.
    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }
}

impl fmt::Display for Diagnostic {
    /// `file:line:col: code: message`, the form the CLI prints (spec section
    /// 11.6); without a file, `line:col: code: message`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(file) = &self.file {
            write!(f, "{file}:")?;
        }
        write!(
            f,
            "{}:{}: {}: {}",
            self.span.start.line, self.span.start.column, self.code, self.message
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::span::Pos;

    #[test]
    fn a_diagnostic_round_trips_through_json_with_its_file() {
        // Spec section 12: every diagnostic names the file it was found in.
        let diagnostic = Diagnostic::error(
            ErrorCode::UsePrivate,
            "private",
            Span::new(Pos::new(3, 4, 20), Pos::new(3, 9, 25)),
        )
        .in_file("lib/agent_loop.jev");
        let json = serde_json::to_string(&diagnostic).expect("serializes");
        assert!(json.contains("\"file\":\"lib/agent_loop.jev\""), "{json}");
        let back: Diagnostic = serde_json::from_str(&json).expect("deserializes");
        assert_eq!(back, diagnostic);
        assert_eq!(
            diagnostic.to_string(),
            "lib/agent_loop.jev:3:4: use_private: private"
        );
    }

    #[test]
    fn a_diagnostic_without_a_file_omits_the_field() {
        let diagnostic = Diagnostic::warning(ErrorCode::UncappedField, "big", Span::default());
        let json = serde_json::to_string(&diagnostic).expect("serializes");
        assert!(!json.contains("file"), "{json}");
        let back: Diagnostic = serde_json::from_str(&json).expect("deserializes");
        assert_eq!(back.file, None);
    }
}
