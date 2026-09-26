//! The parser's negative suite: one minimal program per check the parser owns
//! (spec section 12), plus malformed programs that are plain `syntax` errors.
//! Each test names the rule and asserts the code and the offending line.

use jevscript_syntax::{Diagnostic, ErrorCode, parse, parse_expression};

/// Wraps statements in `program t` / `task main:` so line 4 is the first
/// statement.
fn in_task(body: &str) -> String {
    let indented: Vec<String> = body.lines().map(|l| format!("  {l}")).collect();
    format!("program t\n\ntask main:\n{}\n", indented.join("\n"))
}

fn first_error(source: &str) -> Diagnostic {
    let diagnostics = parse(source).expect_err("the program is rejected");
    diagnostics
        .into_iter()
        .next()
        .expect("at least one diagnostic")
}

fn assert_error(source: &str, code: ErrorCode, line: u32) {
    let diagnostic = first_error(source);
    assert_eq!(
        diagnostic.code, code,
        "wrong code for {source:?}: {diagnostic}"
    );
    assert_eq!(
        diagnostic.span.start.line, line,
        "wrong line for {source:?}: {diagnostic}"
    );
}

#[test]
fn a_loop_without_max_is_unbounded() {
    // Spec section 5.5: `loop` and `until` require `max`.
    assert_error(&in_task("loop:\n  break"), ErrorCode::UnboundedLoop, 4);
    assert_error(
        &in_task("x = 1\nuntil x > 2:\n  break"),
        ErrorCode::UnboundedLoop,
        5,
    );
    assert_error(
        &in_task("until verify(x):\n  break"),
        ErrorCode::UnboundedLoop,
        4,
    );
}

#[test]
fn a_pick_without_an_escape_label_is_rejected() {
    // Spec section 6.3: exactly one bare `other` or `none`.
    assert_error(
        &in_task("c = x pick:\n  a \"one thing\"\n  b \"another\""),
        ErrorCode::PickNoOther,
        4,
    );
    assert_error(
        &in_task("c = x pick:\n  a \"one thing\"\n  other\n  none"),
        ErrorCode::PickNoOther,
        4,
    );
    // The escape is written bare: a description on it is a syntax error.
    assert_error(
        &in_task("c = x pick:\n  a \"one thing\"\n  other \"anything else\""),
        ErrorCode::Syntax,
        6,
    );
}

#[test]
fn a_pick_with_too_few_or_too_many_labels_is_rejected() {
    // Spec section 6.3: two to eight labels, including the escape.
    assert_error(&in_task("c = x pick:\n  other"), ErrorCode::PickArity, 4);
    let mut nine = String::from("c = x pick:\n");
    for i in 0..8 {
        nine.push_str(&format!("  l{i} \"situation {i}\"\n"));
    }
    nine.push_str("  other");
    assert_error(&in_task(&nine), ErrorCode::PickArity, 4);
}

#[test]
fn a_rate_with_too_few_or_too_many_levels_is_rejected() {
    // Spec section 6.4: two to ten levels.
    assert_error(
        &in_task("l = x rate:\n  \"only one\""),
        ErrorCode::RateArity,
        4,
    );
    let mut eleven = String::from("l = x rate:\n");
    for i in 0..11 {
        eleven.push_str(&format!("  \"situation number {i}\"\n"));
    }
    assert_error(&in_task(&eleven), ErrorCode::RateArity, 4);
}

#[test]
fn a_rate_level_that_is_only_a_degree_word_or_number_is_rejected() {
    // Spec section 6.4: levels describe situations that stand alone.
    for bare in [
        "low",
        "Medium",
        "high",
        "very low",
        "very high",
        "3",
        "0.5",
        " 42 ",
    ] {
        let source = in_task(&format!(
            "l = x rate:\n  \"nothing has changed\"\n  \"{bare}\"\n  \"the goal is met\""
        ));
        assert_error(&source, ErrorCode::RateBareDegree, 6);
    }
    // A situation that merely contains a degree word is fine.
    let ok = in_task(
        "l = x rate:\n  \"the load is low enough to ignore\"\n  \"the load is high enough to page someone\"",
    );
    parse(&ok).expect("a situation is not a bare degree");
}

#[test]
fn a_judgment_subject_must_be_a_path() {
    // Spec section 6.1: compute first, then judge.
    assert_error(
        &in_task("p = tail(x, 4) feels \"repeats an earlier attempt\""),
        ErrorCode::SubjectNotPath,
        4,
    );
    assert_error(
        &in_task("p = (a + b) feels \"is large\""),
        ErrorCode::SubjectNotPath,
        4,
    );
    assert_error(
        &in_task("p = each [a, b] feels \"is large\""),
        ErrorCode::SubjectNotPath,
        4,
    );
    // A field path, an index and a plain name are all paths.
    parse(&in_task(
        "p = obs.summary feels \"a\"\nq = files[0] feels \"b\"\nr = x feels \"c\"",
    ))
    .expect("paths are subjects");
}

#[test]
fn a_second_verify_in_one_task_is_rejected() {
    // Spec section 7.4: a task may declare `verify` once.
    assert_error(
        &in_task("until verify(a), max 2:\n  until verify(b), max 2:\n    break"),
        ErrorCode::VerifyTwice,
        5,
    );
    assert_error(
        &in_task("until verify(a), max 2:\n  break\nuntil verify(b), max 2:\n  break"),
        ErrorCode::VerifyTwice,
        6,
    );
    // One per unit: two tasks may each verify.
    let two_tasks = "program t\n\ntask a:\n  until verify(x), max 2:\n    break\n\ntask b:\n  until verify(y), max 2:\n    break\n";
    parse(two_tasks).expect("verify once per task");
}

#[test]
fn an_event_without_a_description_is_rejected() {
    // Spec section 7.8: every event needs a description; the grammar makes it
    // mandatory, so the parser is the only stage that can report it.
    let source = "program t\n\nmachine m(dev):\n  state a:\n    on go -> b\n  state b done\n";
    assert_error(source, ErrorCode::EventNoDescription, 5);
}

#[test]
fn malformed_programs_are_syntax_errors_with_exact_spans() {
    // Everything the grammar rejects that section 12 does not name.
    let cases: [(&str, u32, u32); 12] = [
        // (source, line, column)
        ("task main:\n  x = 1\n", 1, 0), // no `program` header
        ("program t\n\nin a: text\nuse \"./x.jev\" as x\n", 4, 0), // `use` after a decl
        ("program t\n\ntask main:\n  x = 1\nin a: text\n", 5, 0), // decl after a unit
        ("program t\n\nuse \"./x.jev\" as loop\n", 3, 17), // alias is reserved (3.9)
        ("program t\n\ntask main:\n  if x\n    return\n", 4, 6), // missing `:`
        ("program t\n\ntask main:\n  x = 1 +\n", 4, 9), // dangling operator
        ("program t\n\ntask main:\n  1 + 2\n", 4, 2), // an expression is not a statement
        ("program t\n\ntask main:\n  x.y = shape:\n    a 1\n", 4, 2), // shape target is a name
        (
            "program t\n\ntask main:\n  x = y feels \"a\":\n    bogus \"z\"\n",
            5,
            4,
        ), // unknown detail key
        ("program t\n\ntask main:\n  x = y feels \"{a b}\"\n", 4, 18), // malformed hole
        ("program t\n\nneeds a: robot\n", 3, 9), // unknown capability kind
        (
            "program t\n\nmachine m(d):\n  state a:\n    on go \"desc\" -> b\n  state b:\n  goal \"late\"\n",
            7,
            2,
        ), // goal after states
    ];
    for (source, line, column) in cases {
        let diagnostic = first_error(source);
        assert_eq!(
            diagnostic.code,
            ErrorCode::Syntax,
            "{source:?}: {diagnostic}"
        );
        assert_eq!(
            (diagnostic.span.start.line, diagnostic.span.start.column),
            (line, column),
            "{source:?}: {diagnostic}"
        );
        assert!(
            diagnostic.message.starts_with("expected") || diagnostic.message.contains("shape"),
            "the message names what was expected: {diagnostic}"
        );
    }
}

#[test]
fn a_malformed_hole_points_at_the_hole() {
    // Spec section 2.7: a hole is parsed as an expression where it sits.
    let source = "program t\n\ntask main:\n  dev.send \"Tests fail: {obs.}\"\n";
    let diagnostic = first_error(source);
    assert_eq!(diagnostic.code, ErrorCode::Syntax);
    assert_eq!(diagnostic.span.start.line, 4);
    // The hole starts at column 25; `}` after `obs.` is at column 29.
    assert_eq!(diagnostic.span.start.column, 29);
    assert!(diagnostic.message.contains("interpolation"), "{diagnostic}");

    let base = jevscript_syntax::Span::new(
        jevscript_syntax::Pos::new(7, 10, 100),
        jevscript_syntax::Pos::new(7, 15, 105),
    );
    let expr = parse_expression("a + b", base).expect("parses");
    assert_eq!(expr.span().start.line, 7);
    assert_eq!(expr.span().start.column, 10);
    assert_eq!(expr.span().end.column, 15);
    let error = parse_expression("", base).expect_err("an empty hole is an error");
    assert_eq!(error[0].span.start.line, 7);
    assert_eq!(error[0].span.start.column, 10);
}

#[test]
fn duplicate_name_for_a_repeated_detail_key_gate_argument_or_gate_arm() {
    // Spec 12: detail keys, gate arguments and gate arms are unique in their
    // scopes; the grammar's repetitions admit them, so the parser rejects them.
    assert_error(
        &in_task("p = m feels \"x\":\n  focus \"a\"\n  focus \"b\""),
        ErrorCode::DuplicateName,
        6,
    );
    assert_error(
        &in_task("gate risk 1, confidence 1, risk 0:\n  proceed -> x = 1"),
        ErrorCode::DuplicateName,
        4,
    );
    assert_error(
        &in_task("gate risk 1, confidence 1:\n  proceed -> x = 1\n  proceed -> x = 2"),
        ErrorCode::DuplicateName,
        6,
    );
}
