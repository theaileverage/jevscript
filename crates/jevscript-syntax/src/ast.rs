//! The abstract syntax tree.
//!
//! One type per production in section 13 of the spec's grammar. Every node
//! carries a [`Span`]; the compiler copies those spans into the IR so that
//! pauses and errors point back at lines (spec section 11.1).

use serde::Serialize;

use crate::span::Span;
use crate::token::TextLit;

/// An identifier with its source span.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Ident {
    /// The name as written. Always lowercase (spec section 2.4).
    pub name: String,
    /// Where it was written.
    pub span: Span,
}

/// `program = "program" NAME NEWLINE { use_decl } { decl } { unit }`.
///
/// A source file holds exactly one program, and one program is one module
/// (spec sections 3 and 3.9).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Program {
    /// `program <name>`.
    pub name: Ident,
    /// `use` declarations, in source order (spec section 3.9).
    pub uses: Vec<UseDecl>,
    /// `in`, `out` and `needs` declarations, in source order.
    pub decls: Vec<Decl>,
    /// `judgment`, `task` and `def` units, in source order.
    pub units: Vec<Unit>,
    /// The whole file.
    pub span: Span,
}

impl Program {
    /// Whether the file declares a judgment, task, def or machine named
    /// `name`. A unit named `log` turns the `log` expression off in its file
    /// (spec section 5.8).
    pub fn declares_unit(&self, name: &str) -> bool {
        self.units.iter().any(|unit| {
            let ident = match unit {
                Unit::Judgment(u) => &u.name,
                Unit::Task(u) => &u.name,
                Unit::Def(u) => &u.name,
                Unit::Machine(u) => &u.name,
            };
            ident.name == name
        })
    }
}

/// `use_decl = "use" TEXT "as" NAME [ "with" mapping { "," mapping } ]`.
///
/// Imports another `.jev` file as a module (spec section 3.9). The path is
/// relative to the importing file unless it resolves against a host search
/// root. Every `needs` the imported file declares must appear in the `with`
/// clause, or the import is [`crate::ErrorCode::UseNeedsUnmapped`].
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UseDecl {
    /// The path, as written.
    pub path: TextLit,
    /// The alias its exported units are reached through: `loop.read_agent`.
    pub alias: Ident,
    /// How the importer's capabilities satisfy the library's `needs`.
    pub mapping: Vec<UseMapping>,
    /// The declaration.
    pub span: Span,
}

/// `mapping = NAME [ ":" NAME ]`.
///
/// `with claude` binds the library's `claude` to the importer's `claude`;
/// `with dev: claude` binds the library's `dev` to the importer's `claude`.
/// Nothing is bound implicitly: a library cannot reach a capability it was not
/// handed.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UseMapping {
    /// The name inside the library.
    pub inner: Ident,
    /// The importer's capability it is bound to. Absent means the same name.
    pub outer: Option<Ident>,
    /// Where it was written.
    pub span: Span,
}

/// `decl = "in" ... | "out" ... | "needs" ...`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "decl", rename_all = "snake_case")]
pub enum Decl {
    /// `in <name>: <shape>` (spec section 3.2).
    In {
        /// The input's name.
        name: Ident,
        /// Its declared shape.
        shape: Shape,
        /// The declaration.
        span: Span,
    },
    /// `out <name>` (spec section 3.3).
    Out {
        /// The output's name.
        name: Ident,
        /// The declaration.
        span: Span,
    },
    /// `needs <name>: <kind>` (spec section 3.4), with an optional signature
    /// block for a `tool` (spec section 9.4).
    Needs {
        /// The capability's program-scoped name.
        name: Ident,
        /// Which verbs the compiler will accept on it.
        kind: CapabilityKind,
        /// The verbs a `tool` declares. Empty means the tool is open and
        /// nothing is checked before run time.
        signatures: Vec<ToolSignature>,
        /// The declaration.
        span: Span,
    },
}

/// `shape = "text" | "number" | "bool" | "list" | "record" | "{" ... "}"`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "shape", rename_all = "snake_case")]
pub enum Shape {
    /// A bare type name.
    Type {
        /// Which type.
        name: TypeName,
        /// Where it was written.
        span: Span,
    },
    /// A record shape listing field names, optionally typed.
    Record {
        /// The declared fields.
        fields: Vec<FieldDecl>,
        /// Where it was written.
        span: Span,
    },
}

/// The five declarable types (spec section 3.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[allow(missing_docs, reason = "each variant is the type it names")]
pub enum TypeName {
    Text,
    Number,
    Bool,
    List,
    Record,
}

/// `field_decl = NAME [ ":" shape ]`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FieldDecl {
    /// The field's name.
    pub name: Ident,
    /// Its shape, if one was written.
    pub shape: Option<Shape>,
    /// The declaration.
    pub span: Span,
}

/// `kind = "agent" | "person" | "llm" | "tool"` (spec section 9).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
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

/// `signature = NAME "(" [ NAME { "," NAME } ] ")" "->" ret_type` (spec 9.4).
///
/// With a signature block the compiler rejects an unlisted verb
/// ([`crate::ErrorCode::VerbUnknown`]) and a call with the wrong number of
/// positional arguments ([`crate::ErrorCode::VerbArity`]), and the runtime
/// checks the adapter's actual return against `returns`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ToolSignature {
    /// The verb's name.
    pub name: Ident,
    /// Its positional parameter names.
    pub params: Vec<Ident>,
    /// What it returns.
    pub returns: ReturnType,
    /// Where it was written.
    pub span: Span,
}

/// `ret_type = "text" | "number" | "bool" | "list" | "record" | "handle" | "none"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
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

/// `unit = judgment | task | def | machine`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "unit", rename_all = "snake_case")]
pub enum Unit {
    /// A pure block of judgments over declared state fields.
    Judgment(JudgmentUnit),
    /// A block that can observe, judge, gate and act.
    Task(TaskUnit),
    /// A pure helper.
    Def(DefUnit),
    /// A state machine whose transitions Jev chooses among (spec section 7.8).
    Machine(MachineUnit),
}

/// `judgment = "judgment" NAME "(" params ")" ":" INDENT { NAME "=" judge_expr } DEDENT`.
///
/// One `judgment` compiles to exactly one Jev request (spec section 6.7). A
/// name starting with `_` is private to its file (spec section 3.9).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct JudgmentUnit {
    /// The judgment's name.
    pub name: Ident,
    /// Its parameters, which are the state fields Jev sees.
    pub params: Vec<Ident>,
    /// Its body: judgment assignments and nothing else.
    pub results: Vec<JudgmentResult>,
    /// `log` lines among the results (spec section 5.8). They never reach
    /// Jev; each runs after the answers arrive, where it was written.
    pub logs: Vec<JudgmentLog>,
    /// The whole unit.
    pub span: Span,
}

/// A `log` line inside a `judgment` block (spec section 5.8).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct JudgmentLog {
    /// How many results were written before it, which are the results it may
    /// read.
    pub after: usize,
    /// The log, always an [`Expr::Log`].
    pub log: Expr,
    /// The whole line.
    pub span: Span,
}

/// One `<result> = <judgment expression>` line inside a `judgment` block.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct JudgmentResult {
    /// The field the answer lands in.
    pub name: Ident,
    /// The question.
    pub question: JudgeExpr,
    /// The whole line.
    pub span: Span,
}

/// `task = "task" NAME [ "(" params ")" ] [ "budget" ... ] [ "thresholds" ... ] ":" block`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TaskUnit {
    /// The task's name. `main` is the entry point and takes no parameters; it
    /// reads the program's `in` declarations instead
    /// ([`crate::ErrorCode::MainHasParams`]).
    pub name: Ident,
    /// Its parameters. Tasks are the unit of reuse across modules: a library
    /// exposes a task and the importer hands it handles and values
    /// (spec sections 7 and 3.9).
    pub params: Vec<Param>,
    /// Declared budgets (spec section 7.1).
    pub budgets: Vec<BudgetItem>,
    /// Declared thresholds, read by every gate in the task (spec section 7.6).
    pub thresholds: Vec<ThresholdItem>,
    /// The task body.
    pub body: Block,
    /// The whole unit.
    pub span: Span,
}

/// `budget_item = ( "calls" | "minutes" | "usd" | "steps" ) NUMBER`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BudgetItem {
    /// Which budget.
    pub key: BudgetKey,
    /// Its limit.
    pub value: f64,
    /// Where it was written.
    pub span: Span,
}

/// The four budget keys (spec section 7.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
#[allow(missing_docs, reason = "each variant is the key it names")]
pub enum BudgetKey {
    Calls,
    Minutes,
    Usd,
    Steps,
}

/// `threshold_item = NAME NUMBER`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ThresholdItem {
    /// The threshold's name, such as `risk_confirm`.
    pub name: Ident,
    /// Its value.
    pub value: f64,
    /// Where it was written.
    pub span: Span,
}

/// `def = "def" NAME "(" params ")" ":" block` (spec section 8).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DefUnit {
    /// The def's name.
    pub name: Ident,
    /// Its parameters, with optional defaults.
    pub params: Vec<Param>,
    /// Its body.
    pub body: Block,
    /// The whole unit.
    pub span: Span,
}

/// `param = NAME [ "=" expr ]`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Param {
    /// The parameter's name.
    pub name: Ident,
    /// Its default, if one was written.
    pub default: Option<Expr>,
    /// Where it was written.
    pub span: Span,
}

/// `machine = "machine" NAME "(" params ")" [ "budget" ... ] [ "thresholds" ... ] ":"`
/// (spec section 7.8).
///
/// The fourth unit. It declares states and the events enabled in each state; at
/// every step Jev is asked one Choice over exactly the enabled events plus
/// `stay`. The machine is the deterministic skeleton and Jev only ever picks a
/// legal transition: guards are code, actions are code, and the transition sets
/// the state.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MachineUnit {
    /// The machine's name.
    pub name: Ident,
    /// Its parameters, readable in guards, actions and `observe`.
    pub params: Vec<Param>,
    /// Its budgets. A callee runs under the smaller of its own limit and the
    /// caller's remainder (spec section 7.1).
    pub budgets: Vec<BudgetItem>,
    /// Its thresholds, applied as a gate on every step.
    pub thresholds: Vec<ThresholdItem>,
    /// `goal "<text>"`, sent to Jev on every step.
    pub goal: Option<Expr>,
    /// `initial <state>`. Absent means the first state declared.
    pub initial: Option<Ident>,
    /// `observe:`, evaluated before every step and bound to `obs`.
    pub observe: Vec<ShapeField>,
    /// The states, in source order.
    pub states: Vec<MachineState>,
    /// The whole unit.
    pub span: Span,
}

/// `state = "state" NAME "done" | "state" NAME ":" INDENT { transition } DEDENT`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MachineState {
    /// The state's name.
    pub name: Ident,
    /// Whether it is terminal. A machine needs at least one
    /// ([`crate::ErrorCode::MachineNoDone`]).
    pub done: bool,
    /// The events it enables. A terminal state has none.
    pub transitions: Vec<Transition>,
    /// The whole state.
    pub span: Span,
}

/// `transition = "on" NAME TEXT "->" NAME [ "when" expr ] [ "risky" ] ( NEWLINE | ":" block )`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Transition {
    /// The event's name. Unique within its state, and reusable across states.
    pub event: Ident,
    /// What the event means, which is what Jev reads. Required
    /// ([`crate::ErrorCode::EventNoDescription`]).
    pub description: Expr,
    /// The state this moves to. Must be declared
    /// ([`crate::ErrorCode::MachineUnknownState`]).
    pub target: Ident,
    /// `when <expr>`: a code guard. A false guard removes the event from what
    /// Jev sees, and a guarded entry into a terminal state is what makes the
    /// result `verified` (spec section 7.8).
    pub when: Option<Expr>,
    /// `risky`: the gate sees `risk` 1 for this event instead of 0.
    pub risky: bool,
    /// The action block, run when the event fires. It may call capabilities,
    /// run judgments and call defs, but may not `gate`
    /// ([`crate::ErrorCode::MachineGate`]).
    pub body: Option<Block>,
    /// The whole transition.
    pub span: Span,
}

/// `block = NEWLINE INDENT { statement } DEDENT`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Block {
    /// The statements, in order.
    pub statements: Vec<Stmt>,
    /// The whole block.
    pub span: Span,
}

/// `statement = ...`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "stmt", rename_all = "snake_case")]
pub enum Stmt {
    /// `target = judge_expr | command | expr`.
    Assign {
        /// What is assigned to.
        target: Target,
        /// What it is assigned.
        value: Expr,
        /// The statement.
        span: Span,
    },
    /// A bare command in statement position, such as `dev.stop`.
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
        otherwise: Option<Block>,
        /// The statement.
        span: Span,
    },
    /// `for NAME [, NAME] in expr:`.
    For {
        /// The loop variables.
        names: Vec<Ident>,
        /// What is iterated.
        iterable: Expr,
        /// The body.
        body: Block,
        /// The statement.
        span: Span,
    },
    /// `loop max N:` (spec section 5.5).
    Loop {
        /// The compile-time upper bound.
        max: f64,
        /// The body.
        body: Block,
        /// The statement.
        span: Span,
    },
    /// `until cond, max N:` or `until verify(cond), max N:`.
    Until {
        /// The condition, evaluated before each iteration and once after the last.
        test: Expr,
        /// Whether the condition was written as `verify(...)` (spec section 7.4).
        verify: bool,
        /// The compile-time upper bound.
        max: f64,
        /// The body.
        body: Block,
        /// The statement.
        span: Span,
    },
    /// `gate risk ..., confidence ..., done ...:` (spec section 7.6).
    Gate(GateStmt),
    /// `name = shape [strict]:` (spec section 7.2).
    Shape(ShapeStmt),
    /// `return [expr]`.
    Return {
        /// The returned value, if any.
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
    /// `stop "reason"`: ends the run with a `stopped` pause.
    Stop {
        /// Why.
        reason: Expr,
        /// The statement.
        span: Span,
    },
    /// `escalate "reason"`: ends the run with an `escalate` pause.
    Escalate {
        /// Why.
        reason: Expr,
        /// The statement.
        span: Span,
    },
}

/// One `if` or `elif` arm.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Branch {
    /// The condition.
    pub test: Expr,
    /// The body.
    pub body: Block,
    /// The arm.
    pub span: Span,
}

/// `target = NAME { "." NAME }`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Target {
    /// The root variable.
    pub root: Ident,
    /// Field names that follow it.
    pub path: Vec<Ident>,
    /// The whole target.
    pub span: Span,
}

/// `gate_stmt = "gate" gate_arg { "," gate_arg } ":" INDENT { gate_arm } DEDENT`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GateStmt {
    /// `risk <expr>`, if written.
    pub risk: Option<Expr>,
    /// `confidence <expr>`, if written.
    pub confidence: Option<Expr>,
    /// `done <expr>`, if written.
    pub done: Option<Expr>,
    /// The arms that were written. Unwritten arms take the defaults in spec 7.6.
    pub arms: Vec<GateArm>,
    /// The statement.
    pub span: Span,
}

/// `gate_arm = ( "proceed" | "confirm" | "escalate" | "stop" ) "->" ( statement | block )`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GateArm {
    /// Which verdict this arm handles.
    pub verdict: Verdict,
    /// What it runs.
    pub body: Block,
    /// The arm.
    pub span: Span,
}

/// The four gate verdicts, in the order section 7.6 computes them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
#[allow(missing_docs, reason = "each variant is the verdict it names")]
pub enum Verdict {
    Proceed,
    Confirm,
    Escalate,
    Stop,
}

/// `shape_assign = NAME "=" "shape" [ "strict" ] ":" INDENT { shape_field } DEDENT`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ShapeStmt {
    /// The record the fields land in.
    pub target: Ident,
    /// `shape strict:` turns an overflow into an `error` pause (spec section 7.2).
    pub strict: bool,
    /// The fields.
    pub fields: Vec<ShapeField>,
    /// The statement.
    pub span: Span,
}

/// `shape_field = NAME ( command | expr ) [ "," "max" NUMBER [ "," "tail" ] ]`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ShapeField {
    /// The field's name.
    pub name: Ident,
    /// What fills it.
    pub value: Expr,
    /// Its hard token cap, if one was written.
    pub max: Option<f64>,
    /// `, tail` after the cap: keep the last tokens on overflow instead of
    /// the first (spec section 7.2).
    pub tail: bool,
    /// The field.
    pub span: Span,
}

/// An expression.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "node", rename_all = "snake_case")]
pub enum Expr {
    /// A number literal. `2k` is 2000 and `80%` is 0.8.
    Number {
        /// Its value.
        value: f64,
        /// Where it was written.
        span: Span,
    },
    /// A text literal, with its interpolations still as raw source.
    Text {
        /// The literal.
        value: TextLit,
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
    Name(Ident),
    /// `[a, b, c]`.
    List {
        /// The items.
        items: Vec<Expr>,
        /// Where it was written.
        span: Span,
    },
    /// `{ title: "x", body }`.
    Record {
        /// The fields. `{ title }` is shorthand for `{ title: title }`.
        fields: Vec<RecordField>,
        /// Where it was written.
        span: Span,
    },
    /// `x.field`.
    Field {
        /// What is accessed.
        target: Box<Expr>,
        /// The field name.
        name: Ident,
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
    /// A call in command or function form; the two are equivalent (spec 5.2).
    Call {
        /// What is called.
        callee: Box<Expr>,
        /// Positional and named arguments, in order.
        args: Vec<Arg>,
        /// Which form it was written in. Diagnostics only.
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
    /// `c is <label>`: compares a `choice` or named `level` against a bare label.
    Is {
        /// What is compared.
        target: Box<Expr>,
        /// The label, written without quotes.
        label: Ident,
        /// Where it was written.
        span: Span,
    },
    /// `[expr for x in xs if cond]` (spec section 8.2).
    Comprehension {
        /// What each item becomes.
        expr: Box<Expr>,
        /// The loop variables.
        names: Vec<Ident>,
        /// What is iterated.
        iterable: Box<Expr>,
        /// The filter, if written.
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
    /// `trail <n>`: the last n step records of the enclosing task (spec 7.5).
    Trail {
        /// How many records.
        count: f64,
        /// Where it was written.
        span: Span,
    },
    /// A judgment expression.
    Judge(Box<JudgeExpr>),
    /// `log <level> <expr> [ { fields } ]` (spec section 5.8): records the
    /// value's text form at a level, with structured fields, and evaluates to
    /// the value. It never reaches Jev and never changes control flow.
    Log {
        /// The level.
        level: LogLevel,
        /// The logged value, whose text form is the message.
        value: Box<Expr>,
        /// The structured fields, when a record follows the value.
        fields: Vec<RecordField>,
        /// Where it was written.
        span: Span,
    },
}

/// The four `log` levels, lowest first (spec section 5.8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
#[allow(missing_docs, reason = "each variant is the level it names")]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

impl LogLevel {
    /// The level a word names, if it is one of the four.
    pub fn from_word(word: &str) -> Option<LogLevel> {
        match word {
            "debug" => Some(LogLevel::Debug),
            "info" => Some(LogLevel::Info),
            "warn" => Some(LogLevel::Warn),
            "error" => Some(LogLevel::Error),
            _ => None,
        }
    }

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

impl Expr {
    /// Where this expression was written.
    pub fn span(&self) -> Span {
        match self {
            Expr::Number { span, .. }
            | Expr::Text { span, .. }
            | Expr::Bool { span, .. }
            | Expr::None { span }
            | Expr::List { span, .. }
            | Expr::Record { span, .. }
            | Expr::Field { span, .. }
            | Expr::Index { span, .. }
            | Expr::Call { span, .. }
            | Expr::Unary { span, .. }
            | Expr::Binary { span, .. }
            | Expr::Is { span, .. }
            | Expr::Comprehension { span, .. }
            | Expr::Focus { span, .. }
            | Expr::Trail { span, .. }
            | Expr::Log { span, .. } => *span,
            Expr::Name(ident) => ident.span,
            Expr::Judge(judge) => judge.span,
        }
    }
}

/// `rec_field = NAME [ ":" expr ]`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RecordField {
    /// The field's name.
    pub name: Ident,
    /// Its value. `None` means the `{ title }` shorthand.
    pub value: Option<Expr>,
    /// Where it was written.
    pub span: Span,
}

/// A call argument, positional or named.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Arg {
    /// The argument's name, for `name value` and `name: value` forms.
    pub name: Option<Ident>,
    /// Its value.
    pub value: Expr,
    /// Where it was written.
    pub span: Span,
}

/// Which syntax a call was written in. The two are equivalent (spec 5.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CallForm {
    /// `tree.create issue.branch`.
    Command,
    /// `tree.create(issue.branch)`.
    Function,
}

/// `not` and unary `-`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[allow(missing_docs, reason = "each variant is the operator it names")]
pub enum UnaryOp {
    Not,
    Neg,
}

/// The binary operators, in the precedence order of spec section 5.1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
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

/// `judge_expr = [ "each" ] subject judge_verb` (spec section 6).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct JudgeExpr {
    /// `each` asks the question once per element of a list, in one request.
    pub each: bool,
    /// What is judged. Always a path, never an arbitrary expression (spec 6.1).
    pub subject: Subject,
    /// Which question is asked.
    pub verb: JudgeVerb,
    /// The detail block that refines the question, if written (spec 6.8).
    pub detail: Option<Detail>,
    /// The whole expression.
    pub span: Span,
}

/// `subject = NAME { "." NAME | "[" expr "]" }`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Subject {
    /// The root variable, input or parameter.
    pub root: Ident,
    /// What follows it.
    pub path: Vec<SubjectStep>,
    /// The whole subject.
    pub span: Span,
}

/// One step of a subject path.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "step", rename_all = "snake_case")]
pub enum SubjectStep {
    /// `.name`.
    Field(Ident),
    /// `[expr]`.
    Index(Expr),
}

/// The three judgment verbs.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "verb", rename_all = "snake_case")]
pub enum JudgeVerb {
    /// `feels "<condition>"`: a yes/no question, answered with a probability.
    Feels {
        /// The condition.
        condition: Expr,
    },
    /// `pick:` with two to eight labels, exactly one of them the bare escape.
    Pick {
        /// The labels.
        labels: Vec<PickLabel>,
    },
    /// `rate:` with two to ten situations, low to high.
    Rate {
        /// The levels.
        levels: Vec<RateLevel>,
    },
    /// `pick among "<question>":` chooses one element of a runtime list
    /// (spec section 6.4a).
    ///
    /// Each element becomes one option, described by its text form or by the
    /// `by` field when the elements are records. `each` cannot be combined with
    /// it ([`crate::ErrorCode::PickAmongEach`]).
    PickAmong {
        /// What Jev is choosing for.
        question: Expr,
        /// `by <field>`: which field of each item is the description Jev sees.
        by: Option<Ident>,
        /// `none`: whether choosing nothing is allowed. Without it Jev must
        /// choose an element, and the program should threshold on `confidence`.
        allow_none: bool,
    },
}

/// `pick_label = NAME [ TEXT | ":" detail_block ] | ( "other" | "none" )`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PickLabel {
    /// The label. It is an API: renaming it is a breaking change (spec 6.3).
    pub name: Ident,
    /// Whether it is the bare `other` / `none` escape option.
    pub escape: bool,
    /// Its one-line description.
    pub description: Option<Expr>,
    /// Its contrastive detail, when written as a block.
    pub detail: Option<Detail>,
    /// Where it was written.
    pub span: Span,
}

/// `rate_level = [ NAME ] TEXT`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RateLevel {
    /// The level's name, if it was named. Named levels allow `l is unchanged`.
    pub name: Option<Ident>,
    /// The situation it describes. Situations stand alone (spec 6.4).
    pub situation: Expr,
    /// Where it was written.
    pub span: Span,
}

/// A detail block. The keys map directly onto Jev's instruction object (6.8).
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct Detail {
    /// `focus`: what to look at within the subject.
    pub focus: Option<Expr>,
    /// `note`: a supporting fact the question needs.
    pub note: Option<Expr>,
    /// `compare`: other state paths to read alongside the subject.
    pub compare: Option<Expr>,
    /// `yes`: short concrete examples of the positive side.
    pub yes: Option<Expr>,
    /// `no`: short concrete examples of the negative side.
    pub no: Option<Expr>,
    /// `examples`: short concrete instances of a `pick` label.
    pub examples: Option<Expr>,
    /// `what`: what a `pick` label covers.
    pub what: Option<Expr>,
    /// `not_for`: what a `pick` label does not cover.
    pub not_for: Option<Expr>,
    /// `sample true`: draw the label or level from Jev's distribution instead
    /// of taking the argmax (spec section 6.11). `feels` is never sampled.
    pub sample: Option<bool>,
    /// The whole block.
    pub span: Span,
}
