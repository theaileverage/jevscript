//! Whole-program checks (spec section 12).
//!
//! The lexer already raises [`ErrorCode::Indent`] and
//! [`ErrorCode::UppercaseIdentifier`], and the parser owns the checks that are
//! local to one construct. The checks here need the whole program in view:
//!
//! - [`ErrorCode::JudgmentSideEffect`]: a capability call, gate, or loop inside
//!   a `judgment` (spec section 3.5). The grammar leaves a judgment block no
//!   room for a statement, so what remains is a capability or task reached
//!   from inside a question — an interpolation hole or a subject's index;
//! - [`ErrorCode::DefSideEffect`]: a capability call, gate, or pause inside a
//!   `def` (spec section 8): a verb on a capability or a handle, a `gate`,
//!   `stop` or `escalate`, and a call to a task or machine, which act;
//! - [`ErrorCode::VerbUnknown`]: a verb not defined for the capability's kind.
//!   Never raised for an open `tool`, whose verbs fail at run time with
//!   `verb_missing` instead (spec section 9.4);
//! - [`ErrorCode::UnassignedRead`]: a variable read before assignment (5.3).
//!   Inputs, parameters, capability names, unit names, the prelude and the
//!   builtins are always readable; a loop or comprehension variable inside its
//!   body; a variable assigned in every branch of an `if` / `else` is assigned
//!   after it, one assigned in only some branches is not, and a loop body may
//!   run zero times so nothing it assigns is assigned after it;
//! - [`ErrorCode::MainHasParams`]: `task main` takes no parameters, because it
//!   reads the program's `in` declarations (spec section 7);
//! - [`ErrorCode::VerbArity`]: a `tool` call whose positional argument count
//!   does not match its declared signature (spec section 9.4). A `tool` with no
//!   signature block stays open and is checked at bind time or at run time
//!   instead;
//! - the machine checks (spec section 7.8):
//!   [`ErrorCode::MachineUnknownState`] for a `->` target that was never
//!   declared, [`ErrorCode::MachineNoDone`] for a machine with no terminal
//!   state, [`ErrorCode::EventNoDescription`] for an `on` line whose text is
//!   empty (the parser rejects a missing one), [`ErrorCode::MachineGate`] for
//!   a `gate` inside an action block, and the
//!   [`ErrorCode::MachineUnreachableDone`] *warning* for a state with no path to
//!   any terminal state;
//! - [`ErrorCode::PickAmongEach`]: `each` combined with `pick among`
//!   (spec section 6.4a);
//! - [`ErrorCode::UsePrivate`]: a `_name` reached through a module alias
//!   (spec section 3.9). The linker decides what each module can see; this
//!   pass is the one that walks every reference.
//!
//! The other module checks — `use_not_found`, `use_cycle`, `use_has_inputs`
//! and `use_needs_unmapped` — belong to [`crate::link`], which is the pass that
//! has more than one file in view.
//!
//! It also raises the codes section 12 gives the rules the spec states in
//! prose: [`ErrorCode::GateArgs`] for a `gate` without both `risk` and
//! `confidence` (7.6), [`ErrorCode::DuplicateName`] for two units, two states
//! of one machine or two events of one state with the same name,
//! [`ErrorCode::AssignImmutable`] for an assignment to an input (3.2) or a
//! capability (3.4), and [`ErrorCode::LoopControlOutside`] for `break` or
//! `continue` outside a loop.
//!
//! For `log` (spec section 5.8) it raises [`ErrorCode::LogInQuestion`] for a
//! `log` inside a judgment expression, whose texts are built without effects,
//! and [`ErrorCode::LogLevel`] for a call to an unassigned `log`, which is a
//! log written without its level (`log "x"`). The parser reports a word in
//! the level position that is not a level. Log lines in a `judgment` block
//! read the parameters and the results written above them.
//!
//! The warnings, which never fail a compile: [`ErrorCode::BareProbCondition`]
//! for a bare `prob` in a condition (4.1), [`ErrorCode::UncappedField`] for a
//! `shape` or `observe` field without `max` whose value is not obviously
//! small (7.2), [`ErrorCode::PreludeShadowed`] for a unit that shadows a
//! prelude name (3.9), [`ErrorCode::LogShadowed`] for a unit named `log`
//! (5.8), and [`ErrorCode::MachineUnreachableDone`] above.
//! [`ErrorCode::DetailIgnored`] is raised by [`crate::lower`], which is where
//! a detail key lands in a slot the IR does not have.

use std::collections::{BTreeMap, BTreeSet};

use jevscript_syntax::ast::{
    Arg, Decl, DefUnit, Detail, Expr, Ident, JudgeExpr, JudgeVerb, JudgmentUnit, MachineUnit,
    ShapeField, Stmt, TaskUnit, Unit,
};
use jevscript_syntax::token::TextPart;
use jevscript_syntax::{Diagnostic, ErrorCode, Program, Span, parse_expression_in};

use crate::link::{
    Linked, LinkedCapability, LinkedModule, PRELUDE_ALIAS, Resolution, UnitKind, kind_name,
};
use crate::lower::dotted_path;

/// The verbs each capability kind accepts (spec section 9). `tool` is absent:
/// its verbs are open.
pub const AGENT_VERBS: [&str; 5] = ["spawn", "observe", "send", "wait", "stop"];
/// The verbs a `person` accepts (spec section 9.2).
pub const PERSON_VERBS: [&str; 3] = ["ask", "notify", "take_over"];
/// The verbs an `llm` accepts (spec section 9.3).
pub const LLM_VERBS: [&str; 1] = ["write"];

/// The label a machine's Choice always carries alongside the enabled events
/// (spec section 7.8). It is reserved, so an event may not be called `stay`.
pub const STAY: &str = "stay";

/// The builtin functions of spec section 5.7, always readable.
pub const BUILTINS: [&str; 23] = [
    "len", "count", "max", "min", "sum", "top", "join", "split", "lines", "head", "tail", "tokens",
    "chunk", "hash", "now", "random", "zip", "keys", "values", "items", "text", "number", "bool",
];

/// The agent verbs that read as a property on a handle and are still calls
/// (spec section 5.2): `dev.observe`, `dev.stop`. Mirrors `batch.rs`.
const HANDLE_PROPERTY_VERBS: [&str; 2] = ["observe", "stop"];

/// The mode word of `<handle>.wait idle, minutes <n>` (spec section 9.1). It
/// is not a variable, so it is not a read.
const WAIT_MODE: &str = "idle";

/// The builtins whose result is a number or a bool, so a `shape` field that
/// is one of them needs no cap (spec section 7.2).
const SMALL_BUILTINS: [&str; 10] = [
    "len", "count", "max", "min", "sum", "tokens", "hash", "random", "number", "bool",
];

/// How many items a list literal may hold and still be "a list of fewer than
/// 64 short items" (spec section 7.2).
const SMALL_LIST: usize = 64;

/// Run the whole-program checks on one program without linking: `use`
/// aliases are visible but opaque, so references through them are not
/// checked. [`crate::compile`] runs [`check_module`] on every linked module
/// instead.
///
/// Returns every diagnostic found, errors and warnings together, in source
/// order. An empty result means the program passes.
pub fn check(program: &Program) -> Vec<Diagnostic> {
    let linked = Linked::standalone(program);
    check_module(linked.root(), &linked)
}

/// Run the whole-program checks on one linked module.
pub fn check_module(module: &LinkedModule, linked: &Linked) -> Vec<Diagnostic> {
    let mut checker = Checker {
        module,
        linked,
        inputs: module
            .program
            .decls
            .iter()
            .filter_map(|decl| match decl {
                Decl::In { name, .. } => Some(name.name.clone()),
                _ => None,
            })
            .collect(),
        units: module
            .units()
            .into_iter()
            .map(|(name, kind)| (name.to_string(), kind))
            .collect(),
        prelude: if module.alias == PRELUDE_ALIAS {
            BTreeSet::new()
        } else {
            linked
                .prelude_names()
                .into_iter()
                .map(str::to_string)
                .collect()
        },
        diagnostics: Vec::new(),
        in_question: false,
    };
    checker.program();
    let mut diagnostics = checker.diagnostics;
    diagnostics.sort_by_key(|d| (d.span.start.line, d.span.start.column));
    diagnostics.into_iter().map(|d| module.locate(d)).collect()
}

/// Whether `code` is a check this module owns.
pub fn is_whole_program_check(code: ErrorCode) -> bool {
    matches!(
        code,
        ErrorCode::JudgmentSideEffect
            | ErrorCode::DefSideEffect
            | ErrorCode::VerbUnknown
            | ErrorCode::UnassignedRead
            | ErrorCode::VerbArity
            | ErrorCode::MachineUnknownState
            | ErrorCode::MachineNoDone
            | ErrorCode::EventNoDescription
            | ErrorCode::MachineGate
            | ErrorCode::MachineUnreachableDone
            | ErrorCode::PickAmongEach
            | ErrorCode::MainHasParams
            | ErrorCode::UsePrivate
            | ErrorCode::GateArgs
            | ErrorCode::DuplicateName
            | ErrorCode::AssignImmutable
            | ErrorCode::LoopControlOutside
            | ErrorCode::LogInQuestion
            | ErrorCode::BareProbCondition
            | ErrorCode::UncappedField
            | ErrorCode::PreludeShadowed
            | ErrorCode::LogShadowed
    )
}

struct Checker<'a> {
    module: &'a LinkedModule,
    linked: &'a Linked,
    inputs: BTreeSet<String>,
    units: BTreeMap<String, UnitKind>,
    prelude: BTreeSet<String>,
    diagnostics: Vec<Diagnostic>,
    /// Whether the expressions being walked belong to a judgment
    /// expression's question, where a `log` cannot run (spec section 5.8).
    in_question: bool,
}

/// What is known inside one unit while its body is walked.
struct Scope {
    kind: UnitKind,
    /// Names readable anywhere in the unit: parameters, and `obs` in a machine.
    always: BTreeSet<String>,
    /// Variables holding a bare `prob` (assigned from a `feels`, spec 4.1).
    probs: BTreeSet<String>,
    /// Variables holding a judgment's record, by the judgment's qualified name.
    judgment_vars: BTreeMap<String, String>,
    /// How many loops enclose the current statement.
    loops: usize,
    /// How many statement blocks enclose the current statement: 1 in the
    /// unit's own body, more inside any statement's block.
    depth: usize,
}

/// What a statement list leaves behind for the statements after it.
struct Flow {
    /// Definitely assigned when the list falls off its end.
    assigned: BTreeSet<String>,
    /// Whether every path through the list leaves the unit or the loop.
    terminates: bool,
}

impl Checker<'_> {
    fn error(&mut self, code: ErrorCode, message: impl Into<String>, span: Span) {
        self.diagnostics
            .push(Diagnostic::error(code, message, span));
    }

    fn warn(&mut self, code: ErrorCode, message: impl Into<String>, span: Span) {
        self.diagnostics
            .push(Diagnostic::warning(code, message, span));
    }

    fn duplicate(&mut self, name: &str, message: impl Into<String>, span: Span, first: Span) {
        let mut diagnostic = Diagnostic::error(ErrorCode::DuplicateName, message, span);
        diagnostic.related.push(jevscript_syntax::RelatedLocation {
            message: format!("`{name}` was first declared here"),
            span: first,
            file: self
                .module
                .path
                .as_ref()
                .map(|path| path.display().to_string()),
        });
        self.diagnostics.push(diagnostic);
    }

    fn program(&mut self) {
        let mut seen: BTreeMap<&str, Span> = BTreeMap::new();
        for unit in &self.module.program.units {
            let (name, kind) = crate::link::unit_name_and_kind(unit);
            let span = unit_name_span(unit);
            if let Some(first) = seen.get(name).copied() {
                self.duplicate(
                    name,
                    format!("{} `{name}` is declared twice", kind.keyword()),
                    span,
                    first,
                );
            } else {
                seen.insert(name, span);
            }
            if name == "log" {
                self.warn(
                    ErrorCode::LogShadowed,
                    format!(
                        "{} `log` turns off the `log` expression in this file, so `log <word> <value>` calls it instead; rename it to write logs here (spec section 5.8)",
                        kind.keyword()
                    ),
                    span,
                );
            }
            if self.prelude.contains(name) {
                self.warn(
                    ErrorCode::PreludeShadowed,
                    format!(
                        "{} `{name}` shadows the prelude's `{name}`; the prelude's stays reachable as `std.{name}` (spec section 3.9)",
                        kind.keyword()
                    ),
                    span,
                );
            }
        }
        for unit in &self.module.program.units {
            match unit {
                Unit::Judgment(judgment) => self.judgment(judgment),
                Unit::Task(task) => self.task(task),
                Unit::Def(def) => self.def(def),
                Unit::Machine(machine) => self.machine(machine),
            }
        }
    }

    /* ---------------------------------------------------------------- units */

    fn scope(&self, kind: UnitKind, params: impl IntoIterator<Item = String>) -> Scope {
        Scope {
            kind,
            always: params.into_iter().collect(),
            probs: BTreeSet::new(),
            judgment_vars: BTreeMap::new(),
            loops: 0,
            depth: 0,
        }
    }

    fn judgment(&mut self, judgment: &JudgmentUnit) {
        let mut scope = self.scope(
            UnitKind::Judgment,
            judgment.params.iter().map(|p| p.name.clone()),
        );
        let assigned = BTreeSet::new();
        let mut seen = BTreeMap::new();
        for result in &judgment.results {
            if let Some(first) = seen.get(result.name.name.as_str()).copied() {
                self.duplicate(
                    &result.name.name,
                    format!("result `{}` is assigned twice", result.name.name),
                    result.name.span,
                    first,
                );
            } else {
                seen.insert(result.name.name.as_str(), result.name.span);
            }
            self.judge(&mut scope, &assigned, &result.question);
        }
        // A log line runs after the answers arrive and reads the results
        // written above it (spec section 5.8).
        for log in &judgment.logs {
            let above: BTreeSet<String> = judgment.results[..log.after]
                .iter()
                .map(|result| result.name.name.clone())
                .collect();
            self.expr(&mut scope, &above, &log.log);
        }
    }

    fn task(&mut self, task: &TaskUnit) {
        if task.name.name == "main" && !task.params.is_empty() {
            self.error(
                ErrorCode::MainHasParams,
                "`task main` takes no parameters; it reads the program's `in` declarations (spec section 7)",
                task.name.span,
            );
        }
        let mut scope = self.scope(
            UnitKind::Task,
            task.params.iter().map(|p| p.name.name.clone()),
        );
        for param in &task.params {
            if let Some(default) = &param.default {
                self.expr(&mut scope, &BTreeSet::new(), default);
            }
        }
        self.block(&mut scope, &task.body.statements, &BTreeSet::new());
    }

    fn def(&mut self, def: &DefUnit) {
        let mut scope = self.scope(
            UnitKind::Def,
            def.params.iter().map(|p| p.name.name.clone()),
        );
        for param in &def.params {
            if let Some(default) = &param.default {
                self.expr(&mut scope, &BTreeSet::new(), default);
            }
        }
        self.block(&mut scope, &def.body.statements, &BTreeSet::new());
    }

    fn machine(&mut self, machine: &MachineUnit) {
        let mut scope = self.scope(
            UnitKind::Machine,
            machine.params.iter().map(|p| p.name.name.clone()),
        );
        let none = BTreeSet::new();
        if let Some(goal) = &machine.goal {
            self.expr(&mut scope, &none, goal);
        }
        // `obs` is built by `observe` and readable everywhere after it (7.8).
        for field in &machine.observe {
            self.shape_field(&mut scope, &none, field);
        }
        scope.always.insert("obs".to_string());

        let states: BTreeMap<&str, &jevscript_syntax::ast::MachineState> = machine
            .states
            .iter()
            .map(|state| (state.name.name.as_str(), state))
            .collect();
        let mut seen_states = BTreeMap::new();
        for state in &machine.states {
            if let Some(first) = seen_states.get(state.name.name.as_str()).copied() {
                self.duplicate(
                    &state.name.name,
                    format!("state `{}` is declared twice", state.name.name),
                    state.name.span,
                    first,
                );
            } else {
                seen_states.insert(state.name.name.as_str(), state.name.span);
            }
            let mut events = BTreeMap::new();
            for transition in &state.transitions {
                if transition.event.name == STAY {
                    self.error(
                        ErrorCode::Syntax,
                        "`stay` is the label every machine step offers; an event cannot use it (spec section 7.8)",
                        transition.event.span,
                    );
                }
                if let Some(first) = events.get(transition.event.name.as_str()).copied() {
                    self.duplicate(
                        &transition.event.name,
                        format!(
                            "event `{}` is declared twice in state `{}` (spec section 7.8)",
                            transition.event.name, state.name.name
                        ),
                        transition.event.span,
                        first,
                    );
                } else {
                    events.insert(transition.event.name.as_str(), transition.event.span);
                }
                if is_empty_text(&transition.description) {
                    self.error(
                        ErrorCode::EventNoDescription,
                        format!(
                            "event `{}` needs a description: it is what Jev reads (spec section 7.8)",
                            transition.event.name
                        ),
                        transition.description.span(),
                    );
                }
                self.expr(&mut scope, &none, &transition.description);
                if !states.contains_key(transition.target.name.as_str()) {
                    self.error(
                        ErrorCode::MachineUnknownState,
                        format!(
                            "`-> {}` names a state the machine does not declare (spec section 7.8)",
                            transition.target.name
                        ),
                        transition.target.span,
                    );
                }
                if let Some(when) = &transition.when {
                    self.condition(&mut scope, &none, when);
                }
                if let Some(body) = &transition.body {
                    self.block(&mut scope, &body.statements, &none);
                }
            }
        }

        if !machine.states.iter().any(|state| state.done) {
            self.error(
                ErrorCode::MachineNoDone,
                format!(
                    "machine `{}` has no `done` state, so it can never finish (spec section 7.8)",
                    machine.name.name
                ),
                machine.name.span,
            );
            return;
        }
        // Walk the transitions backwards from every terminal state; whatever
        // is never reached cannot finish.
        let mut can_finish: BTreeSet<&str> = machine
            .states
            .iter()
            .filter(|state| state.done)
            .map(|state| state.name.name.as_str())
            .collect();
        loop {
            let before = can_finish.len();
            for state in &machine.states {
                if state
                    .transitions
                    .iter()
                    .any(|t| can_finish.contains(t.target.name.as_str()))
                {
                    can_finish.insert(state.name.name.as_str());
                }
            }
            if can_finish.len() == before {
                break;
            }
        }
        for state in &machine.states {
            if !can_finish.contains(state.name.name.as_str()) {
                self.diagnostics.push(Diagnostic::warning(
                    ErrorCode::MachineUnreachableDone,
                    format!(
                        "state `{}` has no path to a `done` state (spec section 7.8)",
                        state.name.name
                    ),
                    state.name.span,
                ));
            }
        }
    }

    /* ----------------------------------------------------------- statements */

    fn block(&mut self, scope: &mut Scope, stmts: &[Stmt], assigned: &BTreeSet<String>) -> Flow {
        let mut assigned = assigned.clone();
        let mut terminates = false;
        scope.depth += 1;
        for stmt in stmts {
            if self.stmt(scope, &mut assigned, stmt) {
                terminates = true;
            }
        }
        scope.depth -= 1;
        Flow {
            assigned,
            terminates,
        }
    }

    /// Check one statement, extending `assigned` with what it definitely
    /// assigns. Returns whether it leaves the unit or the loop.
    fn stmt(&mut self, scope: &mut Scope, assigned: &mut BTreeSet<String>, stmt: &Stmt) -> bool {
        match stmt {
            Stmt::Assign { target, value, .. } => {
                let root = &target.root;
                if self.inputs.contains(&root.name) {
                    self.error(
                        ErrorCode::AssignImmutable,
                        format!("input `{}` is immutable (spec section 3.2)", root.name),
                        root.span,
                    );
                } else if self.module.capabilities.contains_key(&root.name) {
                    self.error(
                        ErrorCode::AssignImmutable,
                        format!(
                            "capability `{}` cannot be reassigned (spec section 3.4)",
                            root.name
                        ),
                        root.span,
                    );
                }
                match value {
                    Expr::Judge(judge) => {
                        self.judge(scope, assigned, judge);
                        scope.judgment_vars.remove(&root.name);
                        if target.path.is_empty()
                            && !judge.each
                            && matches!(judge.verb, JudgeVerb::Feels { .. })
                        {
                            scope.probs.insert(root.name.clone());
                        } else {
                            scope.probs.remove(&root.name);
                        }
                    }
                    _ => {
                        self.expr(scope, assigned, value);
                        scope.probs.remove(&root.name);
                        match self.judgment_called(value) {
                            Some(qualified) if target.path.is_empty() => {
                                scope.judgment_vars.insert(root.name.clone(), qualified);
                            }
                            _ => {
                                scope.judgment_vars.remove(&root.name);
                            }
                        }
                    }
                }
                if target.path.is_empty() {
                    assigned.insert(root.name.clone());
                } else {
                    self.read(scope, assigned, root);
                }
                false
            }
            Stmt::Expr { expr, .. } => {
                self.expr(scope, assigned, expr);
                false
            }
            Stmt::If {
                branches,
                otherwise,
                ..
            } => {
                let mut flows = Vec::new();
                for branch in branches {
                    self.condition(scope, assigned, &branch.test);
                    flows.push(self.block(scope, &branch.body.statements, assigned));
                }
                flows.push(match otherwise {
                    Some(body) => self.block(scope, &body.statements, assigned),
                    None => Flow {
                        assigned: assigned.clone(),
                        terminates: false,
                    },
                });
                self.merge(assigned, flows)
            }
            Stmt::For {
                names,
                iterable,
                body,
                ..
            } => {
                self.expr(scope, assigned, iterable);
                let mut inner = assigned.clone();
                inner.extend(names.iter().map(|n| n.name.clone()));
                scope.loops += 1;
                self.block(scope, &body.statements, &inner);
                scope.loops -= 1;
                false
            }
            Stmt::Loop { body, .. } => {
                scope.loops += 1;
                self.block(scope, &body.statements, assigned);
                scope.loops -= 1;
                false
            }
            Stmt::Until {
                test,
                body,
                verify,
                span,
                ..
            } => {
                // Spec 7.4: `verify` sits on an `until` directly in the task
                // body, never nested in another statement's block, a def or
                // a machine action, so that its condition is in task scope
                // for the final read.
                if *verify {
                    let placement = match scope.kind {
                        UnitKind::Task if scope.depth == 1 => None,
                        UnitKind::Task => Some("inside another statement's block"),
                        UnitKind::Def => Some("in a `def`"),
                        UnitKind::Machine => Some("in a machine action"),
                        UnitKind::Judgment => Some("in a `judgment`"),
                    };
                    if let Some(placement) = placement {
                        self.error(
                            ErrorCode::Syntax,
                            format!(
                                "`verify` goes on an `until` directly in the task body, not {placement} (spec section 7.4)"
                            ),
                            *span,
                        );
                    }
                }
                self.condition(scope, assigned, test);
                scope.loops += 1;
                self.block(scope, &body.statements, assigned);
                scope.loops -= 1;
                false
            }
            Stmt::Gate(gate) => {
                match scope.kind {
                    UnitKind::Def => self.error(
                        ErrorCode::DefSideEffect,
                        "a `def` cannot `gate`; gates belong to tasks (spec section 8)",
                        gate.span,
                    ),
                    UnitKind::Machine => self.error(
                        ErrorCode::MachineGate,
                        "an action block cannot `gate`; the machine's thresholds gate every step (spec section 7.8)",
                        gate.span,
                    ),
                    UnitKind::Task | UnitKind::Judgment => {}
                }
                if gate.risk.is_none() || gate.confidence.is_none() {
                    self.error(
                        ErrorCode::GateArgs,
                        "a gate needs both `risk` and `confidence` (spec section 7.6)",
                        gate.span,
                    );
                }
                for arg in [&gate.risk, &gate.confidence, &gate.done]
                    .into_iter()
                    .flatten()
                {
                    self.expr(scope, assigned, arg);
                }
                let mut flows = Vec::new();
                for verdict in [
                    jevscript_syntax::ast::Verdict::Proceed,
                    jevscript_syntax::ast::Verdict::Confirm,
                    jevscript_syntax::ast::Verdict::Escalate,
                    jevscript_syntax::ast::Verdict::Stop,
                ] {
                    match gate.arms.iter().find(|arm| arm.verdict == verdict) {
                        Some(arm) => flows.push(self.block(scope, &arm.body.statements, assigned)),
                        // The defaults of 7.6: `proceed` and `confirm` go on,
                        // `escalate` and `stop` end the run.
                        None => flows.push(Flow {
                            assigned: assigned.clone(),
                            terminates: matches!(
                                verdict,
                                jevscript_syntax::ast::Verdict::Escalate
                                    | jevscript_syntax::ast::Verdict::Stop
                            ),
                        }),
                    }
                }
                self.merge(assigned, flows)
            }
            Stmt::Shape(shape) => {
                if self.inputs.contains(&shape.target.name) {
                    self.error(
                        ErrorCode::AssignImmutable,
                        format!(
                            "input `{}` is immutable (spec section 3.2)",
                            shape.target.name
                        ),
                        shape.target.span,
                    );
                } else if self.module.capabilities.contains_key(&shape.target.name) {
                    self.error(
                        ErrorCode::AssignImmutable,
                        format!(
                            "capability `{}` cannot be reassigned (spec section 3.4)",
                            shape.target.name
                        ),
                        shape.target.span,
                    );
                }
                for field in &shape.fields {
                    self.shape_field(scope, assigned, field);
                }
                scope.probs.remove(&shape.target.name);
                scope.judgment_vars.remove(&shape.target.name);
                assigned.insert(shape.target.name.clone());
                false
            }
            Stmt::Return { value, span } => {
                if scope.kind == UnitKind::Machine {
                    self.error(
                        ErrorCode::Syntax,
                        "`return` belongs to defs and tasks, not machine actions (spec section 7.8)",
                        *span,
                    );
                }
                if let Some(value) = value {
                    self.expr(scope, assigned, value);
                }
                true
            }
            Stmt::Continue { span } | Stmt::Break { span } => {
                if scope.loops == 0 {
                    let word = if matches!(stmt, Stmt::Continue { .. }) {
                        "continue"
                    } else {
                        "break"
                    };
                    self.error(
                        ErrorCode::LoopControlOutside,
                        format!("`{word}` outside a loop (spec section 5.5)"),
                        *span,
                    );
                }
                true
            }
            Stmt::Stop { reason, span } | Stmt::Escalate { reason, span } => {
                if scope.kind == UnitKind::Def {
                    let word = if matches!(stmt, Stmt::Stop { .. }) {
                        "stop"
                    } else {
                        "escalate"
                    };
                    self.error(
                        ErrorCode::DefSideEffect,
                        format!("a `def` cannot `{word}`; pauses belong to tasks (spec section 8)"),
                        *span,
                    );
                }
                self.expr(scope, assigned, reason);
                true
            }
        }
    }

    /// After alternatives: what every non-terminating path assigned. If every
    /// path terminates, so does the whole.
    fn merge(&mut self, assigned: &mut BTreeSet<String>, flows: Vec<Flow>) -> bool {
        let mut open = flows.into_iter().filter(|flow| !flow.terminates);
        let Some(first) = open.next() else {
            return true;
        };
        let mut common = first.assigned;
        for flow in open {
            common = common.intersection(&flow.assigned).cloned().collect();
        }
        *assigned = common;
        false
    }

    /// One `shape` or `observe` field (spec sections 7.2 and 7.8).
    fn shape_field(&mut self, scope: &mut Scope, assigned: &BTreeSet<String>, field: &ShapeField) {
        self.expr(scope, assigned, &field.value);
        if field.max.is_none() && !is_small(&field.value) {
            self.warn(
                ErrorCode::UncappedField,
                format!(
                    "field `{}` has no `max`; every field that reaches Jev should say how large it may be (spec section 7.2)",
                    field.name.name
                ),
                field.span,
            );
        }
    }

    /* ---------------------------------------------------------- expressions */

    /// A condition: an expression whose truthiness is tested (spec 4.1).
    fn condition(&mut self, scope: &mut Scope, assigned: &BTreeSet<String>, expr: &Expr) {
        self.bare_prob(scope, expr);
        self.expr(scope, assigned, expr);
    }

    /// Warn on a bare `prob` wherever truthiness reaches it through `and`,
    /// `or` and `not` (spec section 4.1).
    fn bare_prob(&mut self, scope: &Scope, expr: &Expr) {
        match expr {
            Expr::Name(ident) if scope.probs.contains(&ident.name) => self.warn(
                ErrorCode::BareProbCondition,
                format!(
                    "`{}` is a prob; `if {0}:` tests `{0} != 0`, write a threshold such as `{0} > 0.7` (spec section 4.1)",
                    ident.name
                ),
                ident.span,
            ),
            Expr::Field { target, name, span } => {
                if let Expr::Name(root) = &**target
                    && let Some(judgment) = scope.judgment_vars.get(&root.name)
                    && self.is_feels_result(judgment, &name.name)
                {
                    self.warn(
                        ErrorCode::BareProbCondition,
                        format!(
                            "`{}.{}` is a prob; write a threshold such as `{0}.{1} > 0.7` (spec section 4.1)",
                            root.name, name.name
                        ),
                        *span,
                    );
                }
            }
            Expr::Unary {
                op: jevscript_syntax::ast::UnaryOp::Not,
                operand,
                ..
            } => self.bare_prob(scope, operand),
            Expr::Binary {
                op: jevscript_syntax::ast::BinaryOp::And | jevscript_syntax::ast::BinaryOp::Or,
                left,
                right,
                ..
            } => {
                self.bare_prob(scope, left);
                self.bare_prob(scope, right);
            }
            // A `log` evaluates to its value (spec section 5.8).
            Expr::Log { value, .. } => self.bare_prob(scope, value),
            _ => {}
        }
    }

    /// Whether `result` of the judgment `qualified` is a bare `feels`.
    fn is_feels_result(&self, qualified: &str, result: &str) -> bool {
        let Some(Unit::Judgment(judgment)) = self.linked.unit(qualified) else {
            return false;
        };
        judgment.results.iter().any(|r| {
            r.name.name == result
                && !r.question.each
                && matches!(r.question.verb, JudgeVerb::Feels { .. })
        })
    }

    /// The qualified judgment a call expression runs, if it runs one.
    fn judgment_called(&self, expr: &Expr) -> Option<String> {
        let Expr::Call { callee, .. } = expr else {
            return None;
        };
        let path = dotted_path(callee)?;
        let segments: Vec<&str> = path.iter().map(|i| i.name.as_str()).collect();
        match self.linked.resolve(self.module, &segments) {
            Some(Resolution::Unit {
                qualified,
                kind: UnitKind::Judgment,
            }) => Some(qualified),
            _ => None,
        }
    }

    fn expr(&mut self, scope: &mut Scope, assigned: &BTreeSet<String>, expr: &Expr) {
        match expr {
            Expr::Number { .. } | Expr::Bool { .. } | Expr::None { .. } | Expr::Trail { .. } => {}
            Expr::Text { value, .. } => {
                for part in &value.parts {
                    if let TextPart::Interpolation { source, span } = part
                        && let Ok(hole) = parse_expression_in(
                            source,
                            *span,
                            self.module.program.declares_unit("log"),
                        )
                    {
                        self.expr(scope, assigned, &hole);
                    }
                }
            }
            Expr::Name(ident) => self.read(scope, assigned, ident),
            Expr::List { items, .. } => {
                for item in items {
                    self.expr(scope, assigned, item);
                }
            }
            Expr::Record { fields, .. } => {
                for field in fields {
                    match &field.value {
                        Some(value) => self.expr(scope, assigned, value),
                        None => self.read(scope, assigned, &field.name),
                    }
                }
            }
            Expr::Field { target, .. } => match dotted_path(expr) {
                Some(path) => self.path(scope, assigned, &path, None, expr.span()),
                None => self.expr(scope, assigned, target),
            },
            Expr::Index { target, index, .. } => {
                self.expr(scope, assigned, target);
                self.expr(scope, assigned, index);
            }
            Expr::Call {
                callee, args, span, ..
            } => {
                match dotted_path(callee) {
                    Some(path) => self.path(scope, assigned, &path, Some(args), *span),
                    None => self.expr(scope, assigned, callee),
                }
                let skip_mode_word = matches!(
                    dotted_path(callee).as_deref(),
                    Some([.., verb]) if verb.name == "wait"
                );
                for (index, arg) in args.iter().enumerate() {
                    if skip_mode_word
                        && index == 0
                        && arg.name.is_none()
                        && matches!(&arg.value, Expr::Name(word) if word.name == WAIT_MODE)
                    {
                        continue;
                    }
                    self.expr(scope, assigned, &arg.value);
                }
            }
            Expr::Log {
                value,
                fields,
                span,
                ..
            } => {
                if self.in_question {
                    self.error(
                        ErrorCode::LogInQuestion,
                        "a `log` cannot run inside a judgment's question, which is built without effects; log the value before or after the judgment (spec section 5.8)",
                        *span,
                    );
                }
                self.expr(scope, assigned, value);
                for field in fields {
                    match &field.value {
                        Some(value) => self.expr(scope, assigned, value),
                        None => self.read(scope, assigned, &field.name),
                    }
                }
            }
            Expr::Unary { operand, .. } => self.expr(scope, assigned, operand),
            Expr::Binary { left, right, .. } => {
                self.expr(scope, assigned, left);
                self.expr(scope, assigned, right);
            }
            Expr::Is { target, .. } => self.expr(scope, assigned, target),
            Expr::Comprehension {
                expr,
                names,
                iterable,
                test,
                ..
            } => {
                self.expr(scope, assigned, iterable);
                let mut inner = assigned.clone();
                inner.extend(names.iter().map(|n| n.name.clone()));
                if let Some(test) = test {
                    self.condition(scope, &inner, test);
                }
                self.expr(scope, &inner, expr);
            }
            Expr::Focus { text, on, .. } => {
                self.expr(scope, assigned, text);
                self.expr(scope, assigned, on);
            }
            Expr::Judge(judge) => self.judge(scope, assigned, judge),
        }
    }

    /// A dotted path `a.b.c`, read as a value or called with `args`.
    fn path(
        &mut self,
        scope: &mut Scope,
        assigned: &BTreeSet<String>,
        path: &[&Ident],
        args: Option<&[Arg]>,
        span: Span,
    ) {
        let root = path[0];
        let segments: Vec<&str> = path.iter().map(|i| i.name.as_str()).collect();

        // A module alias: the unit it names must exist and be exported.
        if path.len() >= 2 && self.module.visible.contains_key(&root.name) {
            match self.linked.resolve(self.module, &segments[..2]) {
                Some(Resolution::Unit { kind, qualified }) => {
                    if args.is_some() {
                        self.unit_call(scope, kind, &qualified, span);
                    }
                }
                Some(Resolution::Private { qualified }) => self.error(
                    ErrorCode::UsePrivate,
                    format!("`{qualified}` is private to its file: a name starting with `_` is not exported (spec section 3.9)"),
                    span,
                ),
                Some(Resolution::Missing { alias, name }) => self.error(
                    ErrorCode::UnassignedRead,
                    format!("module `{alias}` exports no unit named `{name}` (spec section 3.9)"),
                    span,
                ),
                Some(Resolution::Opaque) | None => {}
            }
            return;
        }

        // A capability: the verb must be one its kind accepts.
        if let Some(capability) = self.module.capabilities.get(&root.name).cloned() {
            if path.len() >= 2 {
                let verb = path[1];
                let positional = if path.len() == 2 {
                    args.map_or(0, |args| args.iter().filter(|a| a.name.is_none()).count())
                } else {
                    0
                };
                self.verb(&root.name, &capability, verb, positional);
                self.side_effect(scope, &format!("`{}.{}`", root.name, verb.name), span);
            }
            return;
        }

        if path.len() == 1
            && args.is_some()
            && root.name == "log"
            && !self.readable(scope, assigned, "log")
        {
            self.error(
                ErrorCode::LogLevel,
                "a `log` starts with its level and then the value: `log info \"...\"` (spec section 5.8)",
                root.span,
            );
            return;
        }
        self.read(scope, assigned, root);
        if path.len() == 1 {
            if let (Some(_), Some(Resolution::Unit { kind, qualified })) =
                (args, self.linked.resolve(self.module, &segments))
            {
                self.unit_call(scope, kind, &qualified, span);
            }
            return;
        }
        // A verb on a handle: `dev.send "..."`, `dev.stop`. Records have no
        // methods, so a call through a field is always one; a bare `observe`
        // or `stop` property is read as one too (spec section 5.2).
        let is_call = args.is_some()
            || (path.len() == 2 && HANDLE_PROPERTY_VERBS.contains(&path[1].name.as_str()));
        if is_call {
            self.side_effect(scope, &format!("`{}.{}`", root.name, path[1].name), span);
        }
    }

    /// A call to a unit by name: a task or machine acts, so a def or a
    /// judgment cannot call one.
    fn unit_call(&mut self, scope: &Scope, kind: UnitKind, qualified: &str, span: Span) {
        if matches!(kind, UnitKind::Task | UnitKind::Machine) {
            let what = format!("{} `{qualified}`", kind.keyword());
            self.side_effect(scope, &what, span);
        }
    }

    /// Something that reaches a capability, from a unit that must stay pure.
    fn side_effect(&mut self, scope: &Scope, what: &str, span: Span) {
        match scope.kind {
            UnitKind::Def => self.error(
                ErrorCode::DefSideEffect,
                format!("a `def` cannot call {what}; defs are pure (spec section 8)"),
                span,
            ),
            UnitKind::Judgment => self.error(
                ErrorCode::JudgmentSideEffect,
                format!("a `judgment` cannot call {what}; judgments are pure (spec section 3.5)"),
                span,
            ),
            UnitKind::Task | UnitKind::Machine => {}
        }
    }

    /// A verb on a declared capability (spec sections 9.1 to 9.4).
    fn verb(&mut self, name: &str, capability: &LinkedCapability, verb: &Ident, positional: usize) {
        use jevscript_syntax::ast::CapabilityKind;

        let table: &[&str] = match capability.kind {
            CapabilityKind::Agent => &AGENT_VERBS,
            CapabilityKind::Person => &PERSON_VERBS,
            CapabilityKind::Llm => &LLM_VERBS,
            CapabilityKind::Tool => {
                if capability.signatures.is_empty() {
                    return;
                }
                let Some(signature) = capability
                    .signatures
                    .iter()
                    .find(|s| s.name.name == verb.name)
                else {
                    self.error(
                        ErrorCode::VerbUnknown,
                        format!(
                            "tool `{name}` declares no verb `{}` in its signature block (spec section 9.4)",
                            verb.name
                        ),
                        verb.span,
                    );
                    return;
                };
                if signature.params.len() != positional {
                    self.error(
                        ErrorCode::VerbArity,
                        format!(
                            "`{name}.{}` takes {} positional argument{}, not {positional} (spec section 9.4)",
                            verb.name,
                            signature.params.len(),
                            if signature.params.len() == 1 { "" } else { "s" }
                        ),
                        verb.span,
                    );
                }
                return;
            }
        };
        if !table.contains(&verb.name.as_str()) {
            self.error(
                ErrorCode::VerbUnknown,
                format!(
                    "{} `{name}` has no verb `{}`; its verbs are {} (spec section 9)",
                    kind_name(capability.kind),
                    verb.name,
                    table
                        .iter()
                        .map(|v| format!("`{v}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                verb.span,
            );
        }
    }

    /// A name read as a value.
    fn read(&mut self, scope: &Scope, assigned: &BTreeSet<String>, ident: &Ident) {
        let name = ident.name.as_str();
        if !self.readable(scope, assigned, name) {
            self.error(
                ErrorCode::UnassignedRead,
                format!("`{name}` is read before it is assigned (spec section 5.3)"),
                ident.span,
            );
        }
    }

    /// Whether `name` may be read here (spec section 5.3).
    fn readable(&self, scope: &Scope, assigned: &BTreeSet<String>, name: &str) -> bool {
        assigned.contains(name)
            || scope.always.contains(name)
            || self.inputs.contains(name)
            || self.module.capabilities.contains_key(name)
            || self.units.contains_key(name)
            || self.prelude.contains(name)
            || self.module.visible.contains_key(name)
            || BUILTINS.contains(&name)
    }

    /* ------------------------------------------------------------ judgments */

    fn judge(&mut self, scope: &mut Scope, assigned: &BTreeSet<String>, judge: &JudgeExpr) {
        let outer = std::mem::replace(&mut self.in_question, true);
        self.question(scope, assigned, judge);
        self.in_question = outer;
    }

    fn question(&mut self, scope: &mut Scope, assigned: &BTreeSet<String>, judge: &JudgeExpr) {
        if judge.each && matches!(judge.verb, JudgeVerb::PickAmong { .. }) {
            self.error(
                ErrorCode::PickAmongEach,
                "`each` cannot be combined with `pick among` (spec section 6.4a)",
                judge.span,
            );
        }
        self.read(scope, assigned, &judge.subject.root);
        for step in &judge.subject.path {
            if let jevscript_syntax::ast::SubjectStep::Index(index) = step {
                self.expr(scope, assigned, index);
            }
        }
        match &judge.verb {
            JudgeVerb::Feels { condition } => self.expr(scope, assigned, condition),
            JudgeVerb::Pick { labels } => {
                for label in labels {
                    if let Some(description) = &label.description {
                        self.expr(scope, assigned, description);
                    }
                    if let Some(detail) = &label.detail {
                        self.detail(scope, assigned, detail);
                    }
                }
            }
            JudgeVerb::Rate { levels } => {
                for level in levels {
                    self.expr(scope, assigned, &level.situation);
                }
            }
            JudgeVerb::PickAmong { question, .. } => self.expr(scope, assigned, question),
        }
        if let Some(detail) = &judge.detail {
            self.detail(scope, assigned, detail);
        }
    }

    fn detail(&mut self, scope: &mut Scope, assigned: &BTreeSet<String>, detail: &Detail) {
        for value in [
            &detail.focus,
            &detail.note,
            &detail.compare,
            &detail.yes,
            &detail.no,
            &detail.examples,
            &detail.what,
            &detail.not_for,
        ]
        .into_iter()
        .flatten()
        {
            self.expr(scope, assigned, value);
        }
    }
}

/// Whether a `shape` field value is obviously a number, a bool, or a short
/// list, and so needs no cap (spec section 7.2).
fn is_small(expr: &Expr) -> bool {
    match expr {
        Expr::Number { .. } | Expr::Bool { .. } | Expr::None { .. } | Expr::Is { .. } => true,
        Expr::Trail { count, .. } => (*count as usize) < SMALL_LIST,
        Expr::List { items, .. } => {
            items.len() < SMALL_LIST
                && items.iter().all(|item| match item {
                    Expr::Text { value, .. } => value.as_plain().is_some_and(|t| t.len() < 100),
                    other => is_small(other),
                })
        }
        Expr::Unary { .. } => true,
        // Every operator but `+` yields a number or a bool; `+` may join texts.
        Expr::Binary { op, .. } => !matches!(op, jevscript_syntax::ast::BinaryOp::Add),
        Expr::Call { callee, .. } => {
            matches!(&**callee, Expr::Name(name) if SMALL_BUILTINS.contains(&name.name.as_str()))
        }
        // A `focus` carries its own cap.
        Expr::Focus { .. } => true,
        // A `log` is its value (spec section 5.8).
        Expr::Log { value, .. } => is_small(value),
        _ => false,
    }
}

/// Whether a description is the empty text (spec section 7.8).
fn is_empty_text(expr: &Expr) -> bool {
    matches!(expr, Expr::Text { value, .. } if value.as_plain() == Some(""))
}

fn unit_name_span(unit: &Unit) -> Span {
    match unit {
        Unit::Judgment(j) => j.name.span,
        Unit::Task(t) => t.name.span,
        Unit::Def(d) => d.name.span,
        Unit::Machine(m) => m.name.span,
    }
}
