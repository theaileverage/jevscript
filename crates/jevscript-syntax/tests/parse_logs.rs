//! `log` (spec section 5.8): the statement and expression forms, how far the
//! logged expression reaches, `log` as an ordinary name elsewhere, log lines
//! in a judgment block, and the parser's rejections. Each test names the rule.

use jevscript_syntax::ast::{BinaryOp, Expr, LogLevel, Stmt, Unit};
use jevscript_syntax::{Diagnostic, ErrorCode, Program, Span, parse, parse_expression};

fn in_task(body: &str) -> String {
    let indented: Vec<String> = body.lines().map(|l| format!("  {l}")).collect();
    format!("program t\n\ntask main:\n{}\n", indented.join("\n"))
}

fn statements(body: &str) -> Vec<Stmt> {
    let program: Program = parse(&in_task(body)).unwrap_or_else(|d| panic!("parses: {d:?}"));
    let Unit::Task(task) = &program.units[0] else {
        panic!("a task");
    };
    task.body.statements.clone()
}

fn assigned(body: &str) -> Expr {
    match statements(body).into_iter().next() {
        Some(Stmt::Assign { value, .. }) => value,
        other => panic!("an assignment, got {other:?}"),
    }
}

fn first_error(source: &str) -> Diagnostic {
    parse(source)
        .expect_err("the program is rejected")
        .into_iter()
        .next()
        .expect("a diagnostic")
}

#[test]
fn the_statement_form_takes_a_message_and_a_field_record() {
    // Spec 5.8: `log info "routed request" { owner, confidence: c.owner }`.
    let stmts = statements("log info \"routed request\" { owner, confidence: c.owner }");
    let Stmt::Expr {
        expr:
            Expr::Log {
                level,
                value,
                fields,
                ..
            },
        ..
    } = &stmts[0]
    else {
        panic!("a log statement, got {:?}", stmts[0]);
    };
    assert_eq!(*level, LogLevel::Info);
    assert!(matches!(**value, Expr::Text { .. }));
    let names: Vec<&str> = fields.iter().map(|f| f.name.name.as_str()).collect();
    assert_eq!(names, vec!["owner", "confidence"]);
    assert!(fields[0].value.is_none(), "`{{ owner }}` is shorthand");
    assert!(fields[1].value.is_some());
}

#[test]
fn the_expression_form_evaluates_to_the_logged_value() {
    // Spec 5.8: `x = log debug classify(message)`; the message is optional
    // in the sense that the value itself is the message.
    let Expr::Log {
        level,
        value,
        fields,
        ..
    } = assigned("x = log debug classify(message)")
    else {
        panic!("a log");
    };
    assert_eq!(level, LogLevel::Debug);
    assert!(matches!(*value, Expr::Call { .. }));
    assert!(fields.is_empty());
    for (word, level) in [
        ("debug", LogLevel::Debug),
        ("info", LogLevel::Info),
        ("warn", LogLevel::Warn),
        ("error", LogLevel::Error),
    ] {
        let Expr::Log { level: parsed, .. } = assigned(&format!("x = log {word} 1")) else {
            panic!("a log");
        };
        assert_eq!(parsed, level);
    }
}

#[test]
fn the_logged_expression_reaches_as_far_right_as_an_expression_can() {
    // Spec 5.8: the lowest precedence, like `focus`'s text; a record after
    // the value is the fields, and a `,` or `:` ends it.
    let Expr::Log { value, .. } = assigned("x = log debug a + b * 2") else {
        panic!("a log");
    };
    assert!(matches!(
        *value,
        Expr::Binary {
            op: BinaryOp::Add,
            ..
        }
    ));

    let Expr::Binary {
        op: BinaryOp::Add,
        right,
        ..
    } = assigned("x = 1 + log debug a * 2")
    else {
        panic!("`1 + (log ...)`");
    };
    assert!(matches!(*right, Expr::Log { .. }));

    let Expr::Call { args, .. } = assigned("x = f(log debug a, b)") else {
        panic!("a call");
    };
    assert_eq!(args.len(), 2);
    assert!(args[0].name.is_none(), "a log is a positional argument");
    assert!(matches!(args[0].value, Expr::Log { .. }));

    let stmts = statements("if log warn p > 0.7:\n  stop \"high\"");
    let Stmt::If { branches, .. } = &stmts[0] else {
        panic!("an if");
    };
    let Expr::Log { value, .. } = &branches[0].test else {
        panic!("the test is the log");
    };
    assert!(matches!(
        **value,
        Expr::Binary {
            op: BinaryOp::Gt,
            ..
        }
    ));

    let Expr::Log { value, fields, .. } = assigned("x = log info { a: 1 } { b }") else {
        panic!("a log");
    };
    assert!(
        matches!(*value, Expr::Record { .. }),
        "the first record is the value"
    );
    assert_eq!(fields.len(), 1, "the second is the fields");
}

#[test]
fn log_is_an_ordinary_name_outside_its_form() {
    // Spec 2.5 and 5.8: `log` is contextual. Followed by anything but a level
    // word and a value it is a variable, a field or an argument name.
    let stmts =
        statements("log = 3\ny = log + 1\nz = log.count\nw = f(log)\nv = f(log: 1)\nu = log[0]");
    assert_eq!(stmts.len(), 6);
    assert!(stmts.iter().all(|s| matches!(s, Stmt::Assign { .. })));
    let Stmt::Assign { value, .. } = &stmts[4] else {
        unreachable!()
    };
    let Expr::Call { args, .. } = value else {
        panic!("a call");
    };
    assert_eq!(args[0].name.as_ref().map(|n| n.name.as_str()), Some("log"));
}

#[test]
fn a_log_may_sit_in_an_interpolation_hole() {
    // Spec 2.7 and 5.8: a hole is any expression.
    let expr = parse_expression("log debug x", Span::default()).expect("parses");
    assert!(matches!(expr, Expr::Log { .. }));
}

#[test]
fn a_judgment_block_takes_log_lines_between_its_results() {
    // Spec 5.8 and 6.7: each log line knows how many results precede it.
    let source = "program t\n\njudgment j(message):\n  log debug \"asking\" { message }\n  a = message feels \"is urgent\"\n  log info a\n  b = message feels \"is polite\"\n";
    let program = parse(source).expect("parses");
    let Unit::Judgment(judgment) = &program.units[0] else {
        panic!("a judgment");
    };
    assert_eq!(judgment.results.len(), 2);
    let after: Vec<usize> = judgment.logs.iter().map(|l| l.after).collect();
    assert_eq!(after, vec![0, 1]);
    assert!(
        judgment
            .logs
            .iter()
            .all(|l| matches!(l.log, Expr::Log { .. }))
    );
}

#[test]
fn a_word_that_is_not_a_level_is_log_level() {
    // Spec 5.8 and 12: the levels are debug, info, warn and error.
    let diagnostic = first_error(&in_task("x = 1\nlog trace \"x\""));
    assert_eq!(diagnostic.code, ErrorCode::LogLevel, "{diagnostic}");
    assert_eq!(diagnostic.span.start.line, 5);
    assert!(diagnostic.message.contains("`trace`"), "{diagnostic}");
}

#[test]
fn a_log_field_given_twice_is_duplicate_name() {
    // Spec 5.8 and 12: log field names are unique.
    let diagnostic = first_error(&in_task("log info \"x\" { a, b: 1, a: 2 }"));
    assert_eq!(diagnostic.code, ErrorCode::DuplicateName, "{diagnostic}");
    assert!(diagnostic.message.contains("`a`"), "{diagnostic}");
}

#[test]
fn a_log_takes_one_field_record() {
    // Spec 5.8: at most one record follows the value.
    let diagnostic = first_error(&in_task("log info \"x\" { a } { b }"));
    assert_eq!(diagnostic.code, ErrorCode::Syntax, "{diagnostic}");
}

#[test]
fn a_file_with_a_unit_named_log_keeps_its_command_form_calls() {
    // Spec 5.8: a unit named `log` in the file turns the log form off, so a
    // program written before `log` existed keeps its meaning.
    let source = "program p\nout y\ndef log(info):\n  return info + 1\ntask main:\n  x = 3\n  y = log info x\n  z = \"{f(log info (x))}\"\n";
    let program = parse(source).expect("parses");
    let Unit::Task(task) = &program.units[1] else {
        panic!("a task");
    };
    let Stmt::Assign { value, .. } = &task.body.statements[1] else {
        panic!("an assignment");
    };
    let Expr::Call { callee, args, .. } = value else {
        panic!("a command-form call to the unit, got {value:?}");
    };
    assert!(matches!(&**callee, Expr::Name(n) if n.name == "log"));
    assert_eq!(args[0].name.as_ref().map(|n| n.name.as_str()), Some("info"));
    assert!(program.declares_unit("log"));

    let hole = jevscript_syntax::parse_expression_in("f(log info (x))", Span::default(), true)
        .expect("parses");
    let Expr::Call { args, .. } = hole else {
        panic!("a call");
    };
    assert_eq!(
        args[0].name.as_ref().map(|n| n.name.as_str()),
        Some("log"),
        "in such a file a hole reads `log` as an argument name"
    );
}
