//! The batching rules of spec section 6.6 as the linked IR shows them,
//! including the call boundary of section 3.9, which `batch.rs` unit tests
//! cover on hand-built IR and these cover end to end.

use jevscript_compiler::compile_source;
use jevscript_ir::{Expr, Ir, Stmt};

/// `(variable, request_group)` of every inline judgment in `main`'s body.
fn main_groups(ir: &Ir) -> Vec<(String, u32)> {
    ir.task("main")
        .expect("main")
        .body
        .iter()
        .filter_map(|stmt| match stmt {
            Stmt::Assign {
                root,
                value: Expr::Judge(judge),
                ..
            } => Some((root.clone(), judge.request_group)),
            _ => None,
        })
        .collect()
}

#[test]
fn a_named_judgment_call_between_inline_judgments_splits_the_request() {
    // Spec 3.9: batching never crosses a call boundary. Without the split,
    // `a` and `b` would share a request around `read`'s own request.
    let source = "program t\n\njudgment read(x):\n  r = x feels \"r\"\n\ntask main:\n  x = \"x\"\n  a = x feels \"before\"\n  j = read(x)\n  b = x feels \"after\"\n";
    let ir = compile_source(source).expect("compiles");
    assert_eq!(
        main_groups(&ir),
        vec![("a".to_string(), 0), ("b".to_string(), 1)]
    );
}

#[test]
fn a_named_def_call_splits_the_request_and_a_builtin_does_not() {
    // Spec 3.9 and 5.7: a def issues its own requests; a builtin is pure.
    let source = "program t\n\ndef judge_it(x):\n  d = x feels \"d\"\n  return d\n\ntask main:\n  x = \"x\"\n  a = x feels \"before\"\n  n = len(x)\n  b = x feels \"still before\"\n  d = judge_it(x)\n  c = x feels \"after\"\n  s = stuck(trail 2)\n  e = x feels \"after the prelude\"\n";
    let ir = compile_source(source).expect("compiles");
    assert_eq!(
        main_groups(&ir),
        vec![
            ("a".to_string(), 0),
            ("b".to_string(), 0),
            ("c".to_string(), 1),
            ("e".to_string(), 2)
        ]
    );
}

#[test]
fn a_focus_between_inline_judgments_splits_the_request() {
    // Spec 7.3: `focus` executes the prelude's `focus_impl`, which asks Jev,
    // so `a` and `b` cannot share a request across it — whether the focus is
    // bare, inside a builtin's argument, or a shape field's value. An
    // ordinary builtin stays pure.
    let source = "program t\n\ntask main:\n  x = \"x\"\n  a = x feels \"before\"\n  shortened = focus x on \"purpose\", max 1\n  b = x feels \"after\"\n  n = len(focus x on \"purpose\", max 1)\n  c = x feels \"after the nested focus\"\n  obs = shape:\n    summary focus x on \"purpose\", max 1\n  d = x feels \"after the shape\"\n  k = len(x)\n  e = x feels \"after a builtin\"\n";
    let ir = compile_source(source).expect("compiles");
    assert_eq!(
        main_groups(&ir),
        vec![
            ("a".to_string(), 0),
            ("b".to_string(), 1),
            ("c".to_string(), 2),
            ("d".to_string(), 3),
            ("e".to_string(), 3)
        ]
    );
}

#[test]
fn a_task_call_nested_in_an_interpolation_splits_the_request() {
    // Spec 3.9: the boundary is wherever the call sits in the statement.
    let source = "program t\n\ntask helper(x):\n  return x\n\ntask main:\n  x = \"x\"\n  a = x feels \"before\"\n  t = \"got {helper(x)}\"\n  b = x feels \"after\"\n";
    let ir = compile_source(source).expect("compiles");
    assert_eq!(
        main_groups(&ir),
        vec![("a".to_string(), 0), ("b".to_string(), 1)]
    );
}
