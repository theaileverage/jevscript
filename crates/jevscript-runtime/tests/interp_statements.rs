//! The statement interpreter over hand-built IR (spec sections 5, 6 and 7).
//!
//! Each test names the rule it checks.

mod support;

use std::collections::BTreeMap;

use jevscript_ir::{BinaryOp, BudgetKey, CapabilityKind, ReturnType, ShapePolicy, Verdict};
use jevscript_runtime::record::Event;
use jevscript_runtime::run::RunOptions;
use jevscript_runtime::{Pause, PauseKind, Resume, RuntimeErrorCode, Value};
use support::*;

fn done_outputs(pause: &Pause) -> BTreeMap<String, Value> {
    match pause {
        Pause::Done { outputs, .. } => outputs.clone(),
        other => panic!("expected done, got {other:?}"),
    }
}

fn run_main(outputs: &[&str], body: Vec<jevscript_ir::Stmt>) -> Pause {
    let ir = program(Vec::new(), outputs, body);
    let mut run = start(
        ir,
        RunOptions::default(),
        bindings(Vec::new()),
        Answers::default(),
    );
    run.next().expect("steps")
}

#[test]
fn a_block_assignment_updates_an_outer_variable_and_a_new_one_stays_inside() {
    // Spec 5.3: variables are block-scoped; assignment to an outer variable
    // from an inner block updates it; a first assignment lands inside, so
    // reading it after the block is reading an unknown name.
    let pause = run_main(
        &["outer", "result"],
        vec![
            assign("outer", num(1.0)),
            if_else(
                boolean(true),
                vec![
                    assign("outer", num(2.0)),
                    assign("inner", num(3.0)),
                    assign("result", num(9.0)),
                ],
                None,
            ),
        ],
    );
    let outputs = done_outputs(&pause);
    assert_eq!(outputs["outer"], Value::Number(2.0));
    assert_eq!(
        outputs["result"],
        Value::Number(9.0),
        "a declared `out` is a variable of the program (spec 3.3), so a block assigns it"
    );

    let pause = run_main(
        &["leaked"],
        vec![
            if_else(boolean(true), vec![assign("inner", num(3.0))], None),
            on_line(assign("leaked", name("inner")), 4),
        ],
    );
    let Pause::Error {
        code,
        message,
        common,
        ..
    } = &pause
    else {
        panic!("error, got {pause:?}");
    };
    assert_eq!(*code, RuntimeErrorCode::TypeError);
    assert!(message.contains("`inner`"), "{message}");
    assert_eq!(common.source, at(4));
}

#[test]
fn a_field_assignment_updates_a_record_held_in_a_variable() {
    // Spec 5.3: `x.field = v` on records held in variables.
    let pause = run_main(
        &["r"],
        vec![
            assign("r", record(vec![("a", num(1.0))])),
            assign_field("r", &["a"], num(5.0)),
            assign_field("r", &["b"], text("new")),
        ],
    );
    let Value::Record(fields) = &done_outputs(&pause)["r"] else {
        panic!("a record");
    };
    assert_eq!(fields["a"], Value::Number(5.0));
    assert_eq!(fields["b"], Value::Text("new".into()));
}

#[test]
fn for_destructures_pairs_and_honours_continue_and_break() {
    // Spec 5.5: `for x, y in list` destructures two-element lists;
    // `continue` and `break` behave as in Python.
    let pause = run_main(
        &["sum", "seen"],
        vec![
            assign("sum", num(0.0)),
            assign("seen", num(0.0)),
            for_in(
                &["k", "v"],
                call(
                    name("zip"),
                    vec![
                        list(vec![text("a"), text("b"), text("c"), text("d")]),
                        list(vec![num(1.0), num(2.0), num(3.0), num(4.0)]),
                    ],
                ),
                vec![
                    assign("seen", binary(BinaryOp::Add, name("seen"), num(1.0))),
                    if_else(
                        binary(BinaryOp::Eq, name("k"), text("b")),
                        vec![cont()],
                        None,
                    ),
                    if_else(
                        binary(BinaryOp::Eq, name("k"), text("d")),
                        vec![brk()],
                        None,
                    ),
                    assign("sum", binary(BinaryOp::Add, name("sum"), name("v"))),
                ],
            ),
        ],
    );
    let outputs = done_outputs(&pause);
    assert_eq!(outputs["sum"], Value::Number(4.0), "a and c");
    assert_eq!(
        outputs["seen"],
        Value::Number(4.0),
        "d was reached, then broke"
    );
}

#[test]
fn until_tests_before_each_iteration_and_once_after_the_last() {
    // Spec 5.5: `until cond, max N` evaluates `cond` before each iteration
    // and once more after the last; if still false the loop ends anyway.
    let pause = run_main(
        &["tests", "runs"],
        vec![
            assign("tests", num(0.0)),
            assign("runs", num(0.0)),
            assign("probe", binary(BinaryOp::Eq, name("tests"), num(-1.0))),
            until(
                // Each evaluation of the condition counts itself through a
                // def would need a side effect; instead count via a shape-free
                // trick: the condition reads `runs` and the body counts.
                binary(BinaryOp::GtEq, name("runs"), num(10.0)),
                3.0,
                vec![assign(
                    "runs",
                    binary(BinaryOp::Add, name("runs"), num(1.0)),
                )],
            ),
        ],
    );
    let outputs = done_outputs(&pause);
    assert_eq!(
        outputs["runs"],
        Value::Number(3.0),
        "the body ran max times"
    );

    // A condition that becomes true after the last iteration is seen by the
    // extra test: `verified` proves it (spec 7.4).
    let ir = program(
        Vec::new(),
        &["runs"],
        vec![
            assign("runs", num(0.0)),
            until_verify(
                binary(BinaryOp::GtEq, name("runs"), num(2.0)),
                2.0,
                vec![assign(
                    "runs",
                    binary(BinaryOp::Add, name("runs"), num(1.0)),
                )],
            ),
        ],
    );
    let mut run = start(
        ir,
        RunOptions::default(),
        bindings(Vec::new()),
        Answers::default(),
    );
    let Pause::Done { verified, .. } = run.next().expect("steps") else {
        panic!("done");
    };
    assert!(verified, "true at the final test after max iterations");
}

#[test]
fn verify_sets_verified_only_when_the_condition_became_true() {
    // Spec 7.4: `done` carries `verified: true` only if the verify
    // condition was true at the end.
    let ir = program(
        Vec::new(),
        &[],
        vec![
            assign("n", num(0.0)),
            until_verify(
                binary(BinaryOp::GtEq, name("n"), num(5.0)),
                2.0,
                vec![assign("n", binary(BinaryOp::Add, name("n"), num(1.0)))],
            ),
        ],
    );
    let mut run = start(
        ir,
        RunOptions::default(),
        bindings(Vec::new()),
        Answers::default(),
    );
    let Pause::Done { verified, .. } = run.next().expect("steps") else {
        panic!("done");
    };
    assert!(!verified, "ran out of loop without proof");

    let ir = program(
        Vec::new(),
        &[],
        vec![
            assign("n", num(0.0)),
            until_verify(
                binary(BinaryOp::GtEq, name("n"), num(1.0)),
                5.0,
                vec![assign("n", binary(BinaryOp::Add, name("n"), num(1.0)))],
            ),
        ],
    );
    let mut run = start(
        ir,
        RunOptions::default(),
        bindings(Vec::new()),
        Answers::default(),
    );
    let Pause::Done { verified, .. } = run.next().expect("steps") else {
        panic!("done");
    };
    assert!(verified);

    // Spec 7.4: a `return` before the verify loop leaves the task
    // unverified, and the skipped condition is not read even though it
    // names a local the loop would have assigned.
    let ir = program(
        Vec::new(),
        &["after"],
        vec![
            assign("after", boolean(false)),
            if_else(boolean(true), vec![ret(None)], None),
            assign("ok", boolean(true)),
            until_verify(name("ok"), 1.0, vec![brk()]),
            assign("after", boolean(true)),
        ],
    );
    let mut run = start(
        ir,
        RunOptions::default(),
        bindings(Vec::new()),
        Answers::default(),
    );
    let pause = run.next().expect("steps");
    let Pause::Done {
        verified, outputs, ..
    } = &pause
    else {
        panic!("done, got {pause:?}");
    };
    assert!(!verified, "returned before the verify loop");
    assert_eq!(outputs["after"], Value::Bool(false));

    // A `break` leaves the loop without the test, so nothing was proven.
    let ir = program(
        Vec::new(),
        &[],
        vec![until_verify(boolean(false), 5.0, vec![brk()])],
    );
    let mut run = start(
        ir,
        RunOptions::default(),
        bindings(Vec::new()),
        Answers::default(),
    );
    let Pause::Done { verified, .. } = run.next().expect("steps") else {
        panic!("done");
    };
    assert!(!verified);

    // Spec 7.4: `verified` is what the condition says at the end of the
    // task, not what it said when the loop ended. True then, false now:
    // not verified.
    let ir = program(
        Vec::new(),
        &[],
        vec![
            assign("ok", boolean(true)),
            until_verify(name("ok"), 1.0, vec![]),
            assign("ok", boolean(false)),
        ],
    );
    let mut run = start(
        ir,
        RunOptions::default(),
        bindings(Vec::new()),
        Answers::default(),
    );
    let Pause::Done { verified, .. } = run.next().expect("steps") else {
        panic!("done");
    };
    assert!(!verified, "the condition was false at the end");

    // And the loop running out with the condition false, then code making
    // it true before the end: verified, because it is true at the end.
    let ir = program(
        Vec::new(),
        &["runs"],
        vec![
            assign("ok", boolean(false)),
            assign("runs", num(0.0)),
            until_verify(
                name("ok"),
                2.0,
                vec![assign(
                    "runs",
                    binary(BinaryOp::Add, name("runs"), num(1.0)),
                )],
            ),
            assign("ok", boolean(true)),
        ],
    );
    let mut run = start(
        ir,
        RunOptions::default(),
        bindings(Vec::new()),
        Answers::default(),
    );
    let Pause::Done {
        verified, outputs, ..
    } = run.next().expect("steps")
    else {
        panic!("done");
    };
    assert_eq!(outputs["runs"], Value::Number(2.0), "the loop ran out");
    assert!(verified, "the condition was true at the end");
}

#[test]
fn return_stop_and_escalate_end_or_pause_the_run() {
    // Spec 5.6.
    let pause = run_main(
        &["x"],
        vec![assign("x", num(1.0)), ret(None), assign("x", num(2.0))],
    );
    assert_eq!(done_outputs(&pause)["x"], Value::Number(1.0));

    let pause = run_main(&[], vec![on_line(stop("enough"), 7)]);
    let Pause::Stopped { common, reason } = &pause else {
        panic!("stopped, got {pause:?}");
    };
    assert_eq!(reason, "enough");
    assert_eq!(common.source, at(7));
    assert_eq!(common.task, "main");

    let ir = program(
        Vec::new(),
        &["after"],
        vec![
            assign("after", boolean(false)),
            on_line(escalate("unsure"), 3),
            assign("after", boolean(true)),
        ],
    );
    let mut run = start(
        ir,
        RunOptions::default(),
        bindings(Vec::new()),
        Answers::default(),
    );
    let pause = run.next().expect("steps");
    let Pause::Escalate { reason, common, .. } = &pause else {
        panic!("escalate, got {pause:?}");
    };
    assert_eq!(reason, "unsure");
    assert_eq!(common.source, at(3));
    assert_eq!(run.state().paused_on(), Some(PauseKind::Escalate));
    // Spec 10.2: `{ resume: true }` continues past an escalate.
    run.resume(Resume::Continue { resume: true })
        .expect("resumes");
    let pause = run.next().expect("continues");
    assert_eq!(done_outputs(&pause)["after"], Value::Bool(true));
}

/* -------------------------------------------------------------------------- */
/* Judgments                                                                   */
/* -------------------------------------------------------------------------- */

#[test]
fn a_request_group_is_one_request_whose_state_is_exactly_the_subjects() {
    // Spec 6.6 rule 2 and 6.9: consecutive independent judgments form one
    // request, and the state carries only the variables the subjects name.
    let dir = temp_dir("group");
    let ir = program(
        Vec::new(),
        &["a", "b", "c"],
        vec![
            assign("unrelated", text("never sent")),
            assign(
                "obs",
                record(vec![("summary", text("all done")), ("tests", text("pass"))]),
            ),
            assign("a", feels("obs.summary", "claims completion")),
            assign(
                "b",
                pick(
                    "obs.tests",
                    &[("passing", "tests pass"), ("failing", "tests fail")],
                ),
            ),
            assign("c", feels("a", "depends on the answer to a")),
        ],
    );
    let answers = Answers::default()
        .prob("a", 0.9)
        .label("b", "passing", 0.8)
        .prob("c", 0.2);
    let mut run = start(
        ir,
        tiny_profile_options(&dir),
        bindings(Vec::new()),
        answers,
    );
    let pause = run.next().expect("steps");
    let outputs = done_outputs(&pause);
    assert_eq!(outputs["a"], Value::Prob(0.9));
    assert!(matches!(&outputs["b"], Value::Choice(c) if c.label == "passing"));
    assert_eq!(outputs["c"], Value::Prob(0.2));

    let events = run.drain_events();
    let events: Vec<Event> = events.into_iter().map(|e| e.event).collect();
    let requests = events_of(&events, "request");
    assert_eq!(requests.len(), 2, "a and b share a request; c depends on a");
    let Event::Request {
        state, questions, ..
    } = requests[0]
    else {
        panic!("request");
    };
    let state = state.full().expect("full");
    assert_eq!(
        state.keys().collect::<Vec<_>>(),
        vec!["obs"],
        "only the subjects' roots"
    );
    assert_eq!(
        state["obs"],
        serde_json::json!({"summary": "all done", "tests": "pass"}),
        "only the fields the paths reach"
    );
    assert_eq!(questions.len(), 2);
    assert_eq!(questions[0].path(), "obs.summary");
    assert_eq!(questions[1].path(), "obs.tests");
    let Event::Request { state, .. } = requests[1] else {
        panic!("request");
    };
    assert_eq!(
        state.full().expect("full").keys().collect::<Vec<_>>(),
        vec!["a"]
    );
    assert_eq!(
        run.usage().calls,
        2,
        "one call per request, not per question"
    );
}

#[test]
fn each_splits_at_the_profile_cap_and_comes_back_as_one_list() {
    // Spec 6.5: `each` over a list longer than the per-request cap is split
    // across requests and returns one list in element order.
    let dir = temp_dir("each");
    let ir = program(
        Vec::new(),
        &["ps", "n"],
        vec![
            assign(
                "files",
                list(vec![text("a"), text("b"), text("c"), text("d"), text("e")]),
            ),
            assign("ps", each_feels("files", "is unrelated")),
            assign(
                "n",
                call_named(name("count"), vec![name("ps")], vec![("above", num(0.5))]),
            ),
        ],
    );
    let answers = Answers::default().each("ps", &[0.1, 0.9, 0.2, 0.8, 0.7]);
    let mut run = start(
        ir,
        tiny_profile_options(&dir),
        bindings(Vec::new()),
        answers,
    );
    let pause = run.next().expect("steps");
    let outputs = done_outputs(&pause);
    assert_eq!(
        outputs["ps"],
        Value::List(vec![
            Value::Prob(0.1),
            Value::Prob(0.9),
            Value::Prob(0.2),
            Value::Prob(0.8),
            Value::Prob(0.7)
        ])
    );
    assert_eq!(outputs["n"], Value::Number(3.0));
    let events: Vec<Event> = run.drain_events().into_iter().map(|e| e.event).collect();
    let requests = events_of(&events, "request");
    // Spec 6.6: only an `each` over the cap on its own may be chunked, into
    // consecutive chunks in question order.
    assert_eq!(requests.len(), 2, "five questions at a cap of four");
    let Event::Request {
        state, questions, ..
    } = requests[1]
    else {
        panic!("request");
    };
    assert_eq!(questions.len(), 1);
    assert_eq!(questions[0].path(), "files[4]");
    // Spec 6.9: unjudged positions are null so the path still addresses it.
    assert_eq!(
        state.full().expect("full")["files"],
        serde_json::json!([null, null, null, null, "e"])
    );
    let Event::Request { questions, .. } = requests[0] else {
        panic!("request");
    };
    assert_eq!(questions.len(), 4);
}

#[test]
fn pick_among_chooses_an_element_and_a_too_long_list_is_an_error() {
    // Spec 6.4a.
    let dir = temp_dir("among");
    let ir = program(
        Vec::new(),
        &["c"],
        vec![
            assign(
                "prs",
                list(vec![
                    record(vec![("title", text("fix lexer"))]),
                    record(vec![("title", text("add docs"))]),
                ]),
            ),
            assign(
                "c",
                pick_among("prs", "which should merge first", Some("title"), true),
            ),
        ],
    );
    let answers = Answers::default().label("c", "i1", 0.7);
    let mut run = start(
        ir,
        tiny_profile_options(&dir),
        bindings(Vec::new()),
        answers,
    );
    let pause = run.next().expect("steps");
    let Value::Choice(choice) = &done_outputs(&pause)["c"] else {
        panic!("a choice");
    };
    assert_eq!(choice.label, "i1");
    assert_eq!(choice.index, Some(1));
    assert_eq!(
        choice.item.as_deref(),
        Some(&Value::Record(BTreeMap::from([(
            "title".to_string(),
            Value::Text("add docs".into())
        )])))
    );

    let ir = program(
        Vec::new(),
        &["c"],
        vec![
            assign(
                "xs",
                list(vec![num(1.0), num(2.0), num(3.0), num(4.0), num(5.0)]),
            ),
            on_line(assign("c", pick_among("xs", "which", None, false)), 9),
        ],
    );
    let mut run = start(
        ir,
        tiny_profile_options(&dir),
        bindings(Vec::new()),
        Answers::default(),
    );
    let pause = run.next().expect("steps");
    let Pause::Error {
        code,
        common,
        retryable,
        ..
    } = &pause
    else {
        panic!("error, got {pause:?}");
    };
    assert_eq!(*code, RuntimeErrorCode::PickTooMany);
    assert!(!retryable);
    assert_eq!(common.source, at(9));
}

#[test]
fn a_named_judgment_is_one_request_over_exactly_its_parameters() {
    // Spec 6.7 and 6.9: calling a judgment sends one request whose state is
    // the parameters, from a record argument or from named arguments.
    let dir = temp_dir("named");
    let mut ir = program(
        Vec::new(),
        &["j", "k"],
        vec![
            assign(
                "obs",
                record(vec![
                    ("summary", text("done")),
                    ("tests", text("pass")),
                    ("notes", text("read by no question")),
                    ("extra", text("not a parameter")),
                ]),
            ),
            assign("j", call_unit("read_agent", vec![name("obs")])),
            assign(
                "k",
                call_named(
                    name("read_agent"),
                    vec![],
                    vec![
                        ("summary", text("s")),
                        ("tests", text("t")),
                        ("notes", text("n")),
                    ],
                ),
            ),
        ],
    );
    ir.judgments.push(judgment(
        "read_agent",
        &["summary", "tests", "notes"],
        vec![
            ("claims_done", feels("summary", "says it is done")),
            ("passing", feels("tests", "tests pass")),
        ],
    ));
    batch(&mut ir);
    let answers = Answers::default()
        .prob("claims_done", 0.8)
        .prob("passing", 0.3);
    let mut run = start(
        ir,
        tiny_profile_options(&dir),
        bindings(Vec::new()),
        answers,
    );
    let pause = run.next().expect("steps");
    let outputs = done_outputs(&pause);
    let Value::Record(j) = &outputs["j"] else {
        panic!("a record");
    };
    assert_eq!(j["claims_done"], Value::Prob(0.8));
    assert_eq!(j["passing"], Value::Prob(0.3));
    assert!(matches!(&outputs["k"], Value::Record(k) if k.len() == 2));
    let events: Vec<Event> = run.drain_events().into_iter().map(|e| e.event).collect();
    let requests = events_of(&events, "request");
    assert_eq!(requests.len(), 2);
    let Event::Request {
        state, questions, ..
    } = requests[0]
    else {
        panic!("request");
    };
    let state = state.full().expect("full");
    assert_eq!(
        state.keys().collect::<Vec<_>>(),
        vec!["notes", "summary", "tests"],
        "every declared parameter, `notes` included, never `extra`"
    );
    assert_eq!(state["notes"], serde_json::json!("read by no question"));
    assert_eq!(questions.len(), 2);
    let Event::Request { state, .. } = requests[1] else {
        panic!("request");
    };
    assert_eq!(state.full().expect("full")["notes"], serde_json::json!("n"));
}

#[test]
fn an_inline_group_over_the_question_cap_fails_before_any_request() {
    // Spec 6.6: the per-request cap never splits a group on its own; a group
    // that expands past it with no `each` over the cap fails with
    // `state_too_large` before anything is sent.
    let dir = temp_dir("group-cap");
    let mut body = vec![assign("x", text("s"))];
    for i in 0..5 {
        body.push(assign(&format!("p{i}"), feels("x", &format!("q{i}"))));
    }
    body[1] = on_line(body[1].clone(), 2);
    let ir = program(Vec::new(), &[], body);
    let mut run = jevscript_runtime::Run::create(
        ir,
        "main",
        tiny_profile_options(&dir),
        bindings(Vec::new()),
    )
    .expect("starts")
    .with_client(Box::new(NeverJev));
    let pause = run.next().expect("steps");
    let Pause::Error {
        code,
        message,
        common,
        ..
    } = &pause
    else {
        panic!("error, got {pause:?}");
    };
    assert_eq!(*code, RuntimeErrorCode::StateTooLarge);
    assert!(
        message.contains("p0") && message.contains("5 questions"),
        "{message}"
    );
    assert_eq!(common.source, at(2));
    assert_eq!(run.usage().calls, 0);
}

#[test]
fn an_each_over_the_cap_chunks_a_named_judgment_and_every_chunk_carries_all_parameters() {
    // Spec 6.5, 6.6 and 6.9: an `each` over the cap is the one thing that
    // splits a named judgment, into consecutive chunks in question order, and
    // each chunk carries every parameter in full.
    let dir = temp_dir("named-each");
    let mut ir = program(
        Vec::new(),
        &["j"],
        vec![
            assign(
                "items",
                list(vec![text("a"), text("b"), text("c"), text("d"), text("e")]),
            ),
            assign("extra", text("context")),
            assign(
                "j",
                call_named(
                    name("triage"),
                    vec![],
                    vec![("items", name("items")), ("extra", name("extra"))],
                ),
            ),
        ],
    );
    ir.judgments.push(judgment(
        "triage",
        &["items", "extra"],
        vec![
            ("ps", each_feels("items", "is relevant")),
            ("q", feels("extra", "is context")),
        ],
    ));
    batch(&mut ir);
    let answers = Answers::default()
        .each("ps", &[0.1, 0.2, 0.3, 0.4, 0.5])
        .prob("q", 0.9);
    let mut run = start(
        ir,
        tiny_profile_options(&dir),
        bindings(Vec::new()),
        answers,
    );
    let outputs = done_outputs(&run.next().expect("steps"));
    let Value::Record(j) = &outputs["j"] else {
        panic!("a record");
    };
    assert_eq!(
        j["ps"],
        Value::List(vec![
            Value::Prob(0.1),
            Value::Prob(0.2),
            Value::Prob(0.3),
            Value::Prob(0.4),
            Value::Prob(0.5)
        ])
    );
    assert_eq!(j["q"], Value::Prob(0.9));
    let events: Vec<Event> = run.drain_events().into_iter().map(|e| e.event).collect();
    let requests = events_of(&events, "request");
    assert_eq!(requests.len(), 2, "six questions at a cap of four");
    let full = serde_json::json!({
        "items": ["a", "b", "c", "d", "e"],
        "extra": "context",
    });
    for (i, request) in requests.iter().enumerate() {
        let Event::Request {
            state, questions, ..
        } = request
        else {
            unreachable!()
        };
        let state = state.full().expect("full");
        assert_eq!(
            serde_json::to_value(state).expect("json"),
            full,
            "chunk {i}"
        );
        let ids: Vec<&str> = questions.iter().map(|q| q.id()).collect();
        let expected: Vec<&str> = if i == 0 {
            vec!["ps[0]", "ps[1]", "ps[2]", "ps[3]"]
        } else {
            vec!["ps[4]", "q"]
        };
        assert_eq!(ids, expected);
    }
    assert_eq!(run.usage().calls, 2);
}

#[test]
fn a_def_runs_with_defaults_and_recursion_is_limited() {
    // Spec 8: a def with defaults; recursion past the limit is
    // `recursion_limit`.
    let mut ir = program(
        Vec::new(),
        &["x", "y"],
        vec![
            assign("x", call_unit("add", vec![num(2.0)])),
            assign("y", call_unit("add", vec![num(2.0), num(5.0)])),
        ],
    );
    ir.defs.push(def(
        "add",
        vec![param("a"), param_default("b", num(10.0))],
        vec![ret(Some(binary(BinaryOp::Add, name("a"), name("b"))))],
    ));
    let mut run = start(
        ir,
        RunOptions::default(),
        bindings(Vec::new()),
        Answers::default(),
    );
    let outputs = done_outputs(&run.next().expect("steps"));
    assert_eq!(outputs["x"], Value::Number(12.0));
    assert_eq!(outputs["y"], Value::Number(7.0));

    let mut ir = program(
        Vec::new(),
        &[],
        vec![expr(call_unit("forever", vec![num(0.0)]))],
    );
    ir.defs.push(def(
        "forever",
        vec![param("n")],
        vec![ret(Some(call_unit(
            "forever",
            vec![binary(BinaryOp::Add, name("n"), num(1.0))],
        )))],
    ));
    let mut run = start(
        ir,
        RunOptions::default(),
        bindings(Vec::new()),
        Answers::default(),
    );
    let pause = run.next().expect("steps");
    let Pause::Error { code, .. } = &pause else {
        panic!("error, got {pause:?}");
    };
    assert_eq!(*code, RuntimeErrorCode::RecursionLimit);
    assert_eq!(
        *run.state(),
        jevscript_runtime::RunState::Ended(PauseKind::Error)
    );
}

#[test]
fn a_judgment_inside_a_def_is_its_own_request() {
    // Spec 3.9: batching never crosses a call boundary.
    let dir = temp_dir("def-judge");
    let mut ir = program(
        Vec::new(),
        &["s"],
        vec![
            assign("steps", list(vec![text("a"), text("b")])),
            assign("s", call_unit("stuckish", vec![name("steps")])),
        ],
    );
    ir.defs.push(def(
        "stuckish",
        vec![param("steps")],
        vec![
            assign("same", feels("steps", "same approach")),
            ret(Some(binary(BinaryOp::Gt, name("same"), num(0.7)))),
        ],
    ));
    batch(&mut ir);
    let answers = Answers::default().prob("same", 0.9);
    let mut run = start(
        ir,
        tiny_profile_options(&dir),
        bindings(Vec::new()),
        answers,
    );
    let outputs = done_outputs(&run.next().expect("steps"));
    assert_eq!(outputs["s"], Value::Bool(true));
    assert_eq!(run.usage().calls, 1);
}

/* -------------------------------------------------------------------------- */
/* Gates                                                                       */
/* -------------------------------------------------------------------------- */

fn gated(
    risk: f64,
    confidence: f64,
    done: Option<f64>,
    arms: Vec<(Verdict, Vec<jevscript_ir::Stmt>)>,
    with_verify: bool,
) -> jevscript_ir::Ir {
    let mut body = vec![
        assign("took", text("none")),
        on_line(gate(num(risk), num(confidence), done.map(num), arms), 5),
        assign("after", boolean(true)),
    ];
    if with_verify {
        body = vec![
            assign("took", text("none")),
            assign("after", boolean(false)),
            until_verify(boolean(false), 1.0, body),
        ];
    }
    let mut ir = program(Vec::new(), &["took", "after"], body);
    ir.tasks[0].thresholds = thresholds(Some(0.2), Some(0.5), Some(0.3), Some(0.9));
    ir
}

fn arm(verdict: Verdict, label: &str) -> (Verdict, Vec<jevscript_ir::Stmt>) {
    (verdict, vec![assign("took", text(label))])
}

fn all_arms() -> Vec<(Verdict, Vec<jevscript_ir::Stmt>)> {
    vec![
        arm(Verdict::Proceed, "proceed"),
        arm(Verdict::Confirm, "confirm"),
        arm(Verdict::Escalate, "escalate"),
        arm(Verdict::Stop, "stop"),
    ]
}

#[test]
fn a_gate_computes_its_verdict_in_the_spec_order() {
    // Spec 7.6: stop if confidence < stop_confidence; else confirm if risk >=
    // risk_confirm; else escalate if confidence < min_confidence; else proceed.
    let cases = [
        (0.0, 0.1, "stop"),
        (0.9, 0.1, "stop"),
        (0.5, 0.4, "confirm"),
        (0.2, 0.9, "confirm"),
        (0.1, 0.4, "escalate"),
        (0.1, 0.9, "proceed"),
    ];
    for (risk, confidence, expected) in cases {
        let ir = gated(risk, confidence, None, all_arms(), false);
        let mut run = start(
            ir,
            RunOptions::default(),
            bindings(Vec::new()),
            Answers::default(),
        );
        let outputs = done_outputs(&run.next().expect("steps"));
        assert_eq!(
            outputs["took"],
            Value::Text(expected.into()),
            "risk {risk} confidence {confidence}"
        );
        assert_eq!(
            outputs["after"],
            Value::Bool(true),
            "a written arm continues"
        );
    }
}

#[test]
fn a_gate_default_confirm_pauses_and_continues_on_yes_or_stops_otherwise() {
    // Spec 7.6: unwritten `confirm` pauses with "Gate asked for confirmation".
    for (answer, expect_done) in [("yes", true), ("no", false)] {
        let ir = gated(0.5, 0.9, None, vec![], false);
        let mut run = start(
            ir,
            RunOptions::default(),
            bindings(Vec::new()),
            Answers::default(),
        );
        let pause = run.next().expect("steps");
        let Pause::Confirm {
            message,
            options,
            common,
            ..
        } = &pause
        else {
            panic!("confirm, got {pause:?}");
        };
        assert_eq!(message, "Gate asked for confirmation");
        assert_eq!(options, &["yes", "no"]);
        assert_eq!(common.source, at(5));
        run.resume(Resume::Answer {
            answer: answer.into(),
            text: None,
        })
        .expect("resumes");
        let pause = run.next().expect("continues");
        if expect_done {
            assert_eq!(done_outputs(&pause)["after"], Value::Bool(true));
        } else {
            assert!(matches!(pause, Pause::Stopped { .. }), "{pause:?}");
        }
    }
}

#[test]
fn a_gate_default_escalate_pauses_and_a_default_stop_ends() {
    // Spec 7.6: unwritten `escalate` pauses with escalate; unwritten `stop`
    // ends with stopped.
    let ir = gated(0.1, 0.4, None, vec![], false);
    let mut run = start(
        ir,
        RunOptions::default(),
        bindings(Vec::new()),
        Answers::default(),
    );
    let pause = run.next().expect("steps");
    assert!(matches!(pause, Pause::Escalate { .. }), "{pause:?}");
    run.resume(Resume::Continue { resume: true })
        .expect("resumes");
    let pause = run.next().expect("continues");
    assert_eq!(done_outputs(&pause)["after"], Value::Bool(true));

    let ir = gated(0.0, 0.1, None, vec![], false);
    let mut run = start(
        ir,
        RunOptions::default(),
        bindings(Vec::new()),
        Answers::default(),
    );
    let pause = run.next().expect("steps");
    assert!(matches!(pause, Pause::Stopped { .. }), "{pause:?}");
}

#[test]
fn gate_done_ends_a_task_without_verify_and_not_one_with_it() {
    // Spec 7.6: `done >= thresholds.done` makes the verdict proceed and sets
    // `gate.done`, which ends a task without `verify` and is ignored by one
    // with it.
    let ir = gated(0.9, 0.1, Some(0.95), all_arms(), false);
    let mut run = start(
        ir,
        RunOptions::default(),
        bindings(Vec::new()),
        Answers::default(),
    );
    let outputs = done_outputs(&run.next().expect("steps"));
    assert_eq!(
        outputs["took"],
        Value::Text("proceed".into()),
        "done overrides stop"
    );
    assert_eq!(outputs["after"], Value::None, "the task ended at the gate");

    let ir = gated(0.1, 0.9, Some(0.95), all_arms(), true);
    let mut run = start(
        ir,
        RunOptions::default(),
        bindings(Vec::new()),
        Answers::default(),
    );
    let outputs = done_outputs(&run.next().expect("steps"));
    assert_eq!(
        outputs["after"],
        Value::Bool(true),
        "a task with verify carries on"
    );

    // Below the threshold nothing changes.
    let ir = gated(0.1, 0.9, Some(0.5), all_arms(), false);
    let mut run = start(
        ir,
        RunOptions::default(),
        bindings(Vec::new()),
        Answers::default(),
    );
    let outputs = done_outputs(&run.next().expect("steps"));
    assert_eq!(outputs["after"], Value::Bool(true));
}

/* -------------------------------------------------------------------------- */
/* Shape                                                                       */
/* -------------------------------------------------------------------------- */

fn long_text() -> String {
    (0..40)
        .map(|i| format!("line {i} of the transcript\n"))
        .collect()
}

#[test]
fn shape_truncates_with_head_and_tail_and_records_a_warning() {
    // Spec 7.2: over the cap, `head` keeps the first tokens and `tail` the
    // last, and a `truncated` warning is recorded.
    let ir = program(
        Vec::new(),
        &["obs"],
        vec![
            assign("t", text(&long_text())),
            assign("n", num(7.0)),
            shape(
                "obs",
                false,
                vec![
                    shape_field("head", name("t"), Some(10.0), ShapePolicy::Head),
                    shape_field("tail", name("t"), Some(10.0), ShapePolicy::Tail),
                    shape_field("fits", name("t"), Some(100_000.0), ShapePolicy::Head),
                    shape_field("n", name("n"), None, ShapePolicy::Head),
                ],
            ),
        ],
    );
    let mut run = start(
        ir,
        RunOptions::default(),
        bindings(Vec::new()),
        Answers::default(),
    );
    let outputs = done_outputs(&run.next().expect("steps"));
    let Value::Record(obs) = &outputs["obs"] else {
        panic!("a record");
    };
    let Value::Text(head) = &obs["head"] else {
        panic!("text");
    };
    let Value::Text(tail) = &obs["tail"] else {
        panic!("text");
    };
    assert!(head.starts_with("line 0 of"), "{head}");
    assert!(head.chars().count() <= 40, "ten chars4 tokens: {head}");
    assert!(tail.ends_with("transcript\n"), "{tail}");
    assert!(tail.contains("line 39"), "{tail}");
    assert_eq!(obs["fits"], Value::Text(long_text()));
    assert_eq!(obs["n"], Value::Number(7.0));
    let events: Vec<Event> = run.drain_events().into_iter().map(|e| e.event).collect();
    let warnings = events_of(&events, "warning");
    assert_eq!(warnings.len(), 2);
    assert!(
        matches!(warnings[0], Event::Warning { code, message, .. } if code == "truncated" && message.contains("`head`"))
    );
}

#[test]
fn shape_strict_makes_an_overflow_an_error_pause() {
    // Spec 7.2: `shape strict:` pauses with `error` instead of truncating.
    let ir = program(
        Vec::new(),
        &["obs"],
        vec![
            assign("t", text(&long_text())),
            shape(
                "obs",
                true,
                vec![shape_field(
                    "head",
                    name("t"),
                    Some(10.0),
                    ShapePolicy::Head,
                )],
            ),
        ],
    );
    let mut run = start(
        ir,
        RunOptions::default(),
        bindings(Vec::new()),
        Answers::default(),
    );
    let pause = run.next().expect("steps");
    let Pause::Error { code, message, .. } = &pause else {
        panic!("error, got {pause:?}");
    };
    assert_eq!(*code, RuntimeErrorCode::StateTooLarge);
    assert!(message.contains("`head`"), "{message}");
}

#[test]
fn shape_focus_reduces_with_jev_one_request_per_pass() {
    // Spec 7.3: `focus` chunks the text, asks relevance per chunk in one
    // request, keeps the chunks that pass and recurses until the text fits.
    let dir = temp_dir("focus");
    // Three chunks of exactly 1500 chars4 tokens each: the chunker cuts at
    // 1500 tokens on line boundaries, so each 6000-character line is one.
    let chunk_a = format!("{}\n", "a".repeat(5999));
    let chunk_b = format!("{}\n", "b".repeat(5999));
    let chunk_c = format!("{}\n", "c".repeat(5999));
    let whole = format!("{chunk_a}{chunk_b}{chunk_c}");
    let ir = program(
        Vec::new(),
        &["obs", "short"],
        vec![
            assign("t", text(&whole)),
            shape(
                "obs",
                false,
                vec![shape_field(
                    "summary",
                    name("t"),
                    Some(1500.0),
                    ShapePolicy::Focus {
                        on: text("what matters"),
                    },
                )],
            ),
            assign("short", focus(name("t"), "what matters", 1500.0)),
        ],
    );
    let answers = Answers::default().each("keep", &[0.1, 0.9, 0.1]);
    let mut run = start(
        ir,
        tiny_profile_options(&dir),
        bindings(Vec::new()),
        answers,
    );
    let outputs = done_outputs(&run.next().expect("steps"));
    let Value::Record(obs) = &outputs["obs"] else {
        panic!("a record");
    };
    assert_eq!(obs["summary"], Value::Text(chunk_b.clone()));
    assert_eq!(outputs["short"], Value::Text(chunk_b.clone()));
    let events: Vec<Event> = run.drain_events().into_iter().map(|e| e.event).collect();
    let requests = events_of(&events, "request");
    assert_eq!(requests.len(), 2, "one request per focus, one pass each");
    let Event::Request {
        questions, state, ..
    } = requests[0]
    else {
        panic!("request");
    };
    assert_eq!(questions.len(), 3);
    assert_eq!(questions[1].path(), "chunks[1]");
    assert!(
        matches!(&questions[1], jevscript_runtime::jev::Question::Noul { condition, .. } if condition == "contains information relevant to: what matters")
    );
    assert_eq!(
        state.full().expect("full").keys().collect::<Vec<_>>(),
        vec!["chunks"]
    );

    // When every chunk is relevant the text cannot shrink, and `focus`
    // fails the way the prelude def would: `recursion_limit` (spec 7.3).
    let mut ir = program(
        Vec::new(),
        &["short"],
        vec![
            assign("t", text(&whole)),
            assign("short", focus(name("t"), "what matters", 1500.0)),
        ],
    );
    ir.tasks[0].budget = budget(Some(1000.0), None, None, None);
    let answers = Answers::default().each("keep", &[0.9, 0.9, 0.9]);
    let mut run = start(
        ir,
        tiny_profile_options(&dir),
        bindings(Vec::new()),
        answers,
    );
    let pause = run.next().expect("steps");
    let Pause::Error { code, .. } = &pause else {
        panic!("error, got {pause:?}");
    };
    assert_eq!(*code, RuntimeErrorCode::RecursionLimit);
}

/* -------------------------------------------------------------------------- */
/* Budgets                                                                     */
/* -------------------------------------------------------------------------- */

#[test]
fn a_callee_tightens_the_budget_and_the_pause_names_the_task_whose_limit_was_hit() {
    // Spec 7.1: the callee's effective budget is the smaller of its own and
    // the caller's remainder; usage counts against every task on the stack;
    // the pause names the task whose limit was hit; extending it extends only
    // that one.
    let dir = temp_dir("budget");
    let mut ir = program(
        Vec::new(),
        &["r"],
        vec![
            assign("x", text("subject")),
            assign("p0", feels("x", "q0")),
            assign("r", call_unit("helper", vec![name("x")])),
        ],
    );
    ir.tasks[0].budget = budget(Some(3.0), None, None, None);
    ir.tasks.push(task(
        "helper",
        vec![param("x")],
        budget(Some(10.0), None, None, None),
        thresholds(None, None, None, None),
        vec![
            assign("p1", feels("x", "q1")),
            assign("y", name("p1")),
            assign("p2", feels("y", "q2")),
            assign("z", name("p2")),
            assign("p3", feels("z", "q3")),
            ret(Some(name("p3"))),
        ],
    ));
    batch(&mut ir);
    let answers = Answers::default()
        .prob("p0", 0.1)
        .prob("p1", 0.2)
        .prob("p2", 0.3)
        .prob("p3", 0.4);
    let mut run = start(
        ir,
        tiny_profile_options(&dir),
        bindings(Vec::new()),
        answers,
    );
    let pause = run.next().expect("steps");
    let Pause::Budget {
        common,
        key,
        used,
        limit,
    } = &pause
    else {
        panic!("budget, got {pause:?}");
    };
    assert_eq!(common.task, "main", "the caller's limit ran out first");
    assert_eq!(*key, BudgetKey::Calls);
    assert_eq!((*used, *limit), (3.0, 3.0));
    run.resume(Resume::Extend {
        extend: BTreeMap::from([(BudgetKey::Calls, 4.0)]),
    })
    .expect("extends");
    let pause = run.next().expect("continues");
    assert_eq!(done_outputs(&pause)["r"], Value::Prob(0.4));
    assert_eq!(run.usage().calls, 4);

    // The callee's own tighter limit is what pauses when the caller is
    // roomy, and the pause names the callee.
    let mut ir = program(
        Vec::new(),
        &["r"],
        vec![
            assign("x", text("subject")),
            assign("r", call_unit("helper", vec![name("x")])),
        ],
    );
    ir.tasks[0].budget = budget(Some(50.0), None, None, None);
    ir.tasks.push(task(
        "helper",
        vec![param("x")],
        budget(Some(1.0), None, None, None),
        thresholds(None, None, None, None),
        vec![
            assign("p1", feels("x", "q1")),
            assign("y", name("p1")),
            assign("p2", feels("y", "q2")),
            ret(Some(name("p2"))),
        ],
    ));
    batch(&mut ir);
    let answers = Answers::default().prob("p1", 0.2).prob("p2", 0.3);
    let mut run = start(
        ir,
        tiny_profile_options(&dir),
        bindings(Vec::new()),
        answers,
    );
    let pause = run.next().expect("steps");
    let Pause::Budget { common, .. } = &pause else {
        panic!("budget, got {pause:?}");
    };
    assert_eq!(common.task, "helper");
    run.resume(Resume::Extend {
        extend: BTreeMap::from([(BudgetKey::Calls, 2.0)]),
    })
    .expect("extends");
    let pause = run.next().expect("continues");
    assert_eq!(done_outputs(&pause)["r"], Value::Prob(0.3));
}

#[test]
fn steps_count_the_outermost_loop_and_usd_comes_from_the_profile() {
    // Spec 7.1: `steps` are iterations of the outermost loop; `usd` is
    // estimated from usage and the profile's price.
    let dir = temp_dir("steps");
    let mut ir = program(
        Vec::new(),
        &["n"],
        vec![
            assign("n", num(0.0)),
            loop_max(
                5.0,
                vec![
                    loop_max(
                        3.0,
                        vec![assign("n", binary(BinaryOp::Add, name("n"), num(1.0)))],
                    ),
                    assign("x", text("s")),
                    assign("p", feels("x", "q")),
                ],
            ),
        ],
    );
    ir.tasks[0].budget = budget(Some(50.0), None, None, Some(2.0));
    batch(&mut ir);
    let mut run = start(
        ir,
        tiny_profile_options(&dir),
        bindings(Vec::new()),
        Answers::default(),
    );
    let pause = run.next().expect("steps");
    let Pause::Budget {
        key, used, limit, ..
    } = &pause
    else {
        panic!("budget, got {pause:?}");
    };
    assert_eq!(*key, BudgetKey::Steps);
    assert_eq!((*used, *limit), (2.0, 2.0));
    run.resume(Resume::Extend {
        extend: BTreeMap::from([(BudgetKey::Steps, 10.0)]),
    })
    .expect("extends");
    let pause = run.next().expect("continues");
    let Pause::Done { usage, outputs, .. } = &pause else {
        panic!("done, got {pause:?}");
    };
    assert_eq!(outputs["n"], Value::Number(15.0));
    assert_eq!(usage.steps, 5);
    assert_eq!(usage.calls, 5);
    assert_eq!(usage.tokens, 5000);
    // 5000 tokens at one dollar per million.
    assert!((usage.usd - 0.005).abs() < 1e-12, "{}", usage.usd);
}

#[test]
fn a_tool_return_is_checked_against_its_declared_signature() {
    // Spec 9.4: the runtime checks the adapter's actual return against the
    // declared return type (`type_error`).
    let log = call_log();
    let mut ir = program(
        vec![need("tree", CapabilityKind::Tool)],
        &["ok"],
        vec![assign("ok", verb("tree", "tests_pass", vec![]))],
    );
    ir.needs[0].signatures = vec![signature("tests_pass", &[], ReturnType::Bool)];
    let tool = FakeTool::new("tree", &log).verb("tests_pass", vec![Value::Text("yes".into())]);
    let mut run = start(
        ir,
        RunOptions::default(),
        bindings(vec![("tree", Box::new(tool))]),
        Answers::default(),
    );
    let pause = run.next().expect("steps");
    let Pause::Error { code, message, .. } = &pause else {
        panic!("error, got {pause:?}");
    };
    assert_eq!(*code, RuntimeErrorCode::TypeError);
    assert!(message.contains("bool"), "{message}");
}
