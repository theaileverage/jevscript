//! The compile warnings of spec section 12, one test per code. A warning
//! never fails the compile.

use jevscript_compiler::{Resolver, Severity, analyze, compile_source};
use jevscript_syntax::{Diagnostic, ErrorCode, parse};

fn warnings(source: &str) -> Vec<Diagnostic> {
    let program = parse(source).expect("parses");
    let compilation = analyze(&program, None, &Resolver::default());
    assert!(
        compilation.is_ok(),
        "warnings must not fail the compile: {:#?}",
        compilation.diagnostics
    );
    compilation
        .diagnostics
        .into_iter()
        .filter(|d| d.severity == Severity::Warning)
        .collect()
}

#[test]
fn an_uncapped_shape_field_warns_unless_it_is_obviously_small() {
    // Spec 7.2: a field without `max` warns unless it is a number, a bool or
    // a short list.
    let source = "program t\n\nneeds tree: tool\n\ntask main:\n  t = tree.summary\n  obs = shape:\n    big    t\n    n      len(t)\n    ok     true\n    few    [1, 2, 3]\n    recent trail 6\n    capped t, max 300\n    short  focus t on \"why\", max 100\n";
    let found = warnings(source);
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!(found[0].code, ErrorCode::UncappedField);
    assert_eq!(found[0].span.start.line, 8, "{}", found[0]);
    assert!(found[0].message.contains("`big`"), "{}", found[0]);
}

#[test]
fn a_bare_prob_in_a_condition_warns() {
    // Spec 4.1: `if p:` tests `p != 0`, which is almost never meant.
    let source = "program t\n\ntask main:\n  m = \"hello\"\n  p = m feels \"is a greeting\"\n  if p:\n    x = 1\n  if p > 0.7 and not p:\n    x = 2\n  until p, max 2:\n    x = 3\n";
    let found = warnings(source);
    let lines: Vec<u32> = found.iter().map(|d| d.span.start.line).collect();
    assert_eq!(lines, vec![6, 8, 10], "{found:#?}");
    assert!(found.iter().all(|d| d.code == ErrorCode::BareProbCondition));
}

#[test]
fn a_feels_result_of_a_judgment_call_is_a_prob_too() {
    // Spec 4.1 through 6.7: `j.claims_done` is a prob when the judgment says so.
    let source = "program t\n\njudgment read(s):\n  done = s feels \"is done\"\n  files = each s feels \"is a file\"\n\ntask main:\n  m = \"x\"\n  j = read(m)\n  if j.done:\n    x = 1\n  if j.files:\n    x = 2\n";
    let found = warnings(source);
    let lines: Vec<u32> = found.iter().map(|d| d.span.start.line).collect();
    assert_eq!(lines, vec![10], "{found:#?}");
    assert_eq!(found[0].code, ErrorCode::BareProbCondition);
}

#[test]
fn a_state_with_no_path_to_done_warns() {
    // Spec 7.8: `machine_unreachable_done` is a warning, not an error.
    let source = "program t\n\nmachine m(x):\n  state a:\n    on go \"go\" -> b\n    on spin \"spin\" -> c\n  state c:\n    on again \"again\" -> c\n  state b done\n";
    let found = warnings(source);
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!(found[0].code, ErrorCode::MachineUnreachableDone);
    assert_eq!(found[0].span.start.line, 7, "{}", found[0]);
    assert!(compile_source(source).is_ok());
}

#[test]
fn a_unit_that_shadows_a_prelude_name_warns() {
    // Spec 3.9: the user's definition shadows the unqualified form; the
    // prelude's stays reachable as `std.stuck`.
    let source = "program t\n\ndef stuck(x):\n  return false\n\ntask main:\n  a = stuck(1)\n  b = std.stuck(trail 2)\n";
    let found = warnings(source);
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!(found[0].code, ErrorCode::PreludeShadowed);
    assert_eq!(found[0].span.start.line, 3, "{}", found[0]);
    assert!(found[0].message.contains("shadows"), "{}", found[0]);
    // The call resolves to the user's def, not the prelude's.
    let ir = compile_source(source).expect("compiles");
    let main = ir.task("main").unwrap();
    let mut callees = Vec::new();
    jevscript_compiler::walk_expr_in_stmts(&main.body, &mut |expr| {
        if let jevscript_ir::Expr::Call { callee, .. } = expr
            && let jevscript_ir::Expr::Name { name, .. } = &**callee
        {
            callees.push(name.clone());
        }
    });
    assert_eq!(callees, vec!["stuck", "std.stuck"]);
}

#[test]
fn a_detail_key_in_the_wrong_position_warns_detail_ignored() {
    // Spec 6.8: `examples` applies to a `pick` label, `note` to a `feels`;
    // 12: `detail_ignored` is a warning.
    let source = "program t\n\ntask main:\n  m = \"x\"\n  p = m feels \"is x\":\n    examples [\"x\"]\n  c = m pick:\n    a:\n      what \"an a\"\n      note \"a note\"\n    other\n";
    let found = warnings(source);
    let codes: Vec<(ErrorCode, u32)> = found.iter().map(|d| (d.code, d.span.start.line)).collect();
    assert_eq!(
        codes,
        vec![
            (ErrorCode::DetailIgnored, 6),
            (ErrorCode::DetailIgnored, 10)
        ],
        "{found:#?}"
    );
}

#[test]
fn a_warning_carries_no_syntax_code() {
    // Spec 12: warnings have their own codes; `syntax` is for the grammar.
    let source = "program t\n\ndef stuck(x):\n  return false\n\nmachine m(x):\n  state a:\n    on go \"go\" -> b\n    on spin \"spin\" -> c\n  state c:\n    on again \"again\" -> c\n  state b done\n\ntask main:\n  t = \"x\"\n  p = t feels \"is x\"\n  if p:\n    obs = shape:\n      big t\n";
    let found = warnings(source);
    let codes: Vec<ErrorCode> = found.iter().map(|d| d.code).collect();
    assert_eq!(
        codes,
        vec![
            ErrorCode::PreludeShadowed,
            ErrorCode::MachineUnreachableDone,
            ErrorCode::BareProbCondition,
            ErrorCode::UncappedField,
        ]
    );
}

#[test]
fn the_examples_that_warn_still_compile() {
    // Spec 14.3 and 14.4 each leave one field uncapped on purpose.
    let ir = compile_source(include_str!("../../../examples/chief_of_staff.jev"))
        .expect("compiles despite the warning");
    assert!(ir.is_runnable());
}
