//! `log` through the compiler (spec section 5.8): the whole-program checks
//! it owns, how it lowers, and what it leaves alone — batching and shape
//! hashes. Each test names the rule.

use jevscript_compiler::{Resolver, Severity, analyze, compile_source};
use jevscript_ir::{Expr, Ir, LogLevel, Stmt};
use jevscript_syntax::{Diagnostic, ErrorCode, parse};

fn in_main(decls: &str, body: &str) -> String {
    let indented: Vec<String> = body.lines().map(|l| format!("  {l}")).collect();
    format!("program t\n{decls}\ntask main:\n{}\n", indented.join("\n"))
}

fn first_error(source: &str) -> Diagnostic {
    compile_source(source)
        .expect_err("the program is rejected")
        .into_iter()
        .find(Diagnostic::is_error)
        .expect("at least one error")
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

fn compiled(source: &str) -> Ir {
    compile_source(source).unwrap_or_else(|d| panic!("compiles: {d:?}"))
}

#[test]
fn a_log_without_a_level_is_log_level() {
    // Spec 5.8: `log "x"` reads as a call to an unassigned `log`; the
    // checker names the missing level instead of an unassigned read.
    assert_error(&in_main("", "x = 1\nlog \"hello\""), ErrorCode::LogLevel, 5);
    // A program that declares its own unit `log` may still call it.
    let own = "program t\n\ndef log(x):\n  return x\n\ntask main:\n  y = log(1)\n  z = log \"a\"\n";
    compiled(own);
}

#[test]
fn a_log_inside_a_question_is_log_in_question() {
    // Spec 5.8: a judgment's question is built without effects, so a log
    // cannot sit in its condition, labels, levels or detail.
    let decls = "\nin message: text\n";
    assert_error(
        &in_main(decls, "a = message feels \"is {log debug message}\""),
        ErrorCode::LogInQuestion,
        6,
    );
    assert_error(
        &in_main(
            decls,
            "c = message pick:\n  billing \"about {log info message}\"\n  other",
        ),
        ErrorCode::LogInQuestion,
        7,
    );
    assert_error(
        &in_main(
            decls,
            "a = message feels \"is urgent\":\n  focus \"{log warn message}\"",
        ),
        ErrorCode::LogInQuestion,
        7,
    );
}

#[test]
fn a_log_reads_only_assigned_names() {
    // Spec 5.3 and 5.8: the value and every field, shorthand included, are
    // ordinary reads.
    assert_error(
        &in_main("", "log info missing"),
        ErrorCode::UnassignedRead,
        4,
    );
    assert_error(
        &in_main("", "log info \"x\" { missing }"),
        ErrorCode::UnassignedRead,
        4,
    );
}

#[test]
fn a_judgment_log_line_reads_the_results_above_it_and_nothing_that_acts() {
    // Spec 5.8 and 6.7: a log line sees the parameters and the results
    // written before it; it is still inside a pure judgment.
    let later = "program t\n\njudgment j(m):\n  log info b\n  b = m feels \"is polite\"\n";
    assert_error(later, ErrorCode::UnassignedRead, 4);
    let acting = "program t\n\nneeds tree: tool\n\njudgment j(m):\n  a = m feels \"is polite\"\n  log info tree.diff\n";
    assert_error(acting, ErrorCode::JudgmentSideEffect, 7);
    compiled("program t\n\njudgment j(m):\n  a = m feels \"is polite\"\n  log info a { m }\n");
}

#[test]
fn a_def_may_log_but_not_act_through_one() {
    // Spec 5.8 and 8: a log has no effect, so a def may log; what it logs
    // is still checked.
    compiled(
        "program t\n\ndef twice(x):\n  return log debug x * 2 { x }\n\ntask main:\n  y = twice(2)\n",
    );
    let acting = "program t\n\nneeds tree: tool\n\ndef d(x):\n  return log debug tree.diff\n\ntask main:\n  y = d(1)\n";
    assert_error(acting, ErrorCode::DefSideEffect, 6);
}

#[test]
fn a_log_lowers_with_its_level_value_and_fields() {
    // Spec 5.8 and 11.1: every node of a log is data in the IR.
    let ir = compiled(&in_main(
        "\nin message: text\n",
        "log error \"routed\" { message, size: len(message) }",
    ));
    let Stmt::Expr {
        expr:
            Expr::Log {
                level,
                value,
                fields,
                ..
            },
        ..
    } = &ir.task("main").expect("main").body[0]
    else {
        panic!("a log statement");
    };
    assert_eq!(*level, LogLevel::Error);
    assert!(matches!(**value, Expr::Text { .. }));
    assert_eq!(fields[0].name, "message");
    assert!(
        matches!(&fields[0].value, Expr::Name { name, .. } if name == "message"),
        "`{{ message }}` lowers to `{{ message: message }}`"
    );
    let json = serde_json::to_value(&ir.task("main").expect("main").body[0]).expect("json");
    assert_eq!(json["expr"]["node"], "log");
    assert_eq!(json["expr"]["level"], "error");
}

#[test]
fn a_log_between_judgments_keeps_them_in_one_request() {
    // Spec 5.8 and 6.6: a log is not a call, a gate or a pause. A log whose
    // value calls a unit is a call boundary through that call.
    let groups = |ir: &Ir| -> Vec<u32> {
        ir.task("main")
            .expect("main")
            .body
            .iter()
            .filter_map(|stmt| match stmt {
                Stmt::Assign {
                    value: Expr::Judge(judge),
                    ..
                } => Some(judge.request_group),
                _ => None,
            })
            .collect()
    };
    let decls = "\nin m: text\n";
    let plain = compiled(&in_main(
        decls,
        "a = m feels \"x\"\nlog info a { m }\nb = m feels \"y\"",
    ));
    assert_eq!(groups(&plain), vec![0, 0]);
    let calling = compiled(&format!(
        "{}\ndef d(x):\n  return x\n",
        in_main(decls, "a = m feels \"x\"\nlog info d(m)\nb = m feels \"y\"")
    ));
    assert_eq!(groups(&calling), vec![0, 1]);
}

#[test]
fn log_lines_do_not_change_a_judgments_shape_hash() {
    // Spec 5.8 and 11.4: the hash covers the answer space, which a log line
    // is not part of.
    let without = compiled("program t\n\njudgment j(m):\n  a = m feels \"is polite\"\n");
    let with = compiled(
        "program t\n\njudgment j(m):\n  log debug \"asking\" { m }\n  a = m feels \"is polite\"\n  log info a\n",
    );
    assert_eq!(with.judgments[0].logs.len(), 2);
    assert_eq!(with.judgments[0].logs[1].after, 1);
    assert_eq!(
        with.judgments[0].shape_hash,
        without.judgments[0].shape_hash
    );
}

#[test]
fn a_bare_prob_through_a_log_still_warns() {
    // Spec 4.1 and 5.8: `if log debug p:` tests the prob it logs.
    let source = "program t\n\ntask main:\n  m = \"hello\"\n  p = m feels \"is a greeting\"\n  if log debug p:\n    x = 1\n";
    let program = parse(source).expect("parses");
    let compilation = analyze(&program, None, &Resolver::default());
    assert!(compilation.is_ok(), "{:#?}", compilation.diagnostics);
    let warnings: Vec<ErrorCode> = compilation
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Warning)
        .map(|d| d.code)
        .collect();
    assert_eq!(warnings, vec![ErrorCode::BareProbCondition]);
}

#[test]
fn a_unit_named_log_warns_that_it_turns_the_log_expression_off() {
    // Spec 5.8 and 12: `log_shadowed` suggests a rename; the program compiles.
    let source = "program p\nout y\ndef log(info):\n  return info + 1\ntask main:\n  x = 3\n  y = log info x\n";
    let program = parse(source).expect("parses");
    let compilation = analyze(&program, None, &Resolver::default());
    assert!(compilation.is_ok(), "{:#?}", compilation.diagnostics);
    let warnings: Vec<(ErrorCode, u32)> = compilation
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Warning)
        .map(|d| (d.code, d.span.start.line))
        .collect();
    assert_eq!(warnings, vec![(ErrorCode::LogShadowed, 3)]);
}
