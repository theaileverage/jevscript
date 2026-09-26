//! The IR node types.
//!
//! One type per construct in the language. The doc comments name the spec
//! section each construct comes from; the spec wins on any disagreement.

use jevscript_syntax::Span;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::IR_VERSION;

/* -------------------------------------------------------------------------- */
/* Document                                                                    */
/* -------------------------------------------------------------------------- */

/// One compiled program (spec section 11.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Ir {
    /// The IR version this document is written in. Always [`crate::IR_VERSION`].
    pub ir_version: String,
    /// `program <name>`.
    pub program: String,
    /// `in` declarations (spec section 3.2).
    pub inputs: Vec<Input>,
    /// `out` declarations (spec section 3.3).
    pub outputs: Vec<Output>,
    /// `needs` declarations (spec section 3.4). After linking these are the
    /// root program's: a library's `needs` are satisfied by its importer, so
    /// they never reach the host.
    pub needs: Vec<Need>,
    /// Every module linked into this document, root first (spec section 3.9).
    /// The host sees one flat program; this is how it can still tell where a
    /// unit came from.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modules: Vec<Module>,
    /// Named judgments (spec section 6.7).
    pub judgments: Vec<Judgment>,
    /// Tasks (spec section 7).
    pub tasks: Vec<Task>,
    /// Machines (spec section 7.8).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub machines: Vec<Machine>,
    /// Pure helpers (spec section 8).
    pub defs: Vec<Def>,
    /// The source file, if the program was compiled from a path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// The whole program.
    pub span: Span,
}

impl Ir {
    /// An otherwise empty document for `program`, at the current IR version.
    pub fn empty(program: impl Into<String>) -> Self {
        Self {
            ir_version: IR_VERSION.to_string(),
            program: program.into(),
            inputs: Vec::new(),
            outputs: Vec::new(),
            needs: Vec::new(),
            judgments: Vec::new(),
            tasks: Vec::new(),
            machines: Vec::new(),
            modules: Vec::new(),
            defs: Vec::new(),
            file: None,
            span: Span::default(),
        }
    }

    /// The task by that name, if the program declares it.
    pub fn task(&self, name: &str) -> Option<&Task> {
        self.tasks.iter().find(|task| task.name == name)
    }

    /// The machine by that name, if the program declares it.
    pub fn machine(&self, name: &str) -> Option<&Machine> {
        self.machines.iter().find(|machine| machine.name == name)
    }

    /// The judgment by that name, if the program declares it.
    pub fn judgment(&self, name: &str) -> Option<&Judgment> {
        self.judgments.iter().find(|judgment| judgment.name == name)
    }

    /// Whether the program is runnable: it declares `task main` (spec 3.1).
    pub fn is_runnable(&self) -> bool {
        self.task("main").is_some()
    }
}

/// One module linked into an IR document (spec section 3.9).
///
/// The compiler inlines every transitively used module and qualifies every unit
/// name, so `loop.read_agent` is a judgment of the linked program rather than a
/// reference into another document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Module {
    /// The alias its units are qualified with, such as `loop`. Empty for the
    /// root program, whose units keep their bare names.
    pub alias: String,
    /// The `program` name the module declared.
    pub program: String,
    /// The path it was imported from, as written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// How the importer's capabilities satisfied this module's `needs`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mapping: Vec<NeedMapping>,
    /// Where the `use` was written, or the whole file for the root.
    pub span: Span,
}

/// One entry of a `use ... with ...` clause (spec section 3.9).
///
/// The mapping is static, so the compiler checks verbs against kinds across the
/// module boundary exactly as it does within a file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct NeedMapping {
    /// The capability's name inside the library.
    pub inner: String,
    /// The importer's capability it was bound to.
    pub outer: String,
    /// The kind both sides agreed on.
    pub kind: CapabilityKind,
}

/// The five declarable types (spec section 3.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[allow(missing_docs, reason = "each variant is the type it names")]
pub enum TypeName {
    Text,
    Number,
    Bool,
    List,
    Record,
}

/// A declared input shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "shape", rename_all = "snake_case")]
pub enum Shape {
    /// A bare type.
    Type {
        /// Which type.
        name: TypeName,
    },
    /// A record shape listing field names, optionally typed.
    Record {
        /// The declared fields.
        fields: Vec<FieldShape>,
    },
}

/// One field of a record shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct FieldShape {
    /// The field's name.
    pub name: String,
    /// Its shape, if one was declared.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shape: Option<Shape>,
}

/// `in <name>: <shape>`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Input {
    /// The input's name.
    pub name: String,
    /// Its declared shape.
    pub shape: Shape,
    /// Where it was declared.
    pub span: Span,
}

/// `out <name>`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Output {
    /// The output's name.
    pub name: String,
    /// Where it was declared.
    pub span: Span,
}

/// Capability kinds (spec section 9).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityKind {
    /// Something that runs a task in the world over time and can be observed.
    Agent,
    /// A human in the loop.
    Person,
    /// A text generation model.
    Llm,
    /// Anything else; verbs are open and unchecked.
    Tool,
}

/// `needs <name>: <kind>`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Need {
    /// The capability's program-scoped name.
    pub name: String,
    /// Which verbs the compiler accepted on it.
    pub kind: CapabilityKind,
    /// The verbs a `tool` declared (spec section 9.4). Empty means the tool is
    /// open and nothing is checked before run time. The runtime also compares
    /// these against the adapter's bind-time manifest, when it passed one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub signatures: Vec<ToolSignature>,
    /// Where it was declared.
    pub span: Span,
}

/// One declared `tool` verb (spec section 9.4).
///
/// With a signature block the compiler rejects an unlisted verb
/// (`verb_unknown`) and a call with the wrong number of positional arguments
/// (`verb_arity`), and the runtime checks the adapter's actual return against
/// `returns` (`type_error`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ToolSignature {
    /// The verb's name.
    pub name: String,
    /// Its positional parameter names.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub params: Vec<String>,
    /// What it returns.
    pub returns: ReturnType,
    /// Where it was declared.
    pub span: Span,
}

/// What a declared `tool` verb returns (spec section 9.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[allow(missing_docs, reason = "each variant is the type it names")]
pub enum ReturnType {
    Text,
    Number,
    Bool,
    List,
    Record,
    Handle,
    None,
}

/* -------------------------------------------------------------------------- */
/* Units                                                                       */
/* -------------------------------------------------------------------------- */

/// A named judgment. One `judgment` is exactly one Jev request (spec 6.7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Judgment {
    /// The judgment's name, qualified with its module alias after linking:
    /// `harness.read_agent` (spec section 3.9).
    pub name: String,
    /// Its parameters, which are the state fields Jev sees.
    pub params: Vec<String>,
    /// Its results, in order.
    pub results: Vec<JudgmentResult>,
    /// Its `log` lines (spec section 5.8), run in order after the answers
    /// arrive. They never reach Jev and are not part of the shape hash.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub logs: Vec<JudgmentLog>,
    /// A stable hash over the answer space: result names, verbs, labels and
    /// level names. Hosts pin it and refuse a program whose answers moved
    /// (spec section 11.4). It uses the unit's own name rather than its module
    /// alias, so moving a judgment between files keeps its hash (section 3.9).
    pub shape_hash: String,
    /// The whole unit.
    pub span: Span,
}

/// One `<result> = <judgment expression>` inside a judgment block.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct JudgmentResult {
    /// The field the answer lands in.
    pub name: String,
    /// The question.
    pub question: Judge,
    /// Where it was written.
    pub span: Span,
}

/// A `log` line inside a judgment block (spec section 5.8).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct JudgmentLog {
    /// How many results precede it: it runs once those are bound, and may
    /// read them, the parameters and the inputs.
    pub after: u32,
    /// The log, always an [`Expr::Log`].
    pub log: Expr,
    /// Where it was written.
    pub span: Span,
}

/// The four budget keys (spec section 7.1).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
#[allow(missing_docs, reason = "each variant is the key it names")]
pub enum BudgetKey {
    Calls,
    Minutes,
    Usd,
    Steps,
}

/// The budgets a task declared. A key that is absent is unlimited, except
/// `calls`, which the compiler fills with its default of 50 (spec section 7.1).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
pub struct Budget {
    /// Model requests: Jev requests plus `llm` generations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calls: Option<f64>,
    /// Wall-clock minutes since the task started, excluding time spent paused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minutes: Option<f64>,
    /// Estimated spend from recorded usage and the runtime's price table.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usd: Option<f64>,
    /// Iterations of the outermost loop in the task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steps: Option<f64>,
}

impl Budget {
    /// The limit for `key`, if one is set.
    pub fn get(&self, key: BudgetKey) -> Option<f64> {
        match key {
            BudgetKey::Calls => self.calls,
            BudgetKey::Minutes => self.minutes,
            BudgetKey::Usd => self.usd,
            BudgetKey::Steps => self.steps,
        }
    }
}

/// The thresholds a gate reads (spec section 7.6). Names are open; these four
/// are the ones the verdict is computed from.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
pub struct Thresholds {
    /// `confirm` at or above this risk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub risk_confirm: Option<f64>,
    /// `escalate` below this confidence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_confidence: Option<f64>,
    /// `stop` below this confidence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_confidence: Option<f64>,
    /// Treat the goal as met at or above this `done` probability.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub done: Option<f64>,
    /// Any other threshold the task declared.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub other: Vec<NamedNumber>,
}

/// A name and a number, for thresholds the verdict does not read.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct NamedNumber {
    /// The name as written.
    pub name: String,
    /// Its value.
    pub value: f64,
}

/// A task: the unit a host runs (spec section 7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Task {
    /// The task's name, qualified with its module alias after linking:
    /// `loop.watch`. A pause carries this as its `task`, so a host can tell
    /// which module it came from (spec section 3.9).
    pub name: String,
    /// Its parameters. `main` has none: it reads the program's `in`
    /// declarations (spec section 7).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub params: Vec<DefParam>,
    /// Its budgets, with defaults applied.
    pub budget: Budget,
    /// Its thresholds.
    pub thresholds: Thresholds,
    /// Its body.
    pub body: Vec<Stmt>,
    /// The whole unit.
    pub span: Span,
}

/// A pure helper (spec section 8).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Def {
    /// The def's name, qualified with its module alias after linking. The
    /// prelude's defs are the implicit module `std`, and are in scope both
    /// unqualified and as `std.stuck` (spec section 3.9).
    pub name: String,
    /// Its parameters, with optional defaults.
    pub params: Vec<DefParam>,
    /// Its body.
    pub body: Vec<Stmt>,
    /// The whole unit.
    pub span: Span,
}

/// One parameter of a def.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DefParam {
    /// The parameter's name.
    pub name: String,
    /// Its default, if one was written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Expr>,
}

/// A machine: the fourth unit (spec section 7.8).
///
/// States and the events enabled in each. At every step Jev is asked one Choice
/// over exactly the enabled events plus `stay`; guards, actions and the
/// transition itself are code.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Machine {
    /// The machine's name, qualified with its module alias after linking. A
    /// pause raised inside it carries `task: "machine:<qualified name>"`.
    pub name: String,
    /// Its parameters, readable in guards, actions and `observe`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub params: Vec<DefParam>,
    /// Its budgets, with defaults applied.
    pub budget: Budget,
    /// Its thresholds, applied as a gate on every step.
    pub thresholds: Thresholds,
    /// `goal`, sent to Jev on every step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal: Option<Expr>,
    /// The state the machine starts in. The compiler fills this with the first
    /// declared state when `initial` was not written.
    pub initial: String,
    /// `observe:`, evaluated before every step and bound to `obs`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub observe: Vec<ShapeField>,
    /// The states, in source order.
    pub states: Vec<MachineState>,
    /// A stable hash over state names, event names and their targets.
    /// Descriptions are excluded, so rewording is not a breaking change
    /// (spec section 7.8).
    pub shape_hash: String,
    /// The whole unit.
    pub span: Span,
}

/// One state of a machine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MachineState {
    /// The state's name.
    pub name: String,
    /// Whether it is terminal. A machine needs at least one.
    pub done: bool,
    /// The events it enables. A terminal state has none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub transitions: Vec<Transition>,
    /// The whole state.
    pub span: Span,
}

/// One event of a state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Transition {
    /// The event's name, which is the Choice label Jev sees.
    pub event: String,
    /// What the event means, which is the label's description.
    pub description: Expr,
    /// The state this moves to.
    pub target: String,
    /// A code guard. A false guard removes the event from what Jev sees, and a
    /// guarded entry into a terminal state is what makes the result `verified`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<Expr>,
    /// Whether the step's gate sees `risk` 1 for this event instead of 0.
    pub risky: bool,
    /// The action block, run when the event fires.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub body: Vec<Stmt>,
    /// The whole transition.
    pub span: Span,
}

/* -------------------------------------------------------------------------- */
/* Statements                                                                  */
/* -------------------------------------------------------------------------- */

/// A statement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "stmt", rename_all = "snake_case")]
pub enum Stmt {
    /// `x = expr` or `x.field = expr`.
    Assign {
        /// The root variable.
        root: String,
        /// Field names that follow it.
        path: Vec<String>,
        /// What is assigned.
        value: Expr,
        /// The statement.
        span: Span,
    },
    /// A call in statement position.
    Expr {
        /// The call.
        expr: Expr,
        /// The statement.
        span: Span,
    },
    /// `if` / `elif` / `else`.
    If {
        /// The `if` and each `elif`, in order.
        branches: Vec<Branch>,
        /// The `else` body, if written.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        otherwise: Option<Vec<Stmt>>,
        /// The statement.
        span: Span,
    },
    /// `for x in xs:`, bounded by its list.
    For {
        /// The loop variables.
        names: Vec<String>,
        /// What is iterated.
        iterable: Expr,
        /// The body.
        body: Vec<Stmt>,
        /// The statement.
        span: Span,
    },
    /// `loop max N:`.
    Loop {
        /// The compile-time upper bound. Every loop has one (spec section 5.5).
        max: f64,
        /// The body.
        body: Vec<Stmt>,
        /// The statement.
        span: Span,
    },
    /// `until cond, max N:` or `until verify(cond), max N:`.
    Until {
        /// The condition.
        test: Expr,
        /// Whether the condition is the task's proof of done (spec section 7.4).
        verify: bool,
        /// The compile-time upper bound.
        max: f64,
        /// The body.
        body: Vec<Stmt>,
        /// The statement.
        span: Span,
    },
    /// A gate (spec section 7.6).
    Gate {
        /// `risk <expr>`, if written.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        risk: Option<Expr>,
        /// `confidence <expr>`, if written.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        confidence: Option<Expr>,
        /// `done <expr>`, if written.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        done: Option<Expr>,
        /// The arms that were written. Unwritten arms take the spec defaults.
        arms: Vec<GateArm>,
        /// The statement.
        span: Span,
    },
    /// `name = shape [strict]:` (spec section 7.2).
    Shape {
        /// The record the fields land in.
        target: String,
        /// Whether an overflow is an `error` pause instead of a truncation.
        strict: bool,
        /// The fields.
        fields: Vec<ShapeField>,
        /// The statement.
        span: Span,
    },
    /// `return [expr]`.
    Return {
        /// The returned value, if any.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        value: Option<Expr>,
        /// The statement.
        span: Span,
    },
    /// `continue`.
    Continue {
        /// The statement.
        span: Span,
    },
    /// `break`.
    Break {
        /// The statement.
        span: Span,
    },
    /// `stop "reason"`.
    Stop {
        /// Why.
        reason: Expr,
        /// The statement.
        span: Span,
    },
    /// `escalate "reason"`.
    Escalate {
        /// Why.
        reason: Expr,
        /// The statement.
        span: Span,
    },
}

/// One `if` or `elif` arm.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Branch {
    /// The condition.
    pub test: Expr,
    /// The body.
    pub body: Vec<Stmt>,
    /// The arm.
    pub span: Span,
}

/// The four gate verdicts, in the order section 7.6 computes them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[allow(missing_docs, reason = "each variant is the verdict it names")]
pub enum Verdict {
    Proceed,
    Confirm,
    Escalate,
    Stop,
}

/// One arm of a gate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct GateArm {
    /// Which verdict this arm handles.
    pub verdict: Verdict,
    /// What it runs.
    pub body: Vec<Stmt>,
    /// The arm.
    pub span: Span,
}

/// What happens to a `shape` field whose value exceeds its cap (spec 7.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "policy", rename_all = "snake_case")]
pub enum ShapePolicy {
    /// Keep the first tokens and record a `truncated` warning. The default.
    Head,
    /// Keep the last tokens.
    Tail,
    /// Reduce the value to what serves a purpose, using Jev (spec section 7.3).
    Focus {
        /// What the text is being reduced for.
        on: Expr,
    },
}

/// One field of a `shape` block.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ShapeField {
    /// The field's name.
    pub name: String,
    /// What fills it.
    pub value: Expr,
    /// Its hard cap in tokens, if one was written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    /// What happens on overflow.
    pub policy: ShapePolicy,
    /// The field.
    pub span: Span,
}

/* -------------------------------------------------------------------------- */
/* Expressions                                                                 */
/* -------------------------------------------------------------------------- */

/// One piece of a text literal (spec section 2.7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "part", rename_all = "snake_case")]
pub enum TextPart {
    /// Literal characters, with escapes resolved.
    Literal {
        /// The characters.
        value: String,
    },
    /// A `{expr}` hole, evaluated before the text reaches anything.
    Interpolation {
        /// The expression.
        expr: Expr,
    },
}

/// Unary operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[allow(missing_docs, reason = "each variant is the operator it names")]
pub enum UnaryOp {
    Not,
    Neg,
}

/// Binary operators, in the precedence order of spec section 5.1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[allow(missing_docs, reason = "each variant is the operator it names")]
pub enum BinaryOp {
    Or,
    And,
    Eq,
    NotEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    Add,
    Sub,
    Mul,
    Div,
    Rem,
}

/// Which syntax a call was written in. The two are equivalent (spec 5.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CallForm {
    /// `tree.create issue.branch`.
    Command,
    /// `tree.create(issue.branch)`.
    Function,
}

/// One argument of a call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Arg {
    /// The argument's name, for named arguments.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Its value.
    pub value: Expr,
}

/// An expression.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "node", rename_all = "snake_case")]
pub enum Expr {
    /// A number.
    Number {
        /// Its value.
        value: f64,
        /// Where it was written.
        span: Span,
    },
    /// A text literal with its interpolations lowered to expressions.
    Text {
        /// The parts, in order.
        parts: Vec<TextPart>,
        /// Where it was written.
        span: Span,
    },
    /// `true` or `false`.
    Bool {
        /// Its value.
        value: bool,
        /// Where it was written.
        span: Span,
    },
    /// `none`.
    None {
        /// Where it was written.
        span: Span,
    },
    /// A variable, input, parameter or capability name.
    Name {
        /// The name.
        name: String,
        /// Where it was written.
        span: Span,
    },
    /// A list.
    List {
        /// The items.
        items: Vec<Expr>,
        /// Where it was written.
        span: Span,
    },
    /// A record.
    Record {
        /// The fields.
        fields: Vec<RecordField>,
        /// Where it was written.
        span: Span,
    },
    /// `x.field`.
    Field {
        /// What is accessed.
        target: Box<Expr>,
        /// The field name.
        name: String,
        /// Where it was written.
        span: Span,
    },
    /// `x[i]`.
    Index {
        /// What is indexed.
        target: Box<Expr>,
        /// The index.
        index: Box<Expr>,
        /// Where it was written.
        span: Span,
    },
    /// A call. A trailing identifier on a capability or handle is a
    /// zero-argument call (spec section 5.2).
    Call {
        /// What is called.
        callee: Box<Expr>,
        /// Its arguments.
        args: Vec<Arg>,
        /// Which form it was written in.
        form: CallForm,
        /// Where it was written.
        span: Span,
    },
    /// `not x` or `-x`.
    Unary {
        /// Which operator.
        op: UnaryOp,
        /// Its operand.
        operand: Box<Expr>,
        /// Where it was written.
        span: Span,
    },
    /// A binary operation.
    Binary {
        /// Which operator.
        op: BinaryOp,
        /// Left operand.
        left: Box<Expr>,
        /// Right operand.
        right: Box<Expr>,
        /// Where it was written.
        span: Span,
    },
    /// `c is <label>` (spec section 4.3).
    Is {
        /// What is compared.
        target: Box<Expr>,
        /// The label, written without quotes.
        label: String,
        /// Where it was written.
        span: Span,
    },
    /// A list comprehension (spec section 8.2).
    Comprehension {
        /// What each item becomes.
        expr: Box<Expr>,
        /// The loop variables.
        names: Vec<String>,
        /// What is iterated.
        iterable: Box<Expr>,
        /// The filter, if written.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        test: Option<Box<Expr>>,
        /// Where it was written.
        span: Span,
    },
    /// `focus <text> on "<purpose>", max <tokens>` (spec section 7.3).
    Focus {
        /// The text to reduce.
        text: Box<Expr>,
        /// What it is being reduced for.
        on: Box<Expr>,
        /// The token cap.
        max: f64,
        /// Where it was written.
        span: Span,
    },
    /// `trail <n>` (spec section 7.5).
    Trail {
        /// How many step records.
        count: f64,
        /// Where it was written.
        span: Span,
    },
    /// A judgment expression.
    Judge(Box<Judge>),
    /// `log <level> <expr> [ { fields } ]` (spec section 5.8). Records a `log`
    /// event with the value's text form as its message and the evaluated
    /// fields, then evaluates to the value. It never reaches Jev, never counts
    /// against a budget and never changes control flow.
    Log {
        /// The level.
        level: LogLevel,
        /// The logged value.
        value: Box<Expr>,
        /// The structured fields, in written order.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        fields: Vec<RecordField>,
        /// Where it was written.
        span: Span,
    },
}

/// The four `log` levels, lowest first (spec section 5.8).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
#[allow(missing_docs, reason = "each variant is the level it names")]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

impl LogLevel {
    /// How the level is written.
    pub const fn as_str(self) -> &'static str {
        match self {
            LogLevel::Debug => "debug",
            LogLevel::Info => "info",
            LogLevel::Warn => "warn",
            LogLevel::Error => "error",
        }
    }
}

/// One field of a record literal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RecordField {
    /// The field's name.
    pub name: String,
    /// Its value. `{ title }` is lowered to `{ title: title }`.
    pub value: Expr,
}

/* -------------------------------------------------------------------------- */
/* Judgments                                                                   */
/* -------------------------------------------------------------------------- */

/// A judgment subject: always a path, so that it can become a state path Jev is
/// told to inspect by name (spec section 6.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Subject {
    /// The root variable, input or parameter.
    pub root: String,
    /// What follows it.
    pub path: Vec<SubjectStep>,
    /// The path as Jev sees it, such as `obs.summary` or `files[3]`.
    pub state_path: String,
    /// Where it was written.
    pub span: Span,
}

/// One step of a subject path.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "step", rename_all = "snake_case")]
pub enum SubjectStep {
    /// `.name`.
    Field {
        /// The field name.
        name: String,
    },
    /// `[expr]`.
    Index {
        /// The index.
        index: Expr,
    },
}

/// A detail block refining a question. The keys map directly onto Jev's
/// instruction object (spec section 6.8).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
pub struct Detail {
    /// What to look at within the subject.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus: Option<Expr>,
    /// A supporting fact the question needs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<Expr>,
    /// Other state paths to read alongside the subject.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compare: Option<Expr>,
    /// Short concrete examples of the positive side of a `feels`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub yes: Option<Expr>,
    /// Short concrete examples of the negative side of a `feels`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub no: Option<Expr>,
    /// `sample true`: draw the label or level from Jev's distribution instead
    /// of taking the argmax (spec section 6.11). `feels` is never sampled, and
    /// sampling changes only which label or level is reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample: Option<bool>,
}

/// One label of a `pick` (spec section 6.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PickLabel {
    /// The label. It is an API; renaming it is a breaking change.
    pub name: String,
    /// Whether it is the bare `other` / `none` escape option.
    pub escape: bool,
    /// Its one-line description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<Expr>,
    /// What the label covers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub what: Option<Expr>,
    /// What the label does not cover.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_for: Option<Expr>,
    /// Short concrete instances of the label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub examples: Option<Expr>,
    /// Where it was written.
    pub span: Span,
}

/// One level of a `rate` (spec section 6.4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RateLevel {
    /// The level's name, if it was named. Named levels allow `l is unchanged`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The situation it describes.
    pub situation: Expr,
    /// Where it was written.
    pub span: Span,
}

/// Which question is asked.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "verb", rename_all = "snake_case")]
pub enum JudgeVerb {
    /// A yes/no question, answered with a probability. Maps to a Jev Noul.
    Feels {
        /// The condition.
        condition: Expr,
    },
    /// A choice among labelled descriptions. Maps to a Jev Choice.
    Pick {
        /// The labels.
        labels: Vec<PickLabel>,
    },
    /// A place on a spectrum of situations. Maps to a Jev Score.
    Rate {
        /// The levels, low to high.
        levels: Vec<RateLevel>,
    },
    /// A choice among the elements of a runtime list (spec section 6.4a).
    ///
    /// The options are built at run time, so the shape hash covers the question
    /// and the `by` field but not the items: the host contract stays stable
    /// across inputs. `label` is `"i<index>"` or `"none"`.
    PickAmong {
        /// What Jev is choosing for.
        question: Expr,
        /// Which field of each item is the description Jev sees.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        by: Option<String>,
        /// Whether choosing nothing is allowed.
        allow_none: bool,
    },
}

/// A judgment expression: one question to Jev (spec section 6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Judge {
    /// `each` asks the question once per element of a list, in one request.
    pub each: bool,
    /// What is judged.
    pub subject: Subject,
    /// Which question is asked.
    pub verb: JudgeVerb,
    /// The detail block, if one was written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<Detail>,
    /// Which request this question is batched into (spec section 6.6).
    /// Questions in one request share one state and never see each other's
    /// answers.
    pub request_group: u32,
    /// Where it was written.
    pub span: Span,
}
