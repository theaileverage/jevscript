//! The batching pass (spec section 6.6).
//!
//! Every judgment expression compiles to a Jev question, and questions are
//! grouped into requests. The compiler computes the grouping; the programmer
//! does not annotate it. The rules, which are normative:
//!
//! 1. Inside a `judgment` block, all questions form one request. Always.
//! 2. Inside a `task` or `def` body, consecutive judgment expressions that do
//!    not depend on each other's answers, and are not separated by a capability
//!    call, a gate, or a pause, form one request.
//! 3. A judgment whose subject depends on an earlier judgment's answer starts a
//!    new request.
//!
//! All questions in a request see the same state, and questions never see each
//! other's answers. The runtime charges one call per request, not per question,
//! which is why speculative fan-out is the encouraged style: ask every question
//! a branch might need up front and ignore the answers the branch does not take.
//!
//! A machine step is always its own request: one Choice over the enabled events
//! plus `stay` (spec section 7.8), and judgments inside an action block batch by
//! the rules above within that block.
//!
//! The pass writes `request_group` on every [`jevscript_ir::Judge`]. Conformance
//! item 2 checks it from the recording's `request` events, so the grouping is
//! observable and has to be right.
//!
//! ## How "consecutive" and "depends" are read
//!
//! A group is a run of judgment assignments in **one statement list**. The
//! runtime executes a group by evaluating every subject in the group when it
//! reaches the group's first judgment, sending one request, and handing each
//! later judgment its answer when execution reaches it. That is only sound if
//! every subject has the same value at the first judgment as it would have at
//! its own position, so the group is closed by:
//!
//! - a statement that calls a capability, a gate, or a terminating statement
//!   (the separators the spec names; `person.ask` and `agent.wait` are
//!   capability calls, so the pauses they raise are covered);
//! - a statement that calls a named unit — a judgment, a def, a task or a
//!   machine — anywhere in its expression, because batching never crosses a
//!   call boundary (spec section 3.9): the callee issues its own requests, and
//!   a group spanning the call would reorder them. A builtin (5.7) is pure and
//!   is not a call in this sense;
//! - a statement with a `focus` expression anywhere in it, a shape field's
//!   value included, because `focus` runs the prelude's `focus_impl`, which
//!   asks Jev (spec section 7.3): it is a call boundary like any unit call;
//! - any control-flow statement (`if`, `for`, `loop`, `until`), because a group
//!   cannot span a block boundary and the next judgment is no longer
//!   consecutive with the last;
//! - a judgment whose subject reads a variable that holds, or was computed
//!   from, an answer in the open group (rule 3); or
//! - a judgment whose subject reads a variable that an intervening statement
//!   assigned, because the value at the first judgment would differ from the
//!   value at the judgment's own position (this is what "sees the same state"
//!   requires).
//!
//! Pure statements between two judgments — arithmetic, a builtin, `shape`
//! with no call in it — do not close a group.

use std::collections::BTreeSet;

use jevscript_ir::{Expr, Judgment, JudgmentResult, Machine, ShapeField, Stmt, TextPart};

use crate::check::BUILTINS;

/// The agent verbs that read as a property on a handle and are still calls
/// (spec section 5.2): `dev.observe`, `dev.stop`. A bare field access with one
/// of these names on a variable is treated as a capability call here, because
/// records have no methods and a handle is the only thing such a field can be.
const HANDLE_PROPERTY_VERBS: [&str; 2] = ["observe", "stop"];

/// Assign `request_group` to every question in a task, def or machine body.
///
/// `capabilities` are the program's capability names in scope for this unit
/// (the root's `needs`, or the mapped names inside a library). Groups are
/// numbered from `0` in statement order, uniquely within the unit.
pub fn assign_request_groups(body: &mut [Stmt], capabilities: &BTreeSet<String>) {
    let mut next = 0;
    assign_in_list(body, capabilities, &mut next);
}

/// Rule 1: every result of a `judgment` block is one request.
pub fn assign_judgment_block(judgment: &mut Judgment) {
    for result in &mut judgment.results {
        result.question.request_group = 0;
    }
}

/// Batch a machine's action blocks. The step's own Choice is not a `Judge`
/// node, so it needs no group here (spec section 7.8).
pub fn assign_machine_groups(machine: &mut Machine, capabilities: &BTreeSet<String>) {
    let mut next = 0;
    for state in &mut machine.states {
        for transition in &mut state.transitions {
            assign_in_list(&mut transition.body, capabilities, &mut next);
        }
    }
}

/// The open group in one statement list.
struct Open {
    group: u32,
    /// Variables that hold, or were computed from, an answer in this group.
    tainted: BTreeSet<String>,
    /// Variables an intervening statement assigned since the group opened.
    assigned_since: BTreeSet<String>,
}

fn assign_in_list(stmts: &mut [Stmt], capabilities: &BTreeSet<String>, next: &mut u32) {
    let mut open: Option<Open> = None;

    for stmt in stmts.iter_mut() {
        match stmt {
            Stmt::Assign {
                root,
                value: Expr::Judge(judge),
                ..
            } => {
                let reads = subject_reads(judge);
                let must_split = open.as_ref().is_some_and(|o| {
                    reads.iter().any(|name| o.tainted.contains(name))
                        || reads.iter().any(|name| o.assigned_since.contains(name))
                });
                if must_split {
                    open = None;
                }
                let o = open.get_or_insert_with(|| {
                    let group = *next;
                    *next += 1;
                    Open {
                        group,
                        tainted: BTreeSet::new(),
                        assigned_since: BTreeSet::new(),
                    }
                });
                judge.request_group = o.group;
                o.tainted.insert(root.clone());
            }
            Stmt::Assign { root, value, .. } => {
                if expr_crosses_call(value, capabilities) {
                    open = None;
                } else if let Some(o) = open.as_mut() {
                    o.assigned_since.insert(root.clone());
                    let mut reads = BTreeSet::new();
                    expr_reads(value, &mut reads);
                    if reads.iter().any(|name| o.tainted.contains(name)) {
                        o.tainted.insert(root.clone());
                    }
                }
            }
            Stmt::Expr { expr, .. } => {
                if expr_crosses_call(expr, capabilities) {
                    open = None;
                }
            }
            Stmt::Shape { target, fields, .. } => {
                if fields
                    .iter()
                    .any(|field| shape_field_calls_capability(field, capabilities))
                {
                    open = None;
                } else if let Some(o) = open.as_mut() {
                    o.assigned_since.insert(target.clone());
                    let mut reads = BTreeSet::new();
                    for field in fields.iter() {
                        expr_reads(&field.value, &mut reads);
                    }
                    if reads.iter().any(|name| o.tainted.contains(name)) {
                        o.tainted.insert(target.clone());
                    }
                }
            }
            Stmt::If {
                branches,
                otherwise,
                ..
            } => {
                open = None;
                for branch in branches.iter_mut() {
                    assign_in_list(&mut branch.body, capabilities, next);
                }
                if let Some(body) = otherwise {
                    assign_in_list(body, capabilities, next);
                }
            }
            Stmt::For { body, .. } | Stmt::Loop { body, .. } | Stmt::Until { body, .. } => {
                open = None;
                assign_in_list(body, capabilities, next);
            }
            Stmt::Gate { arms, .. } => {
                open = None;
                for arm in arms.iter_mut() {
                    assign_in_list(&mut arm.body, capabilities, next);
                }
            }
            Stmt::Return { .. }
            | Stmt::Continue { .. }
            | Stmt::Break { .. }
            | Stmt::Stop { .. }
            | Stmt::Escalate { .. } => {
                open = None;
            }
        }
    }
}

/// The variables a judgment's subject reads: its root and anything its index
/// expressions name.
fn subject_reads(judge: &jevscript_ir::Judge) -> BTreeSet<String> {
    let mut reads = BTreeSet::from([judge.subject.root.clone()]);
    for step in &judge.subject.path {
        if let jevscript_ir::SubjectStep::Index { index } = step {
            expr_reads(index, &mut reads);
        }
    }
    reads
}

fn shape_field_calls_capability(field: &ShapeField, capabilities: &BTreeSet<String>) -> bool {
    expr_crosses_call(&field.value, capabilities)
        || match &field.policy {
            jevscript_ir::ShapePolicy::Focus { on } => expr_crosses_call(on, capabilities),
            jevscript_ir::ShapePolicy::Head | jevscript_ir::ShapePolicy::Tail => false,
        }
}

/// Whether evaluating `expr` crosses a call boundary: it reaches a capability
/// or a handle, or calls a named unit.
///
/// A call whose callee is a field on anything is a verb call, since records
/// have no methods; a bare field access on a capability name is a
/// zero-argument verb call (spec section 5.2); a bare field access named like
/// a handle's property verb is treated as one too; and a call whose callee is
/// a bare name that is not a builtin is a call to a judgment, def, task or
/// machine, which lowering keyed by that name (spec section 3.9). A `focus`
/// is a call to the prelude's `focus_impl`, which issues Jev requests of its
/// own (spec section 7.3), so it is a boundary wherever it sits.
pub fn expr_crosses_call(expr: &Expr, capabilities: &BTreeSet<String>) -> bool {
    match expr {
        Expr::Call { callee, args, .. } => {
            let callee_is_call = match callee.as_ref() {
                Expr::Field { .. } => true,
                Expr::Name { name, .. } => {
                    capabilities.contains(name) || !BUILTINS.contains(&name.as_str())
                }
                _ => false,
            };
            callee_is_call
                || expr_crosses_call(callee, capabilities)
                || args
                    .iter()
                    .any(|arg| expr_crosses_call(&arg.value, capabilities))
        }
        Expr::Field { target, name, .. } => {
            let on_capability = matches!(
                target.as_ref(),
                Expr::Name { name: root, .. } if capabilities.contains(root)
            );
            on_capability
                || (HANDLE_PROPERTY_VERBS.contains(&name.as_str())
                    && !matches!(target.as_ref(), Expr::Field { .. }))
                || expr_crosses_call(target, capabilities)
        }
        Expr::Index { target, index, .. } => {
            expr_crosses_call(target, capabilities) || expr_crosses_call(index, capabilities)
        }
        Expr::List { items, .. } => items
            .iter()
            .any(|item| expr_crosses_call(item, capabilities)),
        Expr::Record { fields, .. } => fields
            .iter()
            .any(|field| expr_crosses_call(&field.value, capabilities)),
        Expr::Text { parts, .. } => parts.iter().any(|part| match part {
            TextPart::Interpolation { expr } => expr_crosses_call(expr, capabilities),
            TextPart::Literal { .. } => false,
        }),
        Expr::Unary { operand, .. } => expr_crosses_call(operand, capabilities),
        Expr::Binary { left, right, .. } => {
            expr_crosses_call(left, capabilities) || expr_crosses_call(right, capabilities)
        }
        Expr::Is { target, .. } => expr_crosses_call(target, capabilities),
        Expr::Comprehension {
            expr,
            iterable,
            test,
            ..
        } => {
            expr_crosses_call(expr, capabilities)
                || expr_crosses_call(iterable, capabilities)
                || test
                    .as_ref()
                    .is_some_and(|test| expr_crosses_call(test, capabilities))
        }
        Expr::Focus { .. } => true,
        // A log is recorded, never sent, so only what it evaluates can cross
        // a boundary (spec section 5.8).
        Expr::Log { value, fields, .. } => {
            expr_crosses_call(value, capabilities)
                || fields
                    .iter()
                    .any(|field| expr_crosses_call(&field.value, capabilities))
        }
        Expr::Judge(judge) => judge.subject.path.iter().any(|step| match step {
            jevscript_ir::SubjectStep::Index { index } => expr_crosses_call(index, capabilities),
            jevscript_ir::SubjectStep::Field { .. } => false,
        }),
        Expr::Number { .. }
        | Expr::Bool { .. }
        | Expr::None { .. }
        | Expr::Name { .. }
        | Expr::Trail { .. } => false,
    }
}

/// Every variable name `expr` reads, by root.
pub fn expr_reads(expr: &Expr, reads: &mut BTreeSet<String>) {
    match expr {
        Expr::Name { name, .. } => {
            reads.insert(name.clone());
        }
        Expr::Call { callee, args, .. } => {
            expr_reads(callee, reads);
            for arg in args {
                expr_reads(&arg.value, reads);
            }
        }
        Expr::Field { target, .. } => expr_reads(target, reads),
        Expr::Index { target, index, .. } => {
            expr_reads(target, reads);
            expr_reads(index, reads);
        }
        Expr::List { items, .. } => {
            for item in items {
                expr_reads(item, reads);
            }
        }
        Expr::Record { fields, .. } => {
            for field in fields {
                expr_reads(&field.value, reads);
            }
        }
        Expr::Text { parts, .. } => {
            for part in parts {
                if let TextPart::Interpolation { expr } = part {
                    expr_reads(expr, reads);
                }
            }
        }
        Expr::Unary { operand, .. } => expr_reads(operand, reads),
        Expr::Binary { left, right, .. } => {
            expr_reads(left, reads);
            expr_reads(right, reads);
        }
        Expr::Is { target, .. } => expr_reads(target, reads),
        Expr::Comprehension {
            expr,
            names,
            iterable,
            test,
            ..
        } => {
            expr_reads(iterable, reads);
            let mut inner = BTreeSet::new();
            expr_reads(expr, &mut inner);
            if let Some(test) = test {
                expr_reads(test, &mut inner);
            }
            for name in names {
                inner.remove(name);
            }
            reads.extend(inner);
        }
        Expr::Focus { text, on, .. } => {
            expr_reads(text, reads);
            expr_reads(on, reads);
        }
        Expr::Judge(judge) => {
            reads.extend(subject_reads(judge));
        }
        Expr::Log { value, fields, .. } => {
            expr_reads(value, reads);
            for field in fields {
                expr_reads(&field.value, reads);
            }
        }
        Expr::Number { .. } | Expr::Bool { .. } | Expr::None { .. } | Expr::Trail { .. } => {}
    }
}

/// The request groups a list of results was assigned, in order. A convenience
/// for tests and for the conformance check.
pub fn groups_of(results: &[JudgmentResult]) -> Vec<u32> {
    results
        .iter()
        .map(|result| result.question.request_group)
        .collect()
}

#[cfg(test)]
mod tests {
    use jevscript_ir::{
        Arg, Branch, CallForm, Judge, JudgeVerb, Span, Subject, SubjectStep, TextPart,
    };

    use super::*;

    fn name(n: &str) -> Expr {
        Expr::Name {
            name: n.to_string(),
            span: Span::default(),
        }
    }

    fn field(target: Expr, n: &str) -> Expr {
        Expr::Field {
            target: Box::new(target),
            name: n.to_string(),
            span: Span::default(),
        }
    }

    fn call(callee: Expr, args: Vec<Expr>) -> Expr {
        Expr::Call {
            callee: Box::new(callee),
            args: args
                .into_iter()
                .map(|value| Arg { name: None, value })
                .collect(),
            form: CallForm::Function,
            span: Span::default(),
        }
    }

    fn text(t: &str) -> Expr {
        Expr::Text {
            parts: vec![TextPart::Literal {
                value: t.to_string(),
            }],
            span: Span::default(),
        }
    }

    fn feels(root: &str, path: &[&str]) -> Expr {
        Expr::Judge(Box::new(Judge {
            each: false,
            subject: Subject {
                root: root.to_string(),
                path: path
                    .iter()
                    .map(|p| SubjectStep::Field {
                        name: (*p).to_string(),
                    })
                    .collect(),
                state_path: std::iter::once(root.to_string())
                    .chain(path.iter().map(|p| (*p).to_string()))
                    .collect::<Vec<_>>()
                    .join("."),
                span: Span::default(),
            },
            verb: JudgeVerb::Feels {
                condition: text("is urgent"),
            },
            detail: None,
            request_group: u32::MAX,
            span: Span::default(),
        }))
    }

    fn assign(root: &str, value: Expr) -> Stmt {
        Stmt::Assign {
            root: root.to_string(),
            path: Vec::new(),
            value,
            span: Span::default(),
        }
    }

    fn expr_stmt(expr: Expr) -> Stmt {
        Stmt::Expr {
            expr,
            span: Span::default(),
        }
    }

    fn groups(body: &[Stmt]) -> Vec<u32> {
        body.iter()
            .filter_map(|stmt| match stmt {
                Stmt::Assign {
                    value: Expr::Judge(judge),
                    ..
                } => Some(judge.request_group),
                _ => None,
            })
            .collect()
    }

    fn caps(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|n| (*n).to_string()).collect()
    }

    #[test]
    fn rule_1_a_judgment_block_is_one_request() {
        let mut judgment = Judgment {
            name: "triage".to_string(),
            params: vec!["message".to_string()],
            results: vec![
                JudgmentResult {
                    name: "urgent".to_string(),
                    question: match feels("message", &[]) {
                        Expr::Judge(j) => *j,
                        _ => unreachable!(),
                    },
                    span: Span::default(),
                },
                JudgmentResult {
                    name: "risky".to_string(),
                    question: match feels("message", &[]) {
                        Expr::Judge(j) => *j,
                        _ => unreachable!(),
                    },
                    span: Span::default(),
                },
            ],
            logs: Vec::new(),
            shape_hash: String::new(),
            span: Span::default(),
        };
        assign_judgment_block(&mut judgment);
        assert_eq!(groups_of(&judgment.results), vec![0, 0]);
    }

    #[test]
    fn rule_2_consecutive_independent_judgments_share_a_request() {
        // Spec 14.3: three questions over `e` in the loop body, one call.
        let mut body = vec![
            assign("urgent", feels("e", &["body"])),
            assign("owner", feels("e", &["body"])),
            assign("risky", feels("e", &["body"])),
        ];
        assign_request_groups(&mut body, &caps(&["me"]));
        assert_eq!(groups(&body), vec![0, 0, 0]);
    }

    #[test]
    fn rule_3_a_subject_that_depends_on_an_answer_starts_a_new_request() {
        let mut body = vec![assign("p", feels("x", &[])), assign("q", feels("p", &[]))];
        assign_request_groups(&mut body, &caps(&[]));
        assert_eq!(groups(&body), vec![0, 1]);
    }

    #[test]
    fn a_value_computed_from_an_answer_carries_the_dependency() {
        // `n = count(off, above 0.6)` is computed from `off`; judging `n`'s
        // record afterwards needs the answer, so it is a second request.
        let mut body = vec![
            assign("off", feels("files", &[])),
            assign("n", call(name("count"), vec![name("off")])),
            assign("big", feels("n", &[])),
        ];
        assign_request_groups(&mut body, &caps(&[]));
        assert_eq!(groups(&body), vec![0, 1]);
    }

    #[test]
    fn a_capability_call_separates_requests() {
        let mut body = vec![
            assign("a", feels("x", &[])),
            expr_stmt(call(field(name("dev"), "send"), vec![text("hi")])),
            assign("b", feels("x", &[])),
        ];
        assign_request_groups(&mut body, &caps(&["claude"]));
        assert_eq!(groups(&body), vec![0, 1]);
    }

    #[test]
    fn a_named_judgment_call_between_judgments_separates_requests() {
        // Spec 3.9: batching never crosses a call boundary. `read` issues its
        // own request, so `a` and `b` cannot share one around it.
        let mut body = vec![
            assign("a", feels("x", &[])),
            assign("j", call(name("read"), vec![name("x")])),
            assign("b", feels("x", &[])),
        ];
        assign_request_groups(&mut body, &caps(&[]));
        assert_eq!(groups(&body), vec![0, 1]);
    }

    #[test]
    fn a_named_def_call_separates_requests_even_nested_in_an_expression() {
        // Spec 3.9: the same holds for a def, wherever the call sits — as an
        // operand, inside an interpolation, or as a bare statement.
        let mut body = vec![
            assign("a", feels("x", &[])),
            assign(
                "n",
                call(
                    name("len"),
                    vec![call(name("std.repeats"), vec![name("x")])],
                ),
            ),
            assign("b", feels("x", &[])),
            expr_stmt(call(name("harness.watch"), vec![name("x")])),
            assign("c", feels("x", &[])),
            assign(
                "t",
                Expr::Text {
                    parts: vec![TextPart::Interpolation {
                        expr: call(name("stuck"), vec![name("x")]),
                    }],
                    span: Span::default(),
                },
            ),
            assign("d", feels("x", &[])),
        ];
        assign_request_groups(&mut body, &caps(&[]));
        assert_eq!(groups(&body), vec![0, 1, 2, 3]);
    }

    #[test]
    fn a_focus_separates_requests_wherever_it_sits() {
        // Spec 7.3: `focus` runs `focus_impl`, which asks Jev, so it is a
        // call boundary: bare, nested in a builtin's argument, and as a shape
        // field's value.
        let focus = || Expr::Focus {
            text: Box::new(name("x")),
            on: Box::new(text("purpose")),
            max: 1.0,
            span: Span::default(),
        };
        let mut body = vec![
            assign("a", feels("x", &[])),
            assign("shortened", focus()),
            assign("b", feels("x", &[])),
            assign("n", call(name("len"), vec![focus()])),
            assign("c", feels("x", &[])),
            Stmt::Shape {
                target: "obs".to_string(),
                strict: false,
                fields: vec![ShapeField {
                    name: "summary".to_string(),
                    value: focus(),
                    max: None,
                    policy: jevscript_ir::ShapePolicy::Focus {
                        on: text("purpose"),
                    },
                    span: Span::default(),
                }],
                span: Span::default(),
            },
            assign("d", feels("x", &[])),
        ];
        assign_request_groups(&mut body, &caps(&[]));
        assert_eq!(groups(&body), vec![0, 1, 2, 3]);
    }

    #[test]
    fn a_zero_argument_verb_on_a_capability_separates_requests() {
        // `tree.tests_pass` is a call even though it reads as a property (5.2).
        let mut body = vec![
            assign("a", feels("x", &[])),
            assign("ok", field(name("tree"), "tests_pass")),
            assign("b", feels("x", &[])),
        ];
        assign_request_groups(&mut body, &caps(&["tree"]));
        assert_eq!(groups(&body), vec![0, 1]);
    }

    #[test]
    fn a_pure_assignment_between_judgments_keeps_them_together() {
        let mut body = vec![
            assign("a", feels("x", &[])),
            assign("k", call(name("len"), vec![name("y")])),
            assign("b", feels("x", &[])),
        ];
        assign_request_groups(&mut body, &caps(&[]));
        assert_eq!(groups(&body), vec![0, 0]);
    }

    #[test]
    fn reassigning_a_later_subject_between_judgments_splits_them() {
        // Both questions must see the same state (6.6); `x` changed in between.
        let mut body = vec![
            assign("a", feels("x", &[])),
            assign("x", call(name("tail"), vec![name("x")])),
            assign("b", feels("x", &[])),
        ];
        assign_request_groups(&mut body, &caps(&[]));
        assert_eq!(groups(&body), vec![0, 1]);
    }

    #[test]
    fn a_gate_and_control_flow_separate_requests_and_blocks_start_fresh() {
        let mut body = vec![
            assign("a", feels("x", &[])),
            Stmt::If {
                branches: vec![Branch {
                    test: name("a"),
                    body: vec![assign("c", feels("x", &[])), assign("d", feels("x", &[]))],
                    span: Span::default(),
                }],
                otherwise: None,
                span: Span::default(),
            },
            assign("b", feels("x", &[])),
            Stmt::Gate {
                risk: None,
                confidence: Some(name("b")),
                done: None,
                arms: Vec::new(),
                span: Span::default(),
            },
            assign("e", feels("x", &[])),
        ];
        assign_request_groups(&mut body, &caps(&[]));
        // Outer: a=0, then the if's block gets 1, then b=2, gate, e=3.
        assert_eq!(groups(&body), vec![0, 2, 3]);
        let Stmt::If { branches, .. } = &body[1] else {
            unreachable!()
        };
        assert_eq!(groups(&branches[0].body), vec![1, 1]);
    }

    #[test]
    fn a_handle_property_verb_is_a_capability_call() {
        // `dev.observe` is a call on a handle (5.2), so it separates requests
        // even though `dev` is a variable, not a declared capability.
        let mut body = vec![
            assign("a", feels("x", &[])),
            assign("obs", field(name("dev"), "observe")),
            assign("b", feels("x", &[])),
        ];
        assign_request_groups(&mut body, &caps(&["claude"]));
        assert_eq!(groups(&body), vec![0, 1]);
        // But a record field that happens to be reached through another field
        // is not: `obs.summary` never calls anything.
        assert!(!expr_crosses_call(
            &field(name("obs"), "summary"),
            &caps(&["claude"])
        ));
    }

    #[test]
    fn a_shape_with_a_capability_call_separates_requests() {
        let mut body = vec![
            assign("a", feels("x", &[])),
            Stmt::Shape {
                target: "obs".to_string(),
                strict: false,
                fields: vec![ShapeField {
                    name: "tests".to_string(),
                    value: field(name("tree"), "test_summary"),
                    max: Some(300.0),
                    policy: jevscript_ir::ShapePolicy::Head,
                    span: Span::default(),
                }],
                span: Span::default(),
            },
            assign("b", feels("obs", &["tests"])),
        ];
        assign_request_groups(&mut body, &caps(&["tree"]));
        assert_eq!(groups(&body), vec![0, 1]);
    }

    #[test]
    fn groups_are_numbered_uniquely_within_a_unit() {
        let mut body = vec![
            assign("a", feels("x", &[])),
            Stmt::Loop {
                max: 3.0,
                body: vec![assign("b", feels("x", &[]))],
                span: Span::default(),
            },
            assign("c", feels("x", &[])),
        ];
        assign_request_groups(&mut body, &caps(&[]));
        let Stmt::Loop { body: inner, .. } = &body[1] else {
            unreachable!()
        };
        assert_eq!(groups(&body), vec![0, 2]);
        assert_eq!(groups(inner), vec![1]);
    }
}
