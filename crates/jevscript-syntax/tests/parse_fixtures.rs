//! The parser over the example programs from spec section 14 and the prelude
//! from sections 7.3 and 8.1.
//!
//! The fixtures are copied verbatim from the spec, so a change here means the
//! spec moved and the parser has to follow. Each test names the rule it pins.

use jevscript_syntax::ast::{
    BudgetKey, CallForm, Decl, Expr, JudgeVerb, Program, Stmt, Unit, Verdict,
};
use jevscript_syntax::parse;
use std::path::{Path, PathBuf};

const FIX_ISSUE: &str = include_str!("../../../examples/fix_issue.jev");
const FIX_ISSUE_INLINE: &str = include_str!("../../../examples/fix_issue_inline.jev");
const AGENT_LOOP: &str = include_str!("../../../examples/lib/agent_loop.jev");
const INBOX_TRIAGE: &str = include_str!("../../../examples/inbox_triage.jev");
const REVIEW_LOOP: &str = include_str!("../../../examples/review_loop.jev");
const CHIEF_OF_STAFF: &str = include_str!("../../../examples/chief_of_staff.jev");

fn parsed(source: &str) -> Program {
    match parse(source) {
        Ok(program) => program,
        Err(diagnostics) => panic!("expected the program to parse, got {diagnostics:?}"),
    }
}

/// `(kind, name)` of every unit, in source order.
fn unit_names(program: &Program) -> Vec<(&'static str, &str)> {
    program
        .units
        .iter()
        .map(|u| match u {
            Unit::Judgment(j) => ("judgment", j.name.name.as_str()),
            Unit::Task(t) => ("task", t.name.name.as_str()),
            Unit::Def(d) => ("def", d.name.name.as_str()),
            Unit::Machine(m) => ("machine", m.name.name.as_str()),
        })
        .collect()
}

fn task<'a>(program: &'a Program, name: &str) -> &'a jevscript_syntax::ast::TaskUnit {
    program
        .units
        .iter()
        .find_map(|u| match u {
            Unit::Task(t) if t.name.name == name => Some(t),
            _ => None,
        })
        .unwrap_or_else(|| panic!("task {name}"))
}

fn judgment<'a>(program: &'a Program, name: &str) -> &'a jevscript_syntax::ast::JudgmentUnit {
    program
        .units
        .iter()
        .find_map(|u| match u {
            Unit::Judgment(j) if j.name.name == name => Some(j),
            _ => None,
        })
        .unwrap_or_else(|| panic!("judgment {name}"))
}

fn verb_name(verb: &JudgeVerb) -> &'static str {
    match verb {
        JudgeVerb::Feels { .. } => "feels",
        JudgeVerb::Pick { .. } => "pick",
        JudgeVerb::Rate { .. } => "rate",
        JudgeVerb::PickAmong { .. } => "pick among",
    }
}

/// Every `.jev` under `examples/`, including `lib/`.
fn example_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("examples directory") {
        let path = entry.expect("entry").path();
        if path.is_dir() {
            example_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "jev") {
            out.push(path);
        }
    }
}

#[test]
fn every_example_parses() {
    // Conformance item 1: every example in section 14 compiles without error.
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    let mut files = Vec::new();
    example_files(&root, &mut files);
    files.sort();
    assert!(files.len() >= 6, "found {files:?}");
    for path in files {
        let source = std::fs::read_to_string(&path).expect("readable");
        if let Err(diagnostics) = parse(&source) {
            panic!("{}: {diagnostics:?}", path.display());
        }
    }
}

#[test]
fn units_come_back_in_source_order_with_their_kinds() {
    // Spec section 3: `unit = judgment | task | def | machine`, in the order written.
    assert_eq!(
        unit_names(&parsed(FIX_ISSUE_INLINE)),
        vec![("judgment", "read_agent"), ("task", "main")]
    );
    assert_eq!(unit_names(&parsed(FIX_ISSUE)), vec![("task", "main")]);
    assert_eq!(
        unit_names(&parsed(AGENT_LOOP)),
        vec![("judgment", "read_agent"), ("task", "watch")]
    );
    assert_eq!(
        unit_names(&parsed(INBOX_TRIAGE)),
        vec![("judgment", "triage")]
    );
    assert_eq!(
        unit_names(&parsed(REVIEW_LOOP)),
        vec![("machine", "review"), ("task", "main")]
    );
    assert_eq!(unit_names(&parsed(CHIEF_OF_STAFF)), vec![("task", "main")]);
}

#[test]
fn the_header_holds_use_in_out_and_needs_in_order() {
    // Spec sections 3.2 to 3.4 and 3.9.
    let program = parsed(FIX_ISSUE);
    assert_eq!(program.name.name, "fix_issue");
    assert_eq!(program.uses.len(), 1);
    let import = &program.uses[0];
    assert_eq!(import.path.as_plain(), Some("./lib/agent_loop.jev"));
    assert_eq!(import.alias.name, "harness");
    let mapped: Vec<&str> = import
        .mapping
        .iter()
        .map(|m| m.inner.name.as_str())
        .collect();
    assert_eq!(mapped, vec!["claude", "tree", "me"]);
    assert!(import.mapping.iter().all(|m| m.outer.is_none()));

    let decls: Vec<&str> = program
        .decls
        .iter()
        .map(|d| match d {
            Decl::In { name, .. } => name.name.as_str(),
            Decl::Out { name, .. } => name.name.as_str(),
            Decl::Needs { name, .. } => name.name.as_str(),
        })
        .collect();
    assert_eq!(decls, vec!["issue", "pr_url", "claude", "tree", "me"]);
}

#[test]
fn a_tool_signature_block_lists_verbs_and_return_types() {
    // Spec section 9.4: `needs tree: tool:` opens a signature block.
    let program = parsed(REVIEW_LOOP);
    let Decl::Needs {
        name, signatures, ..
    } = &program.decls[1]
    else {
        panic!("second declaration is `needs tree`");
    };
    assert_eq!(name.name, "tree");
    let verbs: Vec<(&str, usize)> = signatures
        .iter()
        .map(|s| (s.name.name.as_str(), s.params.len()))
        .collect();
    assert_eq!(verbs, vec![("tests_pass", 0), ("test_summary", 0)]);
}

#[test]
fn main_declares_its_budgets_and_thresholds() {
    // Spec sections 7.1 and 7.6.
    let program = parsed(FIX_ISSUE_INLINE);
    let main = task(&program, "main");
    assert!(main.params.is_empty());
    let budgets: Vec<(BudgetKey, f64)> = main.budgets.iter().map(|b| (b.key, b.value)).collect();
    assert_eq!(
        budgets,
        vec![(BudgetKey::Calls, 40.0), (BudgetKey::Minutes, 30.0)]
    );
    let thresholds: Vec<(&str, f64)> = main
        .thresholds
        .iter()
        .map(|t| (t.name.name.as_str(), t.value))
        .collect();
    assert_eq!(
        thresholds,
        vec![("risk_confirm", 0.2), ("min_confidence", 0.5)]
    );
}

#[test]
fn judgment_blocks_hold_named_results_with_their_verbs() {
    // Spec section 6.7: a judgment body is only `name = judgment` lines.
    let harness = parsed(FIX_ISSUE_INLINE);
    let read_agent = judgment(&harness, "read_agent");
    let params: Vec<&str> = read_agent.params.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(params, vec!["summary", "files", "tests", "recent"]);
    let results: Vec<(&str, &str, bool)> = read_agent
        .results
        .iter()
        .map(|r| {
            (
                r.name.name.as_str(),
                verb_name(&r.question.verb),
                r.question.each,
            )
        })
        .collect();
    assert_eq!(
        results,
        vec![
            ("claims_done", "feels", false),
            ("off_scope", "feels", true),
            ("next", "pick", false),
        ]
    );
    // `claims_done` carries a `focus` detail (spec 6.8).
    let detail = read_agent.results[0]
        .question
        .detail
        .as_ref()
        .expect("detail");
    assert!(detail.focus.is_some());
    assert!(detail.note.is_none());

    let triage = parsed(INBOX_TRIAGE);
    let triage = judgment(&triage, "triage");
    let results: Vec<(&str, &str)> = triage
        .results
        .iter()
        .map(|r| (r.name.name.as_str(), verb_name(&r.question.verb)))
        .collect();
    assert_eq!(
        results,
        vec![
            ("urgent", "feels"),
            ("owner", "pick"),
            ("risky", "feels"),
            ("effort", "rate"),
        ]
    );
    let JudgeVerb::Pick { labels } = &triage.results[1].question.verb else {
        panic!("owner is a pick");
    };
    let names: Vec<(&str, bool)> = labels
        .iter()
        .map(|l| (l.name.name.as_str(), l.escape))
        .collect();
    assert_eq!(
        names,
        vec![
            ("code", false),
            ("customer", false),
            ("schedule", false),
            ("other", true)
        ]
    );
    let JudgeVerb::Rate { levels } = &triage.results[3].question.verb else {
        panic!("effort is a rate");
    };
    let names: Vec<Option<&str>> = levels
        .iter()
        .map(|l| l.name.as_ref().map(|n| n.name.as_str()))
        .collect();
    assert_eq!(names, vec![Some("trivial"), Some("hour"), Some("project")]);
}

#[test]
fn the_review_machine_keeps_its_states_events_guards_and_actions() {
    // Spec section 7.8 and example 14.4.
    let program = parsed(REVIEW_LOOP);
    let Unit::Machine(machine) = &program.units[0] else {
        panic!("first unit is the machine");
    };
    assert_eq!(machine.name.name, "review");
    assert!(machine.goal.is_some());
    assert!(machine.initial.is_none());
    let observed: Vec<(&str, Option<f64>)> = machine
        .observe
        .iter()
        .map(|f| (f.name.name.as_str(), f.max))
        .collect();
    assert_eq!(
        observed,
        vec![("summary", None), ("tests", Some(300.0)), ("status", None)]
    );

    let states: Vec<(&str, bool)> = machine
        .states
        .iter()
        .map(|s| (s.name.name.as_str(), s.done))
        .collect();
    assert_eq!(
        states,
        vec![
            ("working", false),
            ("nudging", false),
            ("waiting_on_me", false),
            ("reviewing", false),
            ("approved", true),
        ]
    );

    // (event, target, has `when`, risky, has actions) per transition.
    let transitions: Vec<(&str, &str, bool, bool, bool)> = machine
        .states
        .iter()
        .flat_map(|s| s.transitions.iter())
        .map(|t| {
            (
                t.event.name.as_str(),
                t.target.name.as_str(),
                t.when.is_some(),
                t.risky,
                t.body.is_some(),
            )
        })
        .collect();
    assert_eq!(
        transitions,
        vec![
            ("finished", "reviewing", true, false, false),
            ("claims_done", "nudging", true, false, true),
            ("stuck", "nudging", false, false, true),
            ("asks", "waiting_on_me", false, false, false),
            ("resumed", "working", false, false, false),
            ("answered", "working", false, false, true),
            ("approved", "approved", true, false, false),
            ("rejected", "working", false, true, true),
        ]
    );
    assert!(
        machine.states[4].transitions.is_empty(),
        "a done state has no events"
    );
}

#[test]
fn a_shape_names_its_fields_and_their_caps() {
    // Spec section 7.2 and 7.3: `focus ... on ..., max 2k` keeps its own cap.
    let program = parsed(AGENT_LOOP);
    let watch = task(&program, "watch");
    let Stmt::Until {
        verify, max, body, ..
    } = &watch.body.statements[0]
    else {
        panic!("watch starts with `until verify(...)`");
    };
    assert!(*verify);
    assert_eq!(*max, 6.0);
    let Stmt::Shape(shape) = &body.statements[1] else {
        panic!("second statement is `obs = shape:`");
    };
    assert_eq!(shape.target.name, "obs");
    assert!(!shape.strict);
    let fields: Vec<(&str, Option<f64>)> = shape
        .fields
        .iter()
        .map(|f| (f.name.name.as_str(), f.max))
        .collect();
    assert_eq!(
        fields,
        vec![
            ("summary", None),
            ("files", Some(500.0)),
            ("tests", Some(300.0)),
            ("recent", None),
        ]
    );
    let Expr::Focus { max, on, .. } = &shape.fields[0].value else {
        panic!("summary is a focus");
    };
    assert_eq!(*max, 2000.0, "`2k` is 2000 (spec 2.6)");
    let Expr::Text { value, .. } = &**on else {
        panic!("the purpose is a text");
    };
    assert_eq!(value.as_plain(), Some("what the agent says it did"));
    assert!(matches!(shape.fields[3].value, Expr::Trail { count, .. } if count == 6.0));
    // A bare path stays the path written; the compiler decides it is a call.
    assert!(matches!(shape.fields[1].value, Expr::Field { .. }));
}

#[test]
fn a_gate_lists_its_arms_and_a_same_line_arm_is_one_statement() {
    // Spec section 7.6.
    let program = parsed(FIX_ISSUE_INLINE);
    let main = task(&program, "main");
    let Stmt::Until { body, .. } = &main.body.statements[2] else {
        panic!("third statement is the until loop");
    };
    let gate = body
        .statements
        .iter()
        .find_map(|s| match s {
            Stmt::Gate(g) => Some(g),
            _ => None,
        })
        .expect("a gate in the loop");
    assert!(gate.risk.is_some());
    assert!(gate.confidence.is_some());
    assert!(gate.done.is_none());
    let arms: Vec<(Verdict, usize)> = gate
        .arms
        .iter()
        .map(|a| (a.verdict, a.body.statements.len()))
        .collect();
    assert_eq!(
        arms,
        vec![
            (Verdict::Confirm, 1),
            (Verdict::Escalate, 1),
            (Verdict::Proceed, 1)
        ]
    );
    assert!(matches!(gate.arms[2].body.statements[0], Stmt::If { .. }));
}

#[test]
fn command_form_names_its_arguments_even_with_reserved_words() {
    // Spec section 5.2: `dev = claude.spawn in tree, prompt issue.body`.
    let program = parsed(FIX_ISSUE);
    let main = task(&program, "main");
    let Stmt::Assign { target, value, .. } = &main.body.statements[1] else {
        panic!("second statement assigns dev");
    };
    assert_eq!(target.root.name, "dev");
    let Expr::Call { args, form, .. } = value else {
        panic!("the value is a call");
    };
    assert_eq!(*form, CallForm::Command);
    let names: Vec<Option<&str>> = args
        .iter()
        .map(|a| a.name.as_ref().map(|n| n.name.as_str()))
        .collect();
    assert_eq!(names, vec![Some("in"), Some("prompt")]);

    // `passed = harness.watch(dev, issue.title)`: function form, positional.
    let Stmt::Assign { value, .. } = &main.body.statements[2] else {
        panic!("third statement assigns passed");
    };
    let Expr::Call { args, form, .. } = value else {
        panic!("the value is a call");
    };
    assert_eq!(*form, CallForm::Function);
    assert!(args.iter().all(|a| a.name.is_none()));

    // `dev.stop` is a bare path; `tree.create issue.branch` is a command.
    assert!(matches!(
        &main.body.statements[3],
        Stmt::Expr {
            expr: Expr::Field { .. },
            ..
        }
    ));
    assert!(matches!(
        &main.body.statements[0],
        Stmt::Expr {
            expr: Expr::Call {
                form: CallForm::Command,
                ..
            },
            ..
        }
    ));
}

#[test]
fn the_dispatcher_reads_using_and_keyword_field_names() {
    // Spec section 9.3: `writer.write "..." using e.body` needs no comma before
    // `using`; section 14.3 also calls `mux.enqueue "calendar", e`.
    let program = parsed(CHIEF_OF_STAFF);
    let main = task(&program, "main");
    let Stmt::For { body, .. } = &main.body.statements[1] else {
        panic!("second statement is the for loop");
    };
    let Stmt::Gate(gate) = &body.statements[5] else {
        panic!("sixth statement in the loop is the gate, after the `log` line");
    };
    let Stmt::If { branches, .. } = &gate.arms[0].body.statements[0] else {
        panic!("proceed arm is an if");
    };
    let Stmt::Assign { value, .. } = &branches[1].body.statements[0] else {
        panic!("elif customer assigns draft");
    };
    let Expr::Call { args, .. } = value else {
        panic!("draft is a call");
    };
    let names: Vec<Option<&str>> = args
        .iter()
        .map(|a| a.name.as_ref().map(|n| n.name.as_str()))
        .collect();
    assert_eq!(names, vec![None, Some("using")]);
}

#[test]
fn the_prelude_parses_as_a_library() {
    // Spec sections 7.3 and 8.1: `focus_impl`, `stuck` and `repeats`.
    let source = "program std\n\n".to_string()
        + "def focus_impl(text, purpose, limit):\n"
        + "  if tokens(text) <= limit:\n"
        + "    return text\n"
        + "  chunks = chunk(text, tokens 1500)\n"
        + "  keep   = each chunks feels \"contains information relevant to: {purpose}\"\n"
        + "  kept   = [c for c, p in zip(chunks, keep) if p > 0.6]\n"
        + "  if len(kept) == 0:\n"
        + "    kept = top(chunks, by keep, n 3)\n"
        + "  return focus_impl(join(kept), purpose, limit)\n"
        + "\n"
        + "def stuck(steps):\n"
        + "  if repeats(steps) >= 2:\n"
        + "    return true\n"
        + "  same = steps feels \"the recent attempts use the same approach with only cosmetic differences\"\n"
        + "  progress = steps rate:\n"
        + "    unchanged \"each step leaves the state unchanged\"\n"
        + "    drifting  \"the state changes but not toward the goal\"\n"
        + "    advancing \"the state is moving toward the goal\"\n"
        + "  return same > 0.7 or (progress is unchanged and progress.confidence > 0.6)\n"
        + "\n"
        + "def repeats(steps):\n"
        + "  keys = [hash([s.action, s.target, s.args]) for s in steps]\n"
        + "  most = 0\n"
        + "  for k in keys:\n"
        + "    n = len([x for x in keys if x == k])\n"
        + "    most = max([most, n])\n"
        + "  return most\n";
    let program = parsed(&source);
    assert_eq!(
        unit_names(&program),
        vec![("def", "focus_impl"), ("def", "stuck"), ("def", "repeats")]
    );
    let Unit::Def(focus_impl) = &program.units[0] else {
        panic!()
    };
    // `chunk(text, tokens 1500)` and `top(chunks, by keep, n 3)` use the
    // `NAME expr` named-argument form (spec 13).
    let Stmt::Assign { value, .. } = &focus_impl.body.statements[1] else {
        panic!("chunks = chunk(...)");
    };
    let Expr::Call { args, .. } = value else {
        panic!()
    };
    assert_eq!(
        args[1].name.as_ref().map(|n| n.name.as_str()),
        Some("tokens")
    );
    // `max([most, n])`: `max` is reserved but names the builtin before `(`.
    let Unit::Def(repeats) = &program.units[2] else {
        panic!()
    };
    let Stmt::For { body, .. } = &repeats.body.statements[2] else {
        panic!()
    };
    let Stmt::Assign { value, .. } = &body.statements[1] else {
        panic!()
    };
    assert!(
        matches!(value, Expr::Call { callee, .. } if matches!(&**callee, Expr::Name(n) if n.name == "max"))
    );
}

#[test]
fn text_holes_are_checked_and_kept_as_source() {
    // Spec section 2.7: `{expr}` is parsed while the literal is, and stays raw
    // in the AST for the compiler to lower with `parse_expression`.
    let program = parsed(CHIEF_OF_STAFF);
    let main = task(&program, "main");
    let Stmt::For { body, .. } = &main.body.statements[1] else {
        panic!()
    };
    let Stmt::If { branches, .. } = body.statements.last().expect("if urgent") else {
        panic!()
    };
    let Stmt::Expr { expr, .. } = &branches[0].body.statements[0] else {
        panic!()
    };
    let Expr::Call { args, .. } = expr else {
        panic!()
    };
    let Expr::Text { value, .. } = &args[0].value else {
        panic!()
    };
    assert_eq!(value.parts.len(), 2);
    let expr = jevscript_syntax::parse_expression("e.subject", value.parts[1].span_of_hole())
        .expect("the hole parses");
    assert!(matches!(expr, Expr::Field { .. }));
}

trait HoleSpan {
    fn span_of_hole(&self) -> jevscript_syntax::Span;
}

impl HoleSpan for jevscript_syntax::TextPart {
    fn span_of_hole(&self) -> jevscript_syntax::Span {
        match self {
            jevscript_syntax::TextPart::Interpolation { span, .. } => *span,
            jevscript_syntax::TextPart::Literal(_) => panic!("not a hole"),
        }
    }
}

#[test]
fn a_shape_field_takes_tail_after_its_cap() {
    // Spec 7.2 and 13: `<field> <expr>, max N, tail` keeps the last tokens on
    // overflow; `tail` is a contextual word and stays a name elsewhere.
    let source = "program t\n\ntask main:\n  t = \"x\"\n  tail = 1\n  obs = shape:\n    last  t, max 300, tail\n    first t, max 300\n    both  t + tail\n";
    let program = parsed(source);
    let Unit::Task(main) = &program.units[0] else {
        panic!("expected a task");
    };
    let Stmt::Shape(shape) = &main.body.statements[2] else {
        panic!("expected a shape");
    };
    assert_eq!(shape.fields[0].max, Some(300.0));
    assert!(shape.fields[0].tail);
    assert!(!shape.fields[1].tail);
    assert!(!shape.fields[2].tail);
    assert!(matches!(shape.fields[2].value, Expr::Binary { .. }));
}
