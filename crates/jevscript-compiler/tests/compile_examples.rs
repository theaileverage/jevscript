//! The compiler over every example in `examples/`, which are the programs of
//! spec section 14. Conformance item 1 (section 15) says each compiles without
//! error; the assertions on the linked IR pin the rules of sections 3.9, 6.6,
//! 7.8 and 11.4 that the examples exercise.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use jevscript_compiler::{Resolver, analyze_file, batch, compile_file, tool_verbs_referenced};
use jevscript_ir::{CapabilityKind, Expr, Ir, Stmt};

fn example(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples")
        .join(name)
}

fn compiled(name: &str) -> Ir {
    match compile_file(&example(name), &Resolver::default()) {
        Ok(ir) => ir,
        Err(diagnostics) => panic!("{name} should compile: {diagnostics:#?}"),
    }
}

/// The request groups of every inline judgment in a statement list, in order,
/// recursing into blocks.
fn inline_groups(stmts: &[Stmt]) -> Vec<u32> {
    let mut groups = Vec::new();
    for stmt in stmts {
        match stmt {
            Stmt::Assign {
                value: Expr::Judge(judge),
                ..
            } => groups.push(judge.request_group),
            Stmt::If {
                branches,
                otherwise,
                ..
            } => {
                for branch in branches {
                    groups.extend(inline_groups(&branch.body));
                }
                if let Some(body) = otherwise {
                    groups.extend(inline_groups(body));
                }
            }
            Stmt::For { body, .. } | Stmt::Loop { body, .. } | Stmt::Until { body, .. } => {
                groups.extend(inline_groups(body));
            }
            Stmt::Gate { arms, .. } => {
                for arm in arms {
                    groups.extend(inline_groups(&arm.body));
                }
            }
            _ => {}
        }
    }
    groups
}

#[test]
fn every_example_compiles_with_zero_errors() {
    // Spec section 15, conformance item 1.
    for name in [
        "inbox_triage.jev",
        "fix_issue_inline.jev",
        "fix_issue.jev",
        "chief_of_staff.jev",
        "review_loop.jev",
        "lib/agent_loop.jev",
    ] {
        let compilation = analyze_file(&example(name), &Resolver::default());
        let errors: Vec<_> = compilation
            .diagnostics
            .iter()
            .filter(|d| d.is_error())
            .collect();
        assert!(errors.is_empty(), "{name}: {errors:#?}");
        assert!(compilation.is_ok(), "{name}");
    }
}

#[test]
fn fix_issue_links_its_library_under_its_alias() {
    // Spec 3.9: every unit is qualified `<alias>.<name>` and the flat program
    // only carries the root's `needs`.
    let ir = compiled("fix_issue.jev");
    let judgments: Vec<&str> = ir.judgments.iter().map(|j| j.name.as_str()).collect();
    assert_eq!(judgments, vec!["harness.read_agent"]);
    let tasks: Vec<&str> = ir.tasks.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(tasks, vec!["main", "harness.watch"]);
    let needs: Vec<(&str, CapabilityKind)> =
        ir.needs.iter().map(|n| (n.name.as_str(), n.kind)).collect();
    assert_eq!(
        needs,
        vec![
            ("claude", CapabilityKind::Agent),
            ("tree", CapabilityKind::Tool),
            ("me", CapabilityKind::Person),
        ]
    );
    let aliases: Vec<&str> = ir.modules.iter().map(|m| m.alias.as_str()).collect();
    assert_eq!(aliases, vec!["", "harness", "std"]);
    assert_eq!(ir.modules[1].path.as_deref(), Some("./lib/agent_loop.jev"));
    assert_eq!(ir.modules[1].mapping.len(), 3);
    assert!(
        ir.file
            .as_deref()
            .is_some_and(|f| f.ends_with("fix_issue.jev"))
    );
}

#[test]
fn a_judgment_keeps_its_shape_hash_when_linked_under_an_alias() {
    // Spec 3.9 and 11.4: moving a judgment between files without changing it
    // keeps its hash.
    let linked = compiled("fix_issue.jev");
    let alone = compiled("lib/agent_loop.jev");
    let via_alias = linked.judgment("harness.read_agent").expect("linked");
    let standalone = alone.judgment("read_agent").expect("alone");
    assert_eq!(via_alias.shape_hash, standalone.shape_hash);
    assert_eq!(via_alias.results.len(), 3);
    // Spec 6.6 rule 1: a judgment block is one request.
    assert_eq!(batch::groups_of(&via_alias.results), vec![0, 0, 0]);
}

#[test]
fn a_library_task_calls_units_by_qualified_name_and_the_roots_capabilities() {
    // Spec 3.9: the runtime sees one flat program.
    let ir = compiled("fix_issue.jev");
    let watch = ir.task("harness.watch").expect("linked task");
    let mut callees = BTreeSet::new();
    let mut names = BTreeSet::new();
    jevscript_compiler::walk_expr_in_stmts(&watch.body, &mut |expr| match expr {
        Expr::Call { callee, .. } => {
            if let Expr::Name { name, .. } = &**callee {
                callees.insert(name.clone());
            }
        }
        Expr::Name { name, .. } => {
            names.insert(name.clone());
        }
        _ => {}
    });
    assert!(callees.contains("harness.read_agent"), "{callees:?}");
    assert!(callees.contains("std.stuck"), "{callees:?}");
    assert!(names.contains("tree") && names.contains("me"), "{names:?}");

    // The root's call into the library is qualified too.
    let main = ir.task("main").expect("main");
    let mut main_callees = BTreeSet::new();
    jevscript_compiler::walk_expr_in_stmts(&main.body, &mut |expr| {
        if let Expr::Call { callee, .. } = expr
            && let Expr::Name { name, .. } = &**callee
        {
            main_callees.insert(name.clone());
        }
    });
    assert!(main_callees.contains("harness.watch"), "{main_callees:?}");
}

#[test]
fn harness_watch_batches_as_section_6_6_predicts() {
    // Spec 6.6, worked by hand: `watch` has no inline judgment expression.
    // Its questions live in `read_agent`, which is one request (rule 1), and
    // the body's `dev.wait`, the `shape` with capability calls, the `if` and
    // the `gate` are all separators, so nothing in the body could share a
    // request even if there were questions.
    let ir = compiled("fix_issue.jev");
    let watch = ir.task("harness.watch").expect("linked task");
    assert_eq!(inline_groups(&watch.body), Vec::<u32>::new());
    let expected: Vec<u32> = Vec::new();
    assert_eq!(inline_groups(&watch.body), expected);
    // And no question was left with the lowering placeholder.
    let mut placeholders = 0;
    jevscript_compiler::walk_expr_in_stmts(&watch.body, &mut |expr| {
        if let Expr::Judge(judge) = expr
            && judge.request_group == u32::MAX
        {
            placeholders += 1;
        }
    });
    assert_eq!(placeholders, 0);
}

#[test]
fn chief_of_staff_asks_its_three_questions_in_one_request() {
    // Spec 6.6 rule 2 and section 14.3: `urgent`, `owner` and `risky` are
    // consecutive, independent, and not separated by a call, so one request.
    let ir = compiled("chief_of_staff.jev");
    let main = ir.task("main").expect("main");
    let Stmt::For { body, .. } = &main.body[1] else {
        panic!("the loop is the second statement");
    };
    let groups: Vec<(String, u32)> = body
        .iter()
        .filter_map(|stmt| match stmt {
            Stmt::Assign {
                root,
                value: Expr::Judge(judge),
                ..
            } => Some((root.clone(), judge.request_group)),
            _ => None,
        })
        .collect();
    assert_eq!(
        groups,
        vec![
            ("urgent".to_string(), 0),
            ("owner".to_string(), 0),
            ("risky".to_string(), 0)
        ]
    );
}

#[test]
fn review_loop_lowers_its_machine() {
    // Spec 7.8: `initial` defaults to the first state, `approved` is terminal.
    let ir = compiled("review_loop.jev");
    let machine = ir.machine("review").expect("machine");
    assert_eq!(machine.initial, "working");
    assert_eq!(machine.states.len(), 5);
    let names: Vec<&str> = machine.states.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(
        names,
        vec![
            "working",
            "nudging",
            "waiting_on_me",
            "reviewing",
            "approved"
        ]
    );
    let approved = machine
        .states
        .iter()
        .find(|s| s.name == "approved")
        .unwrap();
    assert!(approved.done && approved.transitions.is_empty());
    let done_states: Vec<&str> = machine
        .states
        .iter()
        .filter(|s| s.done)
        .map(|s| s.name.as_str())
        .collect();
    assert_eq!(done_states, vec!["approved"]);
    assert!(machine.shape_hash.starts_with("sh1:"));
    assert_eq!(machine.budget.calls, Some(30.0));
    assert_eq!(machine.thresholds.min_confidence, Some(0.6));
    // The tool's signature block reaches the IR (9.4).
    let tree = ir.needs.iter().find(|n| n.name == "tree").unwrap();
    let verbs: Vec<&str> = tree.signatures.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(verbs, vec!["tests_pass", "test_summary"]);
}

#[test]
fn every_linked_program_carries_the_prelude() {
    // Spec 3.9 and 7.3: `focus_impl` must be present so the runtime can
    // execute `focus`; `stuck` and `repeats` are reachable as `std.*`.
    let ir = compiled("inbox_triage.jev");
    let defs: Vec<&str> = ir.defs.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(defs, vec!["std.focus_impl", "std.stuck", "std.repeats"]);
    assert_eq!(ir.modules.last().map(|m| m.alias.as_str()), Some("std"));
    // The prelude's own references are qualified: `repeats` inside `stuck`.
    let stuck = ir.defs.iter().find(|d| d.name == "std.stuck").unwrap();
    let mut callees = BTreeSet::new();
    jevscript_compiler::walk_expr_in_stmts(&stuck.body, &mut |expr| {
        if let Expr::Call { callee, .. } = expr
            && let Expr::Name { name, .. } = &**callee
        {
            callees.insert(name.clone());
        }
    });
    assert!(callees.contains("std.repeats"), "{callees:?}");
}

#[test]
fn tool_verbs_are_collected_from_the_linked_program() {
    // Spec 9.4: the verbs the IR references, including property-form calls.
    let ir = compiled("fix_issue.jev");
    let verbs = tool_verbs_referenced(&ir);
    let tree: Vec<&str> = verbs["tree"].iter().map(String::as_str).collect();
    assert_eq!(
        tree,
        vec!["create", "diff", "open_pr", "test_summary", "tests_pass"]
    );
}

#[test]
fn the_ir_round_trips_through_json() {
    // Spec 11.1: the JSON form is the serde serialization.
    let ir = compiled("fix_issue.jev");
    let json = serde_json::to_string(&ir).expect("serializes");
    let back: Ir = serde_json::from_str(&json).expect("deserializes");
    assert_eq!(back, ir);
}
