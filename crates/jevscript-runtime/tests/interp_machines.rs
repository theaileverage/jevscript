//! Machine execution laws (spec section 7.8).

mod support;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use jevscript_ir::{BinaryOp, Budget, CapabilityKind, Ir, ShapePolicy, Thresholds};
use jevscript_runtime::jev::{
    Instruction, JevAnswer, JevError, JevRequest, JevResponse, JevUsage, Question,
};
use jevscript_runtime::record::Event;
use jevscript_runtime::run::Run;
use jevscript_runtime::{JevClient, Pause, Resume, Value};
use support::*;

fn machine_program(machine: jevscript_ir::Machine) -> Ir {
    let mut ir = program(
        Vec::new(),
        &["result"],
        vec![assign(
            "result",
            call_named(name(&machine.name), Vec::new(), vec![("max", num(20.0))]),
        )],
    );
    ir.machines.push(machine);
    batch(&mut ir);
    ir
}

fn output<'a>(pause: &'a Pause, name: &str) -> &'a Value {
    let Pause::Done { outputs, .. } = pause else {
        panic!("expected done, got {pause:?}")
    };
    &outputs[name]
}

fn field<'a>(value: &'a Value, name: &str) -> &'a Value {
    let Value::Record(record) = value else {
        panic!("expected record, got {value:?}")
    };
    &record[name]
}

struct RecordingChoices {
    labels: Mutex<Vec<String>>,
    requests: Arc<Mutex<Vec<JevRequest>>>,
}

impl JevClient for RecordingChoices {
    fn send(&self, request: &JevRequest) -> Result<JevResponse, JevError> {
        self.requests
            .lock()
            .expect("not poisoned")
            .push(request.clone());
        let label = self.labels.lock().expect("not poisoned").remove(0);
        let Question::Choice { labels, .. } = &request.questions[0] else {
            panic!("machine asks a Choice")
        };
        let probabilities = labels
            .iter()
            .map(|candidate| {
                (
                    candidate.name.clone(),
                    if candidate.name == label { 0.9 } else { 0.1 },
                )
            })
            .collect();
        Ok(JevResponse {
            answers: vec![JevAnswer::Choice {
                id: "event".to_string(),
                label,
                confidence: 0.9,
                probabilities,
            }],
            usage: JevUsage::default(),
            latency_ms: 0,
        })
    }
}

#[test]
fn guards_filter_the_exact_menu_and_recent_is_the_last_six_completed_steps() {
    // Spec 7.8 steps 2, 3 and 5: guards are code, the request has exactly
    // state/goal/obs/recent, and recent is capped to six runtime records.
    let machine = machine(
        "review",
        Vec::new(),
        Budget::default(),
        Thresholds::default(),
        "advance",
        vec![shape_field(
            "summary",
            text("ready"),
            Some(20.0),
            ShapePolicy::Head,
        )],
        vec![
            machine_state(
                "working",
                false,
                vec![
                    transition(
                        "blocked",
                        "a false guarded event",
                        "done",
                        Some(boolean(false)),
                        false,
                        Vec::new(),
                    ),
                    transition("advance", "keep moving", "working", None, false, Vec::new()),
                ],
            ),
            machine_state("done", true, Vec::new()),
        ],
    );
    let mut ir = machine_program(machine);
    rewrite_max(&mut ir, 8.0);
    let requests = Arc::new(Mutex::new(Vec::new()));
    let client = RecordingChoices {
        labels: Mutex::new(vec!["stay".to_string(); 8]),
        requests: Arc::clone(&requests),
    };
    let dir = temp_dir("machine-menu");
    let mut run = Run::create(ir, "main", tiny_profile_options(&dir), bindings(Vec::new()))
        .expect("starts")
        .with_client(Box::new(client));
    let pauses = drive(&mut run, |_| None);
    assert_eq!(pauses.len(), 1);
    assert_eq!(
        field(output(&pauses[0], "result"), "steps"),
        &Value::Number(8.0)
    );

    let requests = requests.lock().expect("not poisoned");
    assert_eq!(requests.len(), 8);
    for request in requests.iter() {
        assert_eq!(
            request.state.keys().map(String::as_str).collect::<Vec<_>>(),
            vec!["goal", "obs", "recent", "state"]
        );
        let Question::Choice {
            id,
            path,
            question,
            labels,
            instruction,
        } = &request.questions[0]
        else {
            unreachable!()
        };
        assert_eq!(id, "event");
        assert_eq!(path, "obs");
        assert_eq!(
            question.as_deref(),
            Some("Which enabled event should happen next to advance the goal?")
        );
        assert_eq!(
            labels
                .iter()
                .map(|label| label.name.as_str())
                .collect::<Vec<_>>(),
            vec!["advance", "stay"]
        );
        assert_eq!(
            instruction,
            &Some(Instruction {
                compare: vec!["state".into(), "goal".into(), "recent".into()],
                ..Instruction::default()
            })
        );
    }
    let recent = requests[7].state["recent"].as_array().expect("recent list");
    assert_eq!(recent.len(), 6);
    assert_eq!(recent[0]["step"], 2);
    assert_eq!(recent[5]["step"], 7);
}

/// Change the generated main call's `max` without coupling every test to the
/// IR expression layout beyond this support assertion.
fn rewrite_max(ir: &mut Ir, max: f64) {
    let jevscript_ir::Stmt::Assign {
        value: jevscript_ir::Expr::Call { args, .. },
        ..
    } = &mut ir.tasks[0].body[0]
    else {
        panic!("main assigns the machine result")
    };
    let arg = args
        .iter_mut()
        .find(|arg| arg.name.as_deref() == Some("max"));
    let Some(arg) = arg else { panic!("max exists") };
    arg.value = num(max);
}

#[test]
fn no_enabled_events_records_a_stay_before_pausing_and_counts_only_after_resume() {
    // Spec 7.8 recording-a-decision rule: the sentinel has no Jev request,
    // the surfaced pause names the machine/state, and explicit resume turns
    // it into the one completed result event.
    let machine = machine(
        "blocked",
        Vec::new(),
        Budget::default(),
        Thresholds::default(),
        "wait",
        Vec::new(),
        vec![
            machine_state(
                "waiting",
                false,
                vec![transition(
                    "ready",
                    "ready now",
                    "done",
                    Some(boolean(false)),
                    false,
                    Vec::new(),
                )],
            ),
            machine_state("done", true, Vec::new()),
        ],
    );
    let mut ir = machine_program(machine);
    rewrite_max(&mut ir, 1.0);
    let dir = temp_dir("machine-no-enabled");
    let mut run = Run::create(ir, "main", tiny_profile_options(&dir), bindings(Vec::new()))
        .expect("starts")
        .with_client(Box::new(NeverJev));
    let pause = run.next().expect("pauses");
    let Pause::Escalate { common, reason, .. } = &pause else {
        panic!("expected escalation, got {pause:?}")
    };
    assert_eq!(reason, "no_enabled_events");
    assert_eq!(common.task, "machine:blocked");
    assert_eq!(common.state.as_deref(), Some("waiting"));
    assert_eq!(common.step, 0, "the stay is not counted before resume");
    let first_events: Vec<Event> = run.drain_events().into_iter().map(|e| e.event).collect();
    assert!(
        !first_events
            .iter()
            .any(|e| matches!(e, Event::Request { .. }))
    );
    let sentinel = first_events
        .iter()
        .find(|event| matches!(event, Event::MachineStep { .. }))
        .expect("machine step before pause");
    assert!(matches!(
        sentinel,
        Event::MachineStep {
            enabled,
            chosen,
            probabilities,
            confidence,
            to,
            ..
        } if enabled == &["stay"] && chosen == "stay" && probabilities.is_empty()
            && *confidence == 0.0 && to == "waiting"
    ));
    run.resume(Resume::Continue { resume: true })
        .expect("resumes");
    let done = run.next().expect("finishes at max");
    assert_eq!(field(output(&done, "result"), "steps"), &Value::Number(1.0));
    let Value::List(events) = field(output(&done, "result"), "events") else {
        panic!("events list")
    };
    assert_eq!(events.len(), 1);
    assert_eq!(field(&events[0], "event"), &Value::Text("stay".into()));
}

#[test]
fn risky_confirmation_declined_is_a_recorded_stay_and_runs_no_action() {
    // Spec 7.8 step 6: risk 1 trips confirm; declining preserves Jev's chosen
    // label but permits no transition and executes no action.
    let log = call_log();
    let machine = machine(
        "review",
        Vec::new(),
        Budget::default(),
        thresholds(Some(0.5), None, None, None),
        "review",
        Vec::new(),
        vec![
            machine_state(
                "working",
                false,
                vec![transition(
                    "reject",
                    "reject it",
                    "done",
                    None,
                    true,
                    vec![expr(verb("tree", "ping", Vec::new()))],
                )],
            ),
            machine_state("done", true, Vec::new()),
        ],
    );
    let mut ir = machine_program(machine);
    ir.needs.push(need("tree", CapabilityKind::Tool));
    rewrite_max(&mut ir, 1.0);
    let dir = temp_dir("machine-decline");
    let client = Answers::default().label("event", "reject", 0.9).client();
    let tree = FakeTool::new("tree", &log).verb("ping", vec![Value::None]);
    let mut run = Run::create(
        ir,
        "main",
        tiny_profile_options(&dir),
        bindings(vec![("tree", Box::new(tree))]),
    )
    .expect("starts")
    .with_client(Box::new(client));
    let pauses = drive(&mut run, |pause| match pause {
        Pause::Confirm { .. } => Some(Resume::Answer {
            answer: "no".into(),
            text: None,
        }),
        _ => None,
    });
    assert!(matches!(&pauses[0], Pause::Confirm { common, .. }
        if common.task == "machine:review" && common.state.as_deref() == Some("working")));
    assert!(logged(&log).is_empty());
    let result = output(pauses.last().expect("done"), "result");
    assert_eq!(field(result, "state"), &Value::Text("working".into()));
    let events = match field(result, "events") {
        Value::List(events) => events,
        _ => panic!("events list"),
    };
    assert_eq!(field(&events[0], "event"), &Value::Text("stay".into()));
    assert_eq!(field(&events[0], "to"), &Value::Text("working".into()));
    let machine_steps: Vec<Event> = run
        .drain_events()
        .into_iter()
        .map(|record| record.event)
        .filter(|event| matches!(event, Event::MachineStep { .. }))
        .collect();
    assert!(
        matches!(&machine_steps[0], Event::MachineStep { chosen, to, .. }
        if chosen == "reject" && to == "working")
    );
}

#[test]
fn guarded_terminal_entry_is_verified_and_unguarded_entry_is_not() {
    // Spec 7.8 result rule: terminal is done in both cases, but only a code
    // guard on the entering transition proves it.
    let guarded = machine(
        "guarded",
        Vec::new(),
        Budget::default(),
        Thresholds::default(),
        "finish",
        Vec::new(),
        vec![
            machine_state(
                "work",
                false,
                vec![transition(
                    "finish",
                    "finish",
                    "done",
                    Some(boolean(true)),
                    false,
                    Vec::new(),
                )],
            ),
            machine_state("done", true, Vec::new()),
        ],
    );
    let mut unguarded = guarded.clone();
    unguarded.name = "unguarded".into();
    unguarded.states[0].transitions[0].when = None;
    let mut ir = program(
        Vec::new(),
        &["guarded_result", "unguarded_result"],
        vec![
            assign(
                "guarded_result",
                call_named(name("guarded"), Vec::new(), vec![("max", num(1.0))]),
            ),
            assign(
                "unguarded_result",
                call_named(name("unguarded"), Vec::new(), vec![("max", num(1.0))]),
            ),
        ],
    );
    ir.machines = vec![guarded, unguarded];
    batch(&mut ir);
    let dir = temp_dir("machine-proof");
    let mut run = start(
        ir,
        tiny_profile_options(&dir),
        bindings(Vec::new()),
        Answers::default().label("event", "finish", 0.9),
    );
    let pauses = drive(&mut run, |_| None);
    let done = pauses.last().expect("done");
    assert_eq!(
        field(output(done, "guarded_result"), "done"),
        &Value::Bool(true)
    );
    assert_eq!(
        field(output(done, "guarded_result"), "verified"),
        &Value::Bool(true)
    );
    assert_eq!(
        field(output(done, "unguarded_result"), "verified"),
        &Value::Bool(false)
    );
}

#[test]
fn nested_machine_calls_share_the_stack_and_return_results() {
    // Spec 7.1 and 7.8: a machine action may call another machine and both
    // complete on the same synchronous call stack.
    let inner = machine(
        "inner",
        Vec::new(),
        Budget::default(),
        Thresholds::default(),
        "inner",
        Vec::new(),
        vec![
            machine_state(
                "inside",
                false,
                vec![transition(
                    "finish",
                    "finish",
                    "done",
                    None,
                    false,
                    Vec::new(),
                )],
            ),
            machine_state("done", true, Vec::new()),
        ],
    );
    let outer = machine(
        "outer",
        Vec::new(),
        Budget::default(),
        Thresholds::default(),
        "outer",
        Vec::new(),
        vec![
            machine_state(
                "outside",
                false,
                vec![transition(
                    "enter",
                    "enter inner",
                    "done",
                    Some(boolean(true)),
                    false,
                    vec![assign(
                        "inner_result",
                        call_named(name("inner"), Vec::new(), vec![("max", num(1.0))]),
                    )],
                )],
            ),
            machine_state("done", true, Vec::new()),
        ],
    );
    let mut ir = machine_program(outer);
    ir.machines.push(inner);
    rewrite_max(&mut ir, 1.0);
    batch(&mut ir);
    let requests = Arc::new(Mutex::new(Vec::new()));
    let client = RecordingChoices {
        labels: Mutex::new(vec!["enter".into(), "finish".into()]),
        requests: Arc::clone(&requests),
    };
    let dir = temp_dir("machine-nested");
    let mut run = Run::create(ir, "main", tiny_profile_options(&dir), bindings(Vec::new()))
        .expect("starts")
        .with_client(Box::new(client));
    let pauses = drive(&mut run, |_| None);
    assert_eq!(
        field(output(&pauses[0], "result"), "state"),
        &Value::Text("done".into())
    );
    assert_eq!(requests.lock().expect("not poisoned").len(), 2);
}

#[test]
fn caller_remaining_budget_tightens_a_machine_and_replay_surfaces_the_same_pause() {
    // Spec 7.1: a machine cannot widen the caller's remaining calls budget;
    // conformance 4: the directly surfaced budget pause replays identically
    // without a live Jev call.
    let machine = machine(
        "bounded",
        Vec::new(),
        budget(Some(5.0), None, None, None),
        Thresholds::default(),
        "stay",
        Vec::new(),
        vec![machine_state(
            "work",
            false,
            vec![transition(
                "again",
                "again",
                "work",
                None,
                false,
                Vec::new(),
            )],
        )],
    );
    let mut ir = machine_program(machine);
    ir.tasks[0].budget.calls = Some(1.0);
    rewrite_max(&mut ir, 2.0);
    let dir = temp_dir("machine-budget");
    let recording = dir.join("run.jsonl");
    let mut options = tiny_profile_options(&dir);
    options.record = Some(recording.clone());
    let mut run = Run::create(ir.clone(), "main", options, bindings(Vec::new()))
        .expect("starts")
        .with_client(Box::new(
            Answers::default().label("event", "stay", 0.9).client(),
        ));
    let pauses = drive(&mut run, |pause| match pause {
        Pause::Budget { .. } => Some(Resume::Extend {
            extend: BTreeMap::from([(jevscript_ir::BudgetKey::Calls, 2.0)]),
        }),
        _ => None,
    });
    assert!(
        matches!(&pauses[0], Pause::Budget { common, used, limit, .. }
        if common.task == "main" && common.state.is_none() && *used == 1.0 && *limit == 1.0)
    );
    let mut replay_options = tiny_profile_options(&dir);
    replay_options.replay = Some(recording);
    let mut replay = Run::create(ir, "main", replay_options, bindings(Vec::new()))
        .expect("replay starts")
        .with_client(Box::new(NeverJev));
    let replayed = drive(&mut replay, |_| None);
    assert_eq!(replayed, pauses);
}

#[test]
fn sampled_machine_choice_uses_a_recorded_draw_and_replays_exactly() {
    // Spec 6.11 applied to 7.8: run-level sampling changes only `chosen`,
    // preserves Jev's probabilities/confidence, and its recorded draw makes
    // replay deterministic with zero live calls.
    let machine = machine(
        "sampled",
        Vec::new(),
        Budget::default(),
        Thresholds::default(),
        "sample",
        Vec::new(),
        vec![
            machine_state(
                "work",
                false,
                vec![transition(
                    "finish",
                    "finish",
                    "done",
                    None,
                    false,
                    Vec::new(),
                )],
            ),
            machine_state("done", true, Vec::new()),
        ],
    );
    let mut ir = machine_program(machine);
    rewrite_max(&mut ir, 1.0);
    let dir = temp_dir("machine-sampling");
    let recording = dir.join("run.jsonl");
    let client = jevscript_runtime::ScriptedJevClient::from_fn(|_| {
        Ok(JevResponse {
            answers: vec![JevAnswer::Choice {
                id: "event".into(),
                label: "finish".into(),
                confidence: 0.7,
                probabilities: BTreeMap::from([("finish".into(), 0.0), ("stay".into(), 1.0)]),
            }],
            usage: JevUsage::default(),
            latency_ms: 0,
        })
    });
    let mut options = tiny_profile_options(&dir);
    options.record = Some(recording.clone());
    options.sample = Some(jevscript_runtime::rpc::Sample::Seeded { seed: 7 });
    let mut run = Run::create(ir.clone(), "main", options, bindings(Vec::new()))
        .expect("starts")
        .with_client(Box::new(client));
    let pauses = drive(&mut run, |_| None);
    assert_eq!(
        field(output(&pauses[0], "result"), "state"),
        &Value::Text("work".into())
    );
    let events: Vec<Event> = run
        .drain_events()
        .into_iter()
        .map(|event| event.event)
        .collect();
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Event::Answers { sampled: true, .. }))
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Event::MachineStep {
        chosen, probabilities, confidence, to, ..
    } if chosen == "stay" && probabilities["stay"] == 1.0 && *confidence == 0.7 && to == "work"))
    );

    let mut replay_options = tiny_profile_options(&dir);
    replay_options.replay = Some(recording);
    replay_options.sample = Some(jevscript_runtime::rpc::Sample::Seeded { seed: 7 });
    let mut replay = Run::create(ir, "main", replay_options, bindings(Vec::new()))
        .expect("replay starts")
        .with_client(Box::new(NeverJev));
    let replayed = drive(&mut replay, |_| None);
    assert_eq!(replayed, pauses);
}

#[test]
fn terminal_initial_state_returns_before_evaluating_goal() {
    // Spec 7.8 step 1: a terminal initial state returns before evaluating any
    // request field, including a side-effectful goal expression.
    let log = call_log();
    let mut machine = machine(
        "terminal",
        Vec::new(),
        Budget::default(),
        Thresholds::default(),
        "unused",
        Vec::new(),
        vec![machine_state("done", true, Vec::new())],
    );
    machine.goal = Some(verb("tree", "peek", Vec::new()));
    let mut ir = machine_program(machine);
    ir.needs.push(need("tree", CapabilityKind::Tool));
    let tree = FakeTool::new("tree", &log).verb("peek", vec![Value::Text("seen".into())]);
    let dir = temp_dir("machine-terminal-goal");
    let mut run = Run::create(
        ir,
        "main",
        tiny_profile_options(&dir),
        bindings(vec![("tree", Box::new(tree))]),
    )
    .expect("starts")
    .with_client(Box::new(NeverJev));
    let pause = run.next().expect("returns terminal result");
    assert_eq!(
        field(output(&pause, "result"), "state"),
        &Value::Text("done".into())
    );
    assert!(logged(&log).is_empty(), "terminal machine evaluated goal");
}

#[test]
fn sampled_machine_rejects_a_malformed_choice_before_drawing() {
    // Spec 6.11 and 7.8: sampling may change one declared Choice label into
    // another, but it cannot hide an answer outside the machine's legal menu.
    let machine = machine(
        "sampled",
        Vec::new(),
        Budget::default(),
        Thresholds::default(),
        "sample",
        Vec::new(),
        vec![
            machine_state(
                "work",
                false,
                vec![transition(
                    "finish",
                    "finish",
                    "done",
                    None,
                    false,
                    Vec::new(),
                )],
            ),
            machine_state("done", true, Vec::new()),
        ],
    );
    let malformed = [
        (
            "unknown-label",
            "not_enabled",
            BTreeMap::from([("finish".into(), 0.0), ("stay".into(), 1.0)]),
        ),
        (
            "unknown-probability-label",
            "finish",
            BTreeMap::from([("not_enabled".into(), 1.0)]),
        ),
    ];
    for (case, label, probabilities) in malformed {
        let label = label.to_string();
        let client = jevscript_runtime::ScriptedJevClient::from_fn(move |_| {
            Ok(JevResponse {
                answers: vec![JevAnswer::Choice {
                    id: "event".into(),
                    label: label.clone(),
                    confidence: 0.7,
                    probabilities: probabilities.clone(),
                }],
                usage: JevUsage::default(),
                latency_ms: 0,
            })
        });
        let dir = temp_dir(&format!("machine-sampling-{case}"));
        let mut options = tiny_profile_options(&dir);
        options.sample = Some(jevscript_runtime::rpc::Sample::Seeded { seed: 7 });
        let mut run = Run::create(
            machine_program(machine.clone()),
            "main",
            options,
            bindings(Vec::new()),
        )
        .expect("starts")
        .with_client(Box::new(client));
        let pause = run.next().expect("rejects malformed Choice");
        assert!(matches!(
            pause,
            Pause::Error {
                code: jevscript_runtime::RuntimeErrorCode::JevRejected,
                ..
            }
        ));
        assert!(
            !run.drain_events().iter().any(|record| matches!(
                record.event,
                Event::Draw {
                    kind: jevscript_runtime::record::DrawKind::Random,
                    ..
                }
            )),
            "malformed Choice was sampled before validation"
        );
    }
}

#[test]
fn budget_extension_targets_the_exact_recursive_machine_frame() {
    // Spec 7.1: recursive calls may have the same visible unit name, but an
    // extension applies only to the dynamic frame that actually hit its limit.
    let recur = machine(
        "recur",
        vec![param("depth")],
        budget(None, None, None, Some(1.0)),
        Thresholds::default(),
        "recur",
        Vec::new(),
        vec![
            machine_state(
                "work",
                false,
                vec![transition(
                    "again",
                    "recur once",
                    "done",
                    None,
                    false,
                    vec![if_else(
                        binary(BinaryOp::Gt, name("depth"), num(0.0)),
                        vec![expr(call_named(
                            name("recur"),
                            vec![binary(BinaryOp::Sub, name("depth"), num(1.0))],
                            vec![("max", num(1.0))],
                        ))],
                        None,
                    )],
                )],
            ),
            machine_state("done", true, Vec::new()),
        ],
    );
    let mut ir = program(
        Vec::new(),
        &["result"],
        vec![assign(
            "result",
            call_named(name("recur"), vec![num(1.0)], vec![("max", num(1.0))]),
        )],
    );
    ir.machines.push(recur);
    batch(&mut ir);
    let requests = Arc::new(Mutex::new(Vec::new()));
    let client = RecordingChoices {
        labels: Mutex::new(vec!["again".into(), "again".into()]),
        requests: Arc::clone(&requests),
    };
    let dir = temp_dir("machine-recursive-budget-frame");
    let mut run = Run::create(ir, "main", tiny_profile_options(&dir), bindings(Vec::new()))
        .expect("starts")
        .with_client(Box::new(client));
    let pause = run.next().expect("outer frame pauses");
    assert!(matches!(
        pause,
        Pause::Budget {
            ref common,
            key: jevscript_ir::BudgetKey::Steps,
            used: 1.0,
            limit: 1.0,
        } if common.task == "machine:recur"
    ));
    run.resume(Resume::Extend {
        extend: BTreeMap::from([(jevscript_ir::BudgetKey::Steps, 2.0)]),
    })
    .expect("extends outer frame");
    let done = run.next().expect("continues through inner request");
    assert!(matches!(done, Pause::Done { .. }), "got {done:?}");
    assert_eq!(requests.lock().expect("not poisoned").len(), 2);
}

#[test]
fn budget_extension_targets_the_exact_recursive_task_frame() {
    // Spec 7.1 applies the same dynamic-frame identity to recursive tasks;
    // matching by their repeated qualified name would extend the inner frame.
    let mut ir = program(
        Vec::new(),
        &["result"],
        vec![assign("result", call_unit("helper", vec![num(1.0)]))],
    );
    ir.tasks.push(task(
        "helper",
        vec![param("depth")],
        budget(Some(1.0), None, None, None),
        Thresholds::default(),
        vec![if_else(
            binary(BinaryOp::Gt, name("depth"), num(0.0)),
            vec![
                assign("x", text("subject")),
                assign("p", feels("x", "continue")),
                ret(Some(call_unit(
                    "helper",
                    vec![binary(BinaryOp::Sub, name("depth"), num(1.0))],
                ))),
            ],
            Some(vec![
                assign("x", text("subject")),
                assign("p", feels("x", "finish")),
                ret(Some(name("p"))),
            ]),
        )],
    ));
    batch(&mut ir);
    let dir = temp_dir("task-recursive-budget-frame");
    let mut run = start(
        ir,
        tiny_profile_options(&dir),
        bindings(Vec::new()),
        Answers::default().prob("p", 0.8),
    );
    let pause = run.next().expect("outer frame pauses");
    assert!(matches!(
        pause,
        Pause::Budget {
            ref common,
            key: jevscript_ir::BudgetKey::Calls,
            used: 1.0,
            limit: 1.0,
        } if common.task == "helper"
    ));
    run.resume(Resume::Extend {
        extend: BTreeMap::from([(jevscript_ir::BudgetKey::Calls, 2.0)]),
    })
    .expect("extends outer frame");
    let done = run.next().expect("continues through recursive task");
    assert!(matches!(done, Pause::Done { .. }), "got {done:?}");
}

#[test]
fn machine_thresholds_stop_or_resume_escalation_as_a_counted_stay() {
    // Spec 7.8 step 6 applies the section 7.6 order. A resumed low-confidence
    // escalation records the model choice but stays and re-observes; a stop
    // records the denied destination before ending the run.
    let base = machine(
        "gated",
        Vec::new(),
        Budget::default(),
        thresholds(None, Some(0.6), Some(0.2), None),
        "gate",
        Vec::new(),
        vec![
            machine_state(
                "work",
                false,
                vec![transition(
                    "finish",
                    "finish",
                    "done",
                    None,
                    false,
                    Vec::new(),
                )],
            ),
            machine_state("done", true, Vec::new()),
        ],
    );
    let calls = Arc::new(Mutex::new(0_u32));
    let seen = Arc::clone(&calls);
    let client = jevscript_runtime::ScriptedJevClient::from_fn(move |_| {
        let mut calls = seen.lock().expect("not poisoned");
        *calls += 1;
        Ok(JevResponse {
            answers: vec![JevAnswer::Choice {
                id: "event".into(),
                label: "finish".into(),
                confidence: if *calls == 1 { 0.4 } else { 0.9 },
                probabilities: BTreeMap::from([("finish".into(), 0.9), ("stay".into(), 0.1)]),
            }],
            usage: JevUsage::default(),
            latency_ms: 0,
        })
    });
    let mut ir = machine_program(base.clone());
    rewrite_max(&mut ir, 2.0);
    let dir = temp_dir("machine-escalate");
    let mut run = Run::create(ir, "main", tiny_profile_options(&dir), bindings(Vec::new()))
        .expect("starts")
        .with_client(Box::new(client));
    let pauses = drive(&mut run, |pause| match pause {
        Pause::Escalate { .. } => Some(Resume::Continue { resume: true }),
        _ => None,
    });
    assert!(matches!(&pauses[0], Pause::Escalate { common, .. }
        if common.task == "machine:gated" && common.state.as_deref() == Some("work")));
    let result = output(pauses.last().expect("done"), "result");
    assert_eq!(field(result, "steps"), &Value::Number(2.0));
    let Value::List(events) = field(result, "events") else {
        panic!("events list")
    };
    assert_eq!(field(&events[0], "event"), &Value::Text("stay".into()));
    assert_eq!(field(&events[0], "to"), &Value::Text("work".into()));
    assert_eq!(field(&events[1], "to"), &Value::Text("done".into()));

    let mut stopped = base;
    stopped.thresholds.stop_confidence = Some(0.95);
    stopped.thresholds.min_confidence = None;
    let ir = machine_program(stopped);
    let client = Answers::default().label("event", "finish", 0.5).client();
    let dir = temp_dir("machine-stop");
    let mut run = Run::create(ir, "main", tiny_profile_options(&dir), bindings(Vec::new()))
        .expect("starts")
        .with_client(Box::new(client));
    let pause = run.next().expect("stops");
    assert!(matches!(pause, Pause::Stopped { ref common, .. }
        if common.task == "machine:gated" && common.state.as_deref() == Some("work")));
    let steps: Vec<Event> = run
        .drain_events()
        .into_iter()
        .map(|record| record.event)
        .filter(|event| matches!(event, Event::MachineStep { .. }))
        .collect();
    assert!(matches!(&steps[0], Event::MachineStep { chosen, to, .. }
        if chosen == "finish" && to == "work"));
}

#[test]
fn exhausted_step_budget_pauses_before_observation_or_model_effects() {
    // Spec 7.1 and 7.8: the next machine step is preflighted against every
    // frame. Neither the machine's own exhausted limit nor its caller's may
    // allow observation or a Jev request before the budget pause.
    for caller_limit in [None, Some(0.0)] {
        let log = call_log();
        let machine_steps = if caller_limit.is_none() {
            Some(0.0)
        } else {
            Some(5.0)
        };
        let machine = machine(
            "bounded",
            Vec::new(),
            budget(None, None, None, machine_steps),
            Thresholds::default(),
            "bounded",
            vec![shape_field(
                "seen",
                verb("tree", "peek", Vec::new()),
                None,
                ShapePolicy::Head,
            )],
            vec![machine_state(
                "work",
                false,
                vec![transition(
                    "again",
                    "again",
                    "work",
                    None,
                    false,
                    Vec::new(),
                )],
            )],
        );
        let mut ir = machine_program(machine);
        ir.needs.push(need("tree", CapabilityKind::Tool));
        ir.tasks[0].budget.steps = caller_limit;
        let tree = FakeTool::new("tree", &log).verb("peek", vec![Value::Text("seen".into())]);
        let dir = temp_dir("machine-step-preflight");
        let mut run = Run::create(
            ir,
            "main",
            tiny_profile_options(&dir),
            bindings(vec![("tree", Box::new(tree))]),
        )
        .expect("starts")
        .with_client(Box::new(NeverJev));
        let pause = run.next().expect("budget pauses");
        let Pause::Budget {
            common,
            used,
            limit,
            ..
        } = pause
        else {
            panic!("expected budget pause")
        };
        assert_eq!(used, 0.0);
        assert_eq!(limit, 0.0);
        if caller_limit.is_some() {
            assert_eq!(common.task, "main");
            assert!(common.state.is_none());
        } else {
            assert_eq!(common.task, "machine:bounded");
            assert_eq!(common.state.as_deref(), Some("work"));
        }
        assert!(
            logged(&log).is_empty(),
            "observation ran before budget pause"
        );
        let events: Vec<Event> = run
            .drain_events()
            .into_iter()
            .map(|event| event.event)
            .collect();
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, Event::Request { .. }))
        );
    }
}

#[test]
fn nested_step_consumption_is_rechecked_before_the_outer_step_completes() {
    // Spec 7.1: observation may call another machine, and that callee's step
    // counts against every machine/task frame. The outer step must re-check
    // its preflight reservation before it commits and cannot silently exceed
    // the remaining limit.
    let inner = machine(
        "inner",
        Vec::new(),
        Budget::default(),
        Thresholds::default(),
        "inner",
        Vec::new(),
        vec![
            machine_state(
                "inside",
                false,
                vec![transition(
                    "finish",
                    "finish",
                    "done",
                    None,
                    false,
                    Vec::new(),
                )],
            ),
            machine_state("done", true, Vec::new()),
        ],
    );
    let outer = machine(
        "outer",
        Vec::new(),
        budget(None, None, None, Some(1.0)),
        Thresholds::default(),
        "outer",
        vec![shape_field(
            "inner",
            call_named(name("inner"), Vec::new(), vec![("max", num(1.0))]),
            None,
            ShapePolicy::Head,
        )],
        vec![machine_state(
            "outside",
            false,
            vec![transition(
                "again",
                "again",
                "outside",
                None,
                false,
                Vec::new(),
            )],
        )],
    );
    let mut ir = machine_program(outer);
    ir.machines.push(inner);
    rewrite_max(&mut ir, 1.0);
    batch(&mut ir);
    let requests = Arc::new(Mutex::new(Vec::new()));
    let client = RecordingChoices {
        labels: Mutex::new(vec!["finish".into(), "stay".into()]),
        requests: Arc::clone(&requests),
    };
    let dir = temp_dir("machine-step-recheck");
    let mut run = Run::create(ir, "main", tiny_profile_options(&dir), bindings(Vec::new()))
        .expect("starts")
        .with_client(Box::new(client));
    let pauses = drive(&mut run, |pause| match pause {
        Pause::Budget { .. } => Some(Resume::Extend {
            extend: BTreeMap::from([(jevscript_ir::BudgetKey::Steps, 2.0)]),
        }),
        _ => None,
    });
    assert!(
        matches!(&pauses[0], Pause::Budget { common, used, limit, .. }
        if common.task == "machine:outer" && common.state.as_deref() == Some("outside")
            && *used == 1.0 && *limit == 1.0)
    );
    assert_eq!(requests.lock().expect("not poisoned").len(), 2);
    assert_eq!(
        field(output(pauses.last().expect("done"), "result"), "steps"),
        &Value::Number(1.0)
    );
}
