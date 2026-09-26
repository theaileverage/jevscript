//! Lowering: one linked module's AST to IR nodes.
//!
//! Almost every construct maps one to one onto a type in `jevscript_ir`; the
//! rules that are not mechanical are these, each with the section that fixes
//! it:
//!
//! - **Text literals** (2.7). Every `{expr}` hole is parsed again with
//!   [`jevscript_syntax::parse_expression`] and lowered, so the runtime never
//!   sees source text. The parser already rejected malformed holes, so a
//!   failure here is a bug worth a diagnostic rather than a panic.
//! - **Record shorthand** (2.6). `{ title }` becomes `{ title: title }`.
//! - **`shape` fields** (7.2). A field written `, max N, tail` has the `tail`
//!   policy, whatever its value. Otherwise the policy is `focus { on }` when
//!   the value is a `focus ... on ..., max N` expression — the lowered field
//!   keeps the `Expr::Focus` as its value, since the runtime's `focus` produces
//!   text that already fits — and `head` for anything else.
//! - **Budgets and thresholds** (7.1, 7.6). `calls` defaults to 50 when absent;
//!   the other keys stay unlimited. `risk_confirm`, `min_confidence`,
//!   `stop_confidence` and `done` land in their fields, anything else in
//!   `other`.
//! - **Machines** (7.8). `initial` defaults to the first declared state, the
//!   shape hash comes from [`jevscript_ir::machine_shape_hash`], and a
//!   transition without an action block lowers to an empty body.
//! - **Judgments** (3.9, 11.4). The shape hash is computed with
//!   [`jevscript_ir::shape_hash`] over the linked unit under its *own* name,
//!   not the alias it was imported as: section 3.9 promises that moving a
//!   judgment between files without changing it keeps its hash, and the alias
//!   is exactly what moving changes. Machines hash the same way. Every subject
//!   carries its
//!   `state_path` (`obs.summary`, `files[3]`); an index that is not a literal is
//!   rendered from its source form (`files[i]`).
//! - **Unit references** (3.9). A call to a unit — `read_agent obs`,
//!   `harness.watch(dev, title)`, `stuck(obs.recent)` — lowers to a call whose
//!   callee is `Expr::Name` holding the unit's *qualified* name: `read_agent`,
//!   `harness.watch`, `std.stuck`. Inside a library the same rewrite qualifies
//!   the library's own units, and a nested `use` qualifies transitively
//!   (`a.b.unit`). The linked IR therefore has one flat namespace, and the
//!   runtime resolves a callee name against it without knowing about modules.
//!   Only callees are rewritten; a unit name is not a value.
//! - **Capability names** (3.9). Inside a library every `Expr::Name` that names
//!   one of the library's `needs` is renamed to the importer's capability per
//!   the `with` mapping, transitively, so the flat program only mentions the
//!   root's `needs`.
//! - **Zero-argument verbs written as properties** (5.2). `dev.stop`,
//!   `tree.tests_pass` and `tree.diff.files` stay `Expr::Field`. The runtime
//!   resolves a field access on a capability name or a handle value as a
//!   zero-argument call — `batch.rs` already reads them that way — and a
//!   record has no methods, so nothing is lost by not guessing here.
//! - **Batching** (6.6). After lowering, `batch::assign_judgment_block`,
//!   `batch::assign_request_groups` and `batch::assign_machine_groups` write
//!   `request_group` on every question, with the capability names as the
//!   runtime will see them.

use std::collections::BTreeSet;

use jevscript_ir as ir;
use jevscript_syntax::ast;
use jevscript_syntax::token::TextPart as TokenPart;
use jevscript_syntax::{Diagnostic, ErrorCode, Span, parse_expression_in};

use crate::batch;
use crate::link::{Linked, LinkedModule, Resolution, qualify};

/// The default `calls` budget (spec section 7.1).
pub const DEFAULT_CALLS: f64 = 50.0;

/// The units of one module, lowered and qualified.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LoweredModule {
    /// Its judgments, in source order.
    pub judgments: Vec<ir::Judgment>,
    /// Its tasks, in source order.
    pub tasks: Vec<ir::Task>,
    /// Its defs, in source order.
    pub defs: Vec<ir::Def>,
    /// Its machines, in source order.
    pub machines: Vec<ir::Machine>,
}

/// Lower every unit of `module`. Diagnostics are only ever raised for an
/// interpolation hole that fails to parse, which the parser should already
/// have caught.
pub fn lower_module(module: &LinkedModule, linked: &Linked) -> (LoweredModule, Vec<Diagnostic>) {
    let capabilities = module
        .capabilities
        .values()
        .map(|c| c.root_name.clone())
        .collect();
    let mut lowerer = Lowerer {
        module,
        linked,
        capabilities,
        diagnostics: Vec::new(),
    };
    let mut lowered = LoweredModule::default();
    for unit in &module.program.units {
        match unit {
            ast::Unit::Judgment(judgment) => lowered.judgments.push(lowerer.judgment(judgment)),
            ast::Unit::Task(task) => lowered.tasks.push(lowerer.task(task)),
            ast::Unit::Def(def) => lowered.defs.push(lowerer.def(def)),
            ast::Unit::Machine(machine) => lowered.machines.push(lowerer.machine(machine)),
        }
    }
    (lowered, lowerer.diagnostics)
}

/// `in <name>: <shape>` (spec section 3.2).
pub fn lower_input(name: &ast::Ident, shape: &ast::Shape, span: Span) -> ir::Input {
    ir::Input {
        name: name.name.clone(),
        shape: lower_shape(shape),
        span,
    }
}

/// `out <name>` (spec section 3.3).
pub fn lower_output(name: &ast::Ident, span: Span) -> ir::Output {
    ir::Output {
        name: name.name.clone(),
        span,
    }
}

/// `needs <name>: <kind>` with its signatures (spec sections 3.4 and 9.4).
pub fn lower_need(
    name: &ast::Ident,
    kind: ast::CapabilityKind,
    signatures: &[ast::ToolSignature],
    span: Span,
) -> ir::Need {
    ir::Need {
        name: name.name.clone(),
        kind: lower_kind(kind),
        signatures: signatures.iter().map(lower_signature).collect(),
        span,
    }
}

fn lower_signature(signature: &ast::ToolSignature) -> ir::ToolSignature {
    ir::ToolSignature {
        name: signature.name.name.clone(),
        params: signature.params.iter().map(|p| p.name.clone()).collect(),
        returns: match signature.returns {
            ast::ReturnType::Text => ir::ReturnType::Text,
            ast::ReturnType::Number => ir::ReturnType::Number,
            ast::ReturnType::Bool => ir::ReturnType::Bool,
            ast::ReturnType::List => ir::ReturnType::List,
            ast::ReturnType::Record => ir::ReturnType::Record,
            ast::ReturnType::Handle => ir::ReturnType::Handle,
            ast::ReturnType::None => ir::ReturnType::None,
        },
        span: signature.span,
    }
}

/// The IR's kind for the AST's.
pub fn lower_kind(kind: ast::CapabilityKind) -> ir::CapabilityKind {
    match kind {
        ast::CapabilityKind::Agent => ir::CapabilityKind::Agent,
        ast::CapabilityKind::Person => ir::CapabilityKind::Person,
        ast::CapabilityKind::Llm => ir::CapabilityKind::Llm,
        ast::CapabilityKind::Tool => ir::CapabilityKind::Tool,
    }
}

fn lower_shape(shape: &ast::Shape) -> ir::Shape {
    match shape {
        ast::Shape::Type { name, .. } => ir::Shape::Type {
            name: match name {
                ast::TypeName::Text => ir::TypeName::Text,
                ast::TypeName::Number => ir::TypeName::Number,
                ast::TypeName::Bool => ir::TypeName::Bool,
                ast::TypeName::List => ir::TypeName::List,
                ast::TypeName::Record => ir::TypeName::Record,
            },
        },
        ast::Shape::Record { fields, .. } => ir::Shape::Record {
            fields: fields
                .iter()
                .map(|field| ir::FieldShape {
                    name: field.name.name.clone(),
                    shape: field.shape.as_ref().map(lower_shape),
                })
                .collect(),
        },
    }
}

/// The budgets a unit declared, with the `calls` default (spec section 7.1).
/// A key written twice takes its last value.
pub fn lower_budget(items: &[ast::BudgetItem]) -> ir::Budget {
    let mut budget = ir::Budget::default();
    for item in items {
        let slot = match item.key {
            ast::BudgetKey::Calls => &mut budget.calls,
            ast::BudgetKey::Minutes => &mut budget.minutes,
            ast::BudgetKey::Usd => &mut budget.usd,
            ast::BudgetKey::Steps => &mut budget.steps,
        };
        *slot = Some(item.value);
    }
    if budget.calls.is_none() {
        budget.calls = Some(DEFAULT_CALLS);
    }
    budget
}

/// The thresholds a unit declared (spec section 7.6). The four the verdict
/// reads land in their fields; the rest are kept by name.
pub fn lower_thresholds(items: &[ast::ThresholdItem]) -> ir::Thresholds {
    let mut thresholds = ir::Thresholds::default();
    for item in items {
        match item.name.name.as_str() {
            "risk_confirm" => thresholds.risk_confirm = Some(item.value),
            "min_confidence" => thresholds.min_confidence = Some(item.value),
            "stop_confidence" => thresholds.stop_confidence = Some(item.value),
            "done" => thresholds.done = Some(item.value),
            other => thresholds.other.push(ir::NamedNumber {
                name: other.to_string(),
                value: item.value,
            }),
        }
    }
    thresholds
}

struct Lowerer<'a> {
    module: &'a LinkedModule,
    linked: &'a Linked,
    /// The capability names as the runtime will see them, for batching.
    capabilities: BTreeSet<String>,
    diagnostics: Vec<Diagnostic>,
}

impl Lowerer<'_> {
    fn qualified(&self, name: &str) -> String {
        qualify(&self.module.alias, name)
    }

    /* ---------------------------------------------------------------- units */

    fn judgment(&mut self, judgment: &ast::JudgmentUnit) -> ir::Judgment {
        let name = self.qualified(&judgment.name.name);
        let results: Vec<ir::JudgmentResult> = judgment
            .results
            .iter()
            .map(|result| ir::JudgmentResult {
                name: result.name.name.clone(),
                question: self.judge(&result.question),
                span: result.span,
            })
            .collect();
        let shape_hash = ir::shape_hash(&judgment.name.name, &results);
        let logs = judgment
            .logs
            .iter()
            .map(|log| ir::JudgmentLog {
                after: log.after as u32,
                log: self.expr(&log.log),
                span: log.span,
            })
            .collect();
        let mut lowered = ir::Judgment {
            name,
            params: judgment.params.iter().map(|p| p.name.clone()).collect(),
            results,
            logs,
            shape_hash,
            span: judgment.span,
        };
        batch::assign_judgment_block(&mut lowered);
        lowered
    }

    fn task(&mut self, task: &ast::TaskUnit) -> ir::Task {
        let mut body = self.block(&task.body);
        batch::assign_request_groups(&mut body, &self.capabilities);
        ir::Task {
            name: self.qualified(&task.name.name),
            params: self.params(&task.params),
            budget: lower_budget(&task.budgets),
            thresholds: lower_thresholds(&task.thresholds),
            body,
            span: task.span,
        }
    }

    fn def(&mut self, def: &ast::DefUnit) -> ir::Def {
        let mut body = self.block(&def.body);
        batch::assign_request_groups(&mut body, &self.capabilities);
        ir::Def {
            name: self.qualified(&def.name.name),
            params: self.params(&def.params),
            body,
            span: def.span,
        }
    }

    fn machine(&mut self, machine: &ast::MachineUnit) -> ir::Machine {
        let name = self.qualified(&machine.name.name);
        let states: Vec<ir::MachineState> = machine
            .states
            .iter()
            .map(|state| ir::MachineState {
                name: state.name.name.clone(),
                done: state.done,
                transitions: state
                    .transitions
                    .iter()
                    .map(|transition| self.transition(transition))
                    .collect(),
                span: state.span,
            })
            .collect();
        let initial = machine.initial.as_ref().map_or_else(
            || {
                machine
                    .states
                    .first()
                    .map(|state| state.name.name.clone())
                    .unwrap_or_default()
            },
            |initial| initial.name.clone(),
        );
        let shape_hash = ir::machine_shape_hash(&machine.name.name, &states);
        let mut lowered = ir::Machine {
            name,
            params: self.params(&machine.params),
            budget: lower_budget(&machine.budgets),
            thresholds: lower_thresholds(&machine.thresholds),
            goal: machine.goal.as_ref().map(|goal| self.expr(goal)),
            initial,
            observe: machine
                .observe
                .iter()
                .map(|field| self.shape_field(field))
                .collect(),
            states,
            shape_hash,
            span: machine.span,
        };
        batch::assign_machine_groups(&mut lowered, &self.capabilities);
        lowered
    }

    fn transition(&mut self, transition: &ast::Transition) -> ir::Transition {
        ir::Transition {
            event: transition.event.name.clone(),
            description: self.expr(&transition.description),
            target: transition.target.name.clone(),
            when: transition.when.as_ref().map(|when| self.expr(when)),
            risky: transition.risky,
            body: transition
                .body
                .as_ref()
                .map(|body| self.block(body))
                .unwrap_or_default(),
            span: transition.span,
        }
    }

    fn params(&mut self, params: &[ast::Param]) -> Vec<ir::DefParam> {
        params
            .iter()
            .map(|param| ir::DefParam {
                name: param.name.name.clone(),
                default: param.default.as_ref().map(|default| self.expr(default)),
            })
            .collect()
    }

    /* ----------------------------------------------------------- statements */

    fn block(&mut self, block: &ast::Block) -> Vec<ir::Stmt> {
        block
            .statements
            .iter()
            .map(|stmt| self.stmt(stmt))
            .collect()
    }

    fn stmt(&mut self, stmt: &ast::Stmt) -> ir::Stmt {
        match stmt {
            ast::Stmt::Assign {
                target,
                value,
                span,
            } => ir::Stmt::Assign {
                root: target.root.name.clone(),
                path: target.path.iter().map(|p| p.name.clone()).collect(),
                value: self.expr(value),
                span: *span,
            },
            ast::Stmt::Expr { expr, span } => ir::Stmt::Expr {
                expr: self.expr(expr),
                span: *span,
            },
            ast::Stmt::If {
                branches,
                otherwise,
                span,
            } => ir::Stmt::If {
                branches: branches
                    .iter()
                    .map(|branch| ir::Branch {
                        test: self.expr(&branch.test),
                        body: self.block(&branch.body),
                        span: branch.span,
                    })
                    .collect(),
                otherwise: otherwise.as_ref().map(|body| self.block(body)),
                span: *span,
            },
            ast::Stmt::For {
                names,
                iterable,
                body,
                span,
            } => ir::Stmt::For {
                names: names.iter().map(|n| n.name.clone()).collect(),
                iterable: self.expr(iterable),
                body: self.block(body),
                span: *span,
            },
            ast::Stmt::Loop { max, body, span } => ir::Stmt::Loop {
                max: *max,
                body: self.block(body),
                span: *span,
            },
            ast::Stmt::Until {
                test,
                verify,
                max,
                body,
                span,
            } => ir::Stmt::Until {
                test: self.expr(test),
                verify: *verify,
                max: *max,
                body: self.block(body),
                span: *span,
            },
            ast::Stmt::Gate(gate) => ir::Stmt::Gate {
                risk: gate.risk.as_ref().map(|e| self.expr(e)),
                confidence: gate.confidence.as_ref().map(|e| self.expr(e)),
                done: gate.done.as_ref().map(|e| self.expr(e)),
                arms: gate
                    .arms
                    .iter()
                    .map(|arm| ir::GateArm {
                        verdict: match arm.verdict {
                            ast::Verdict::Proceed => ir::Verdict::Proceed,
                            ast::Verdict::Confirm => ir::Verdict::Confirm,
                            ast::Verdict::Escalate => ir::Verdict::Escalate,
                            ast::Verdict::Stop => ir::Verdict::Stop,
                        },
                        body: self.block(&arm.body),
                        span: arm.span,
                    })
                    .collect(),
                span: gate.span,
            },
            ast::Stmt::Shape(shape) => ir::Stmt::Shape {
                target: shape.target.name.clone(),
                strict: shape.strict,
                fields: shape
                    .fields
                    .iter()
                    .map(|field| self.shape_field(field))
                    .collect(),
                span: shape.span,
            },
            ast::Stmt::Return { value, span } => ir::Stmt::Return {
                value: value.as_ref().map(|v| self.expr(v)),
                span: *span,
            },
            ast::Stmt::Continue { span } => ir::Stmt::Continue { span: *span },
            ast::Stmt::Break { span } => ir::Stmt::Break { span: *span },
            ast::Stmt::Stop { reason, span } => ir::Stmt::Stop {
                reason: self.expr(reason),
                span: *span,
            },
            ast::Stmt::Escalate { reason, span } => ir::Stmt::Escalate {
                reason: self.expr(reason),
                span: *span,
            },
        }
    }

    /// One `shape` or `observe` field (spec sections 7.2 and 7.8).
    fn shape_field(&mut self, field: &ast::ShapeField) -> ir::ShapeField {
        let policy = match &field.value {
            _ if field.tail => ir::ShapePolicy::Tail,
            ast::Expr::Focus { on, .. } => ir::ShapePolicy::Focus { on: self.expr(on) },
            _ => ir::ShapePolicy::Head,
        };
        ir::ShapeField {
            name: field.name.name.clone(),
            value: self.expr(&field.value),
            max: field.max,
            policy,
            span: field.span,
        }
    }

    /* ---------------------------------------------------------- expressions */

    fn expr(&mut self, expr: &ast::Expr) -> ir::Expr {
        match expr {
            ast::Expr::Number { value, span } => ir::Expr::Number {
                value: *value,
                span: *span,
            },
            ast::Expr::Text { value, span } => ir::Expr::Text {
                parts: self.text_parts(value),
                span: *span,
            },
            ast::Expr::Bool { value, span } => ir::Expr::Bool {
                value: *value,
                span: *span,
            },
            ast::Expr::None { span } => ir::Expr::None { span: *span },
            ast::Expr::Name(ident) => ir::Expr::Name {
                name: self.capability_name(&ident.name),
                span: ident.span,
            },
            ast::Expr::List { items, span } => ir::Expr::List {
                items: items.iter().map(|item| self.expr(item)).collect(),
                span: *span,
            },
            ast::Expr::Record { fields, span } => ir::Expr::Record {
                fields: self.record_fields(fields),
                span: *span,
            },
            ast::Expr::Field { target, name, span } => ir::Expr::Field {
                target: Box::new(self.expr(target)),
                name: name.name.clone(),
                span: *span,
            },
            ast::Expr::Index {
                target,
                index,
                span,
            } => ir::Expr::Index {
                target: Box::new(self.expr(target)),
                index: Box::new(self.expr(index)),
                span: *span,
            },
            ast::Expr::Call {
                callee,
                args,
                form,
                span,
            } => ir::Expr::Call {
                callee: Box::new(self.callee(callee)),
                args: args
                    .iter()
                    .map(|arg| ir::Arg {
                        name: arg.name.as_ref().map(|n| n.name.clone()),
                        value: self.expr(&arg.value),
                    })
                    .collect(),
                form: match form {
                    ast::CallForm::Command => ir::CallForm::Command,
                    ast::CallForm::Function => ir::CallForm::Function,
                },
                span: *span,
            },
            ast::Expr::Unary { op, operand, span } => ir::Expr::Unary {
                op: match op {
                    ast::UnaryOp::Not => ir::UnaryOp::Not,
                    ast::UnaryOp::Neg => ir::UnaryOp::Neg,
                },
                operand: Box::new(self.expr(operand)),
                span: *span,
            },
            ast::Expr::Binary {
                op,
                left,
                right,
                span,
            } => ir::Expr::Binary {
                op: lower_binary_op(*op),
                left: Box::new(self.expr(left)),
                right: Box::new(self.expr(right)),
                span: *span,
            },
            ast::Expr::Is {
                target,
                label,
                span,
            } => ir::Expr::Is {
                target: Box::new(self.expr(target)),
                label: label.name.clone(),
                span: *span,
            },
            ast::Expr::Comprehension {
                expr,
                names,
                iterable,
                test,
                span,
            } => ir::Expr::Comprehension {
                expr: Box::new(self.expr(expr)),
                names: names.iter().map(|n| n.name.clone()).collect(),
                iterable: Box::new(self.expr(iterable)),
                test: test.as_ref().map(|test| Box::new(self.expr(test))),
                span: *span,
            },
            ast::Expr::Focus {
                text,
                on,
                max,
                span,
            } => ir::Expr::Focus {
                text: Box::new(self.expr(text)),
                on: Box::new(self.expr(on)),
                max: *max,
                span: *span,
            },
            ast::Expr::Trail { count, span } => ir::Expr::Trail {
                count: *count,
                span: *span,
            },
            ast::Expr::Judge(judge) => ir::Expr::Judge(Box::new(self.judge(judge))),
            ast::Expr::Log {
                level,
                value,
                fields,
                span,
            } => ir::Expr::Log {
                level: lower_log_level(*level),
                value: Box::new(self.expr(value)),
                fields: self.record_fields(fields),
                span: *span,
            },
        }
    }

    /// The fields of a record literal, or of a `log` (spec sections 2.6 and
    /// 5.8). `{ title }` is `{ title: title }`.
    fn record_fields(&mut self, fields: &[ast::RecordField]) -> Vec<ir::RecordField> {
        fields
            .iter()
            .map(|field| ir::RecordField {
                name: field.name.name.clone(),
                value: match &field.value {
                    Some(value) => self.expr(value),
                    None => ir::Expr::Name {
                        name: self.capability_name(&field.name.name),
                        span: field.name.span,
                    },
                },
            })
            .collect()
    }

    /// The name the runtime will see for `name`: the importer's capability
    /// when `name` is one of this module's `needs`, otherwise itself.
    fn capability_name(&self, name: &str) -> String {
        self.module
            .capabilities
            .get(name)
            .map_or_else(|| name.to_string(), |c| c.root_name.clone())
    }

    /// A call's callee: a unit reference becomes its qualified name, anything
    /// else lowers as an expression.
    fn callee(&mut self, callee: &ast::Expr) -> ir::Expr {
        if let Some(path) = dotted_path(callee)
            && path.len() <= 2
        {
            let segments: Vec<&str> = path.iter().map(|ident| ident.name.as_str()).collect();
            match self.linked.resolve(self.module, &segments) {
                Some(Resolution::Unit { qualified, .. } | Resolution::Private { qualified }) => {
                    return ir::Expr::Name {
                        name: qualified,
                        span: callee.span(),
                    };
                }
                Some(Resolution::Missing { .. } | Resolution::Opaque) | None => {}
            }
        }
        self.expr(callee)
    }

    fn text_parts(&mut self, literal: &jevscript_syntax::TextLit) -> Vec<ir::TextPart> {
        literal
            .parts
            .iter()
            .map(|part| match part {
                TokenPart::Literal(value) => ir::TextPart::Literal {
                    value: value.clone(),
                },
                TokenPart::Interpolation { source, span } => {
                    let log_is_unit = self.module.program.declares_unit("log");
                    let expr = match parse_expression_in(source, *span, log_is_unit) {
                        Ok(expr) => self.expr(&expr),
                        Err(diagnostics) => {
                            for diagnostic in diagnostics {
                                self.diagnostics.push(self.module.locate(diagnostic));
                            }
                            ir::Expr::None { span: *span }
                        }
                    };
                    ir::TextPart::Interpolation { expr }
                }
            })
            .collect()
    }

    /* ------------------------------------------------------------ judgments */

    fn judge(&mut self, judge: &ast::JudgeExpr) -> ir::Judge {
        let verb = match &judge.verb {
            ast::JudgeVerb::Feels { condition } => ir::JudgeVerb::Feels {
                condition: self.expr(condition),
            },
            ast::JudgeVerb::Pick { labels } => ir::JudgeVerb::Pick {
                labels: labels.iter().map(|label| self.pick_label(label)).collect(),
            },
            ast::JudgeVerb::Rate { levels } => ir::JudgeVerb::Rate {
                levels: levels
                    .iter()
                    .map(|level| ir::RateLevel {
                        name: level.name.as_ref().map(|n| n.name.clone()),
                        situation: self.expr(&level.situation),
                        span: level.span,
                    })
                    .collect(),
            },
            ast::JudgeVerb::PickAmong {
                question,
                by,
                allow_none,
            } => ir::JudgeVerb::PickAmong {
                question: self.expr(question),
                by: by.as_ref().map(|b| b.name.clone()),
                allow_none: *allow_none,
            },
        };
        ir::Judge {
            each: judge.each,
            subject: self.subject(&judge.subject),
            verb,
            detail: judge.detail.as_ref().and_then(|detail| self.detail(detail)),
            // Placeholder until the batching pass runs over the unit.
            request_group: u32::MAX,
            span: judge.span,
        }
    }

    fn subject(&mut self, subject: &ast::Subject) -> ir::Subject {
        let mut state_path = subject.root.name.clone();
        let path = subject
            .path
            .iter()
            .map(|step| match step {
                ast::SubjectStep::Field(name) => {
                    state_path.push('.');
                    state_path.push_str(&name.name);
                    ir::SubjectStep::Field {
                        name: name.name.clone(),
                    }
                }
                ast::SubjectStep::Index(index) => {
                    state_path.push('[');
                    state_path.push_str(&render_expr(index));
                    state_path.push(']');
                    ir::SubjectStep::Index {
                        index: self.expr(index),
                    }
                }
            })
            .collect();
        ir::Subject {
            root: subject.root.name.clone(),
            path,
            state_path,
            span: subject.span,
        }
    }

    /// A question's detail block (spec section 6.8). The label-only keys are
    /// dropped with `detail_ignored`: the IR has no place for them on a
    /// question.
    fn detail(&mut self, detail: &ast::Detail) -> Option<ir::Detail> {
        for (key, value) in [
            ("examples", &detail.examples),
            ("what", &detail.what),
            ("not_for", &detail.not_for),
        ] {
            if let Some(value) = value {
                self.warn(
                    format!("`{key}` applies to a `pick` label, not to a question; it is ignored here (spec section 6.8)"),
                    value.span(),
                );
            }
        }
        let lowered = ir::Detail {
            focus: detail.focus.as_ref().map(|e| self.expr(e)),
            note: detail.note.as_ref().map(|e| self.expr(e)),
            compare: detail.compare.as_ref().map(|e| self.expr(e)),
            yes: detail.yes.as_ref().map(|e| self.expr(e)),
            no: detail.no.as_ref().map(|e| self.expr(e)),
            sample: detail.sample,
        };
        (lowered != ir::Detail::default()).then_some(lowered)
    }

    /// A `pick` label (spec section 6.3): a text description, or a block whose
    /// `what`, `not_for` and `examples` give Jev contrastive detail.
    fn pick_label(&mut self, label: &ast::PickLabel) -> ir::PickLabel {
        let mut lowered = ir::PickLabel {
            name: label.name.name.clone(),
            escape: label.escape,
            description: label.description.as_ref().map(|d| self.expr(d)),
            what: None,
            not_for: None,
            examples: None,
            span: label.span,
        };
        if let Some(detail) = &label.detail {
            lowered.what = detail.what.as_ref().map(|e| self.expr(e));
            lowered.not_for = detail.not_for.as_ref().map(|e| self.expr(e));
            lowered.examples = detail.examples.as_ref().map(|e| self.expr(e));
            for (key, value) in [
                ("focus", &detail.focus),
                ("note", &detail.note),
                ("compare", &detail.compare),
                ("yes", &detail.yes),
                ("no", &detail.no),
            ] {
                if let Some(value) = value {
                    self.warn(
                        format!("`{key}` applies to a question, not to a `pick` label; it is ignored here (spec section 6.8)"),
                        value.span(),
                    );
                }
            }
            if detail.sample.is_some() {
                self.warn(
                    "`sample` applies to the whole `pick`, not to one label; it is ignored here (spec section 6.11)",
                    detail.span,
                );
            }
        }
        lowered
    }

    /// `detail_ignored` (spec sections 6.8 and 12): a key in a position it
    /// does not apply to.
    fn warn(&mut self, message: impl Into<String>, span: Span) {
        let diagnostic = Diagnostic::warning(ErrorCode::DetailIgnored, message, span);
        self.diagnostics.push(self.module.locate(diagnostic));
    }
}

fn lower_binary_op(op: ast::BinaryOp) -> ir::BinaryOp {
    match op {
        ast::BinaryOp::Or => ir::BinaryOp::Or,
        ast::BinaryOp::And => ir::BinaryOp::And,
        ast::BinaryOp::Eq => ir::BinaryOp::Eq,
        ast::BinaryOp::NotEq => ir::BinaryOp::NotEq,
        ast::BinaryOp::Lt => ir::BinaryOp::Lt,
        ast::BinaryOp::LtEq => ir::BinaryOp::LtEq,
        ast::BinaryOp::Gt => ir::BinaryOp::Gt,
        ast::BinaryOp::GtEq => ir::BinaryOp::GtEq,
        ast::BinaryOp::Add => ir::BinaryOp::Add,
        ast::BinaryOp::Sub => ir::BinaryOp::Sub,
        ast::BinaryOp::Mul => ir::BinaryOp::Mul,
        ast::BinaryOp::Div => ir::BinaryOp::Div,
        ast::BinaryOp::Rem => ir::BinaryOp::Rem,
    }
}

/// `a.b.c` as its identifiers, root first, when `expr` is such a path.
pub fn dotted_path(expr: &ast::Expr) -> Option<Vec<&ast::Ident>> {
    match expr {
        ast::Expr::Name(ident) => Some(vec![ident]),
        ast::Expr::Field { target, name, .. } => {
            let mut path = dotted_path(target)?;
            path.push(name);
            Some(path)
        }
        _ => None,
    }
}

/// An expression in source form, for a subject's state path (spec section
/// 6.1): `files[i]`, `files[i + 1]`. A number prints without trailing zeros,
/// as section 4.2 prints one.
pub fn render_expr(expr: &ast::Expr) -> String {
    match expr {
        ast::Expr::Number { value, .. } => render_number(*value),
        ast::Expr::Text { value, .. } => {
            let mut out = String::from("\"");
            for part in &value.parts {
                match part {
                    TokenPart::Literal(text) => out.push_str(&text.replace('"', "\\\"")),
                    TokenPart::Interpolation { source, .. } => {
                        out.push('{');
                        out.push_str(source);
                        out.push('}');
                    }
                }
            }
            out.push('"');
            out
        }
        ast::Expr::Bool { value, .. } => value.to_string(),
        ast::Expr::None { .. } => "none".to_string(),
        ast::Expr::Name(ident) => ident.name.clone(),
        ast::Expr::List { items, .. } => {
            format!(
                "[{}]",
                items.iter().map(render_expr).collect::<Vec<_>>().join(", ")
            )
        }
        ast::Expr::Record { fields, .. } => {
            let fields: Vec<String> = fields
                .iter()
                .map(|field| match &field.value {
                    Some(value) => format!("{}: {}", field.name.name, render_expr(value)),
                    None => field.name.name.clone(),
                })
                .collect();
            format!("{{ {} }}", fields.join(", "))
        }
        ast::Expr::Field { target, name, .. } => format!("{}.{}", render_expr(target), name.name),
        ast::Expr::Index { target, index, .. } => {
            format!("{}[{}]", render_expr(target), render_expr(index))
        }
        ast::Expr::Call { callee, args, .. } => {
            let args: Vec<String> = args
                .iter()
                .map(|arg| match &arg.name {
                    Some(name) => format!("{}: {}", name.name, render_expr(&arg.value)),
                    None => render_expr(&arg.value),
                })
                .collect();
            format!("{}({})", render_expr(callee), args.join(", "))
        }
        ast::Expr::Unary { op, operand, .. } => match op {
            ast::UnaryOp::Not => format!("not {}", render_expr(operand)),
            ast::UnaryOp::Neg => format!("-{}", render_expr(operand)),
        },
        ast::Expr::Binary {
            op, left, right, ..
        } => {
            let op = match op {
                ast::BinaryOp::Or => "or",
                ast::BinaryOp::And => "and",
                ast::BinaryOp::Eq => "==",
                ast::BinaryOp::NotEq => "!=",
                ast::BinaryOp::Lt => "<",
                ast::BinaryOp::LtEq => "<=",
                ast::BinaryOp::Gt => ">",
                ast::BinaryOp::GtEq => ">=",
                ast::BinaryOp::Add => "+",
                ast::BinaryOp::Sub => "-",
                ast::BinaryOp::Mul => "*",
                ast::BinaryOp::Div => "/",
                ast::BinaryOp::Rem => "%",
            };
            format!("{} {op} {}", render_expr(left), render_expr(right))
        }
        ast::Expr::Is { target, label, .. } => {
            format!("{} is {}", render_expr(target), label.name)
        }
        ast::Expr::Comprehension {
            expr,
            names,
            iterable,
            test,
            ..
        } => {
            let names: Vec<&str> = names.iter().map(|n| n.name.as_str()).collect();
            let mut out = format!(
                "[{} for {} in {}",
                render_expr(expr),
                names.join(", "),
                render_expr(iterable)
            );
            if let Some(test) = test {
                out.push_str(" if ");
                out.push_str(&render_expr(test));
            }
            out.push(']');
            out
        }
        ast::Expr::Focus { text, on, max, .. } => format!(
            "focus {} on {}, max {}",
            render_expr(text),
            render_expr(on),
            render_number(*max)
        ),
        ast::Expr::Trail { count, .. } => format!("trail {}", render_number(*count)),
        ast::Expr::Judge(judge) => render_expr(&ast::Expr::Name(judge.subject.root.clone())),
        ast::Expr::Log {
            level,
            value,
            fields,
            ..
        } => {
            let mut out = format!("log {} {}", level.as_str(), render_expr(value));
            if !fields.is_empty() {
                out.push(' ');
                out.push_str(&render_expr(&ast::Expr::Record {
                    fields: fields.clone(),
                    span: value.span(),
                }));
            }
            out
        }
    }
}

/// A `log` level as the IR spells it (spec section 5.8).
fn lower_log_level(level: ast::LogLevel) -> ir::LogLevel {
    match level {
        ast::LogLevel::Debug => ir::LogLevel::Debug,
        ast::LogLevel::Info => ir::LogLevel::Info,
        ast::LogLevel::Warn => ir::LogLevel::Warn,
        ast::LogLevel::Error => ir::LogLevel::Error,
    }
}

fn render_number(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e15 {
        format!("{}", value as i64)
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use jevscript_syntax::parse;

    use super::*;

    fn lowered(source: &str) -> LoweredModule {
        let program = parse(source).expect("parses");
        let linked = Linked::standalone(&program);
        let (module, diagnostics) = lower_module(linked.root(), &linked);
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        module
    }

    #[test]
    fn calls_default_to_fifty_and_thresholds_split_by_name() {
        // Spec 7.1: a missing `calls` is 50; 7.6: the four named thresholds.
        let module = lowered(
            "program t\n\ntask main budget minutes 5 thresholds risk_confirm 0.2, done 0.9, custom 3:\n  x = 1\n",
        );
        let task = &module.tasks[0];
        assert_eq!(task.budget.calls, Some(50.0));
        assert_eq!(task.budget.minutes, Some(5.0));
        assert_eq!(task.budget.usd, None);
        assert_eq!(task.thresholds.risk_confirm, Some(0.2));
        assert_eq!(task.thresholds.done, Some(0.9));
        assert_eq!(task.thresholds.other[0].name, "custom");
    }

    #[test]
    fn interpolation_holes_are_lowered_to_expressions() {
        // Spec 2.7: `{expr}` is an expression, not text.
        let module = lowered("program t\n\ntask main:\n  x = 1\n  y = \"n = {x + 1}\"\n");
        let ir::Stmt::Assign {
            value: ir::Expr::Text { parts, .. },
            ..
        } = &module.tasks[0].body[1]
        else {
            panic!("expected a text assignment");
        };
        assert!(matches!(
            parts.as_slice(),
            [
                ir::TextPart::Literal { value },
                ir::TextPart::Interpolation {
                    expr: ir::Expr::Binary { .. }
                }
            ] if value == "n = "
        ));
    }

    #[test]
    fn record_shorthand_is_expanded() {
        // Spec 2.6: `{ title }` is `{ title: title }`.
        let module = lowered("program t\n\ntask main:\n  title = \"x\"\n  r = { title }\n");
        let ir::Stmt::Assign {
            value: ir::Expr::Record { fields, .. },
            ..
        } = &module.tasks[0].body[1]
        else {
            panic!("expected a record assignment");
        };
        assert!(matches!(&fields[0].value, ir::Expr::Name { name, .. } if name == "title"));
    }

    #[test]
    fn a_focus_field_keeps_its_expression_and_takes_the_focus_policy() {
        // Spec 7.2: the policy is `head` unless the value is a `focus`.
        let module = lowered(
            "program t\n\ntask main:\n  t = \"x\"\n  obs = shape:\n    a  focus t on \"why\", max 2k\n    b  t, max 10\n",
        );
        let ir::Stmt::Shape { fields, .. } = &module.tasks[0].body[1] else {
            panic!("expected a shape");
        };
        assert!(matches!(fields[0].policy, ir::ShapePolicy::Focus { .. }));
        assert!(matches!(fields[0].value, ir::Expr::Focus { max, .. } if max == 2000.0));
        assert_eq!(fields[0].max, None);
        assert_eq!(fields[1].policy, ir::ShapePolicy::Head);
        assert_eq!(fields[1].max, Some(10.0));
    }

    #[test]
    fn a_tail_field_takes_the_tail_policy() {
        // Spec 7.2: `, max N, tail` keeps the last tokens on overflow.
        let module = lowered(
            "program t\n\ntask main:\n  t = \"x\"\n  obs = shape:\n    last t, max 300, tail\n",
        );
        let ir::Stmt::Shape { fields, .. } = &module.tasks[0].body[1] else {
            panic!("expected a shape");
        };
        assert_eq!(fields[0].policy, ir::ShapePolicy::Tail);
        assert_eq!(fields[0].max, Some(300.0));
    }

    #[test]
    fn an_explicit_tail_wins_over_a_focus_value() {
        // Spec 7.2 and 13: `, max N, tail` names the policy; the value's own
        // `focus` is kept as the value, not read as the policy.
        let module = lowered(
            "program t\n\ntask main:\n  t = \"x\"\n  obs = shape:\n    last focus t on \"why\", max 100, max 50, tail\n",
        );
        let ir::Stmt::Shape { fields, .. } = &module.tasks[0].body[1] else {
            panic!("expected a shape");
        };
        assert_eq!(fields[0].policy, ir::ShapePolicy::Tail);
        assert_eq!(fields[0].max, Some(50.0));
        assert!(matches!(fields[0].value, ir::Expr::Focus { max, .. } if max == 100.0));
    }

    #[test]
    fn a_subject_carries_its_state_path() {
        // Spec 6.1 and 6.5: `obs.summary`, `files[3]`, `files[i]`.
        let module = lowered(
            "program t\n\ntask main:\n  files = []\n  i = 0\n  a = each files feels \"x\"\n  b = files[3] feels \"x\"\n  c = files[i + 1] feels \"x\"\n",
        );
        let paths: Vec<&str> = module.tasks[0].body[2..]
            .iter()
            .map(|stmt| match stmt {
                ir::Stmt::Assign {
                    value: ir::Expr::Judge(judge),
                    ..
                } => judge.subject.state_path.as_str(),
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(paths, vec!["files", "files[3]", "files[i + 1]"]);
    }

    #[test]
    fn a_prelude_call_is_qualified_and_a_property_verb_stays_a_field() {
        // Spec 3.9: `stuck` is `std.stuck`; 5.2: `dev.stop` is left to the
        // runtime.
        let module = lowered(
            "program t\n\nneeds claude: agent\n\ntask main:\n  dev = claude.spawn prompt \"x\"\n  s = stuck(trail 3)\n  dev.stop\n",
        );
        let body = &module.tasks[0].body;
        assert!(matches!(
            &body[1],
            ir::Stmt::Assign { value: ir::Expr::Call { callee, .. }, .. }
                if matches!(&**callee, ir::Expr::Name { name, .. } if name == "std.stuck")
        ));
        assert!(matches!(
            &body[2],
            ir::Stmt::Expr {
                expr: ir::Expr::Field { .. },
                ..
            }
        ));
    }

    #[test]
    fn a_machine_defaults_initial_to_the_first_state() {
        // Spec 7.8: `initial` is optional and defaults to the first state.
        let module = lowered(
            "program t\n\nmachine m(x):\n  state a:\n    on go \"go\" -> b\n  state b done\n\ntask main:\n  r = m(1, max 3)\n",
        );
        let machine = &module.machines[0];
        assert_eq!(machine.initial, "a");
        assert!(machine.shape_hash.starts_with("sh1:"));
        assert!(machine.states[0].transitions[0].body.is_empty());
    }
}
