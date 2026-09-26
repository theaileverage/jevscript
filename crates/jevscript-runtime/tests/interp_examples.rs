//! The spec's example programs, compiled by the real compiler and run under
//! fake adapters (spec section 14; conformance items 2, 3 and 4).

mod support;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

use jevscript_compiler::{Resolver, compile_file};
use jevscript_ir::Ir;
use jevscript_runtime::jev::{JevError, JevRequest, JevResponse, JevUsage, Question};
use jevscript_runtime::record::Event;
use jevscript_runtime::run::{Run, RunOptions};
use jevscript_runtime::{JevClient, Pause, PauseKind, Resume, Value};
use support::*;

fn example(name: &str) -> Ir {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples")
        .join(name);
    compile_file(&path, &Resolver::default())
        .unwrap_or_else(|diagnostics| panic!("{name} does not compile: {diagnostics:?}"))
}

fn record(fields: &[(&str, &str)]) -> Value {
    Value::Record(
        fields
            .iter()
            .map(|(k, v)| ((*k).to_string(), Value::Text((*v).to_string())))
            .collect(),
    )
}

fn done_outputs(pause: &Pause) -> BTreeMap<String, Value> {
    match pause {
        Pause::Done { outputs, .. } => outputs.clone(),
        other => panic!("expected done, got {other:?}"),
    }
}

/* -------------------------------------------------------------------------- */
/* chief_of_staff.jev                                                          */
/* -------------------------------------------------------------------------- */

/// Answers the triage judgments from the event body, so the four events take
/// four different paths through the program.
struct TriageJev;

impl JevClient for TriageJev {
    fn send(&self, request: &JevRequest) -> Result<JevResponse, JevError> {
        let body = request.state["e"]["body"].as_str().unwrap_or_default();
        let (urgent, owner, confidence, risky) = if body.contains("bug") {
            (0.2, "code", 0.9, 0.1)
        } else if body.contains("pay") {
            (0.3, "code", 0.9, 0.9)
        } else if body.contains("meeting") {
            (0.9, "schedule", 0.3, 0.1)
        } else {
            (0.1, "customer", 0.8, 0.1)
        };
        let answers = Answers::default()
            .prob("urgent", urgent)
            .label("owner", owner, confidence)
            .prob("risky", risky);
        Ok(answers.respond(request))
    }
}

fn chief_of_staff_inputs() -> BTreeMap<String, Value> {
    BTreeMap::from([(
        "events".to_string(),
        Value::List(vec![
            record(&[
                ("subject", "Lexer bug"),
                ("body", "There is a bug in the lexer"),
                ("source", "github"),
            ]),
            record(&[
                ("subject", "Invoice"),
                ("body", "Please pay the invoice"),
                ("source", "mail"),
            ]),
            record(&[
                ("subject", "Sync"),
                ("body", "Can we move the meeting"),
                ("source", "mail"),
            ]),
            record(&[
                ("subject", "Login"),
                ("body", "I need help logging in"),
                ("source", "support"),
            ]),
        ]),
    )])
}

fn chief_of_staff_bindings(log: &CallLog) -> jevscript_runtime::capability::Bindings {
    let mux = FakeTool::new("mux", log)
        .verb("pane", vec![Value::Text("pane-1".into())])
        .verb("enqueue", vec![Value::None]);
    bindings(vec![
        ("claude", Box::new(FakeAgent::new("claude", log, vec![]))),
        ("writer", Box::new(FakeLlm::new("writer", log))),
        ("me", Box::new(FakePerson::new("me", log))),
        ("mux", Box::new(mux)),
    ])
}

#[test]
fn chief_of_staff_records_and_replays_with_identical_pauses_and_no_calls() {
    // Spec 14.3 under fakes; conformance item 4: the recording replays with
    // identical pauses in identical order, and neither Jev nor an adapter is
    // called during the replay.
    let dir = temp_dir("chief-of-staff");
    let log = call_log();
    let ir = example("chief_of_staff.jev");
    let recording = dir.join("run.jsonl");
    let options = RunOptions {
        inputs: chief_of_staff_inputs(),
        record: Some(recording.clone()),
        ..tiny_profile_options(&dir)
    };
    let mut run = Run::create(ir.clone(), "main", options, chief_of_staff_bindings(&log))
        .expect("starts")
        .with_client(Box::new(TriageJev));
    let pauses = drive(&mut run, |pause| match pause {
        Pause::Confirm { .. } => Some(Resume::Answer {
            answer: "no".into(),
            text: None,
        }),
        _ => None,
    });
    let kinds: Vec<PauseKind> = pauses.iter().map(Pause::kind).collect();
    assert_eq!(kinds, vec![PauseKind::Confirm, PauseKind::Done]);
    let Pause::Confirm {
        message, common, ..
    } = &pauses[0]
    else {
        unreachable!()
    };
    assert_eq!(message, "This looks risky: Invoice. Handle it yourself?");
    assert_eq!(common.task, "main");
    let outputs = done_outputs(&pauses[1]);
    assert_eq!(
        outputs["dispatched"],
        Value::List(vec![Value::Record(BTreeMap::from([
            ("subject".to_string(), Value::Text("Lexer bug".into())),
            ("to".to_string(), Value::Text("claude".into())),
        ]))])
    );
    let calls = logged(&log);
    assert_eq!(calls.len(), 6, "{calls:?}");
    assert!(calls[0].starts_with("mux.pane(cos-"), "{}", calls[0]);
    assert!(
        calls[1].starts_with("claude.spawn(in=pane-1, prompt=There is a bug"),
        "{}",
        calls[1]
    );
    assert_eq!(calls[2], "me.notify(Unsure who owns: Sync)");
    assert_eq!(calls[3], "me.notify(Urgent: Sync)");
    assert_eq!(
        calls[4],
        "writer.write(Draft a reply. Commit to nothing. No signature., using=I need help logging in)"
    );
    assert_eq!(
        calls[5],
        "me.notify(Draft for Login:\ndraft: I need help logging in)"
    );

    // Conformance items 2 and 3 from the recording: one request per event
    // carrying the three questions of the group, over exactly `e.body`.
    let events: Vec<Event> = run.drain_events().into_iter().map(|e| e.event).collect();
    let requests = events_of(&events, "request");
    assert_eq!(requests.len(), 4);
    for request in &requests {
        let Event::Request {
            state, questions, ..
        } = request
        else {
            unreachable!()
        };
        let mut ids: Vec<&str> = questions.iter().map(Question::id).collect();
        ids.sort_unstable();
        assert_eq!(ids, vec!["owner", "risky", "urgent"]);
        let state = state.full().expect("full");
        assert_eq!(state.keys().collect::<Vec<_>>(), vec!["e"]);
        assert_eq!(
            state["e"]
                .as_object()
                .expect("record")
                .keys()
                .collect::<Vec<_>>(),
            vec!["body"],
            "only the field the subjects reach"
        );
    }
    assert_eq!(run.usage().calls, 5, "four Jev requests and one generation");

    let options = RunOptions {
        inputs: chief_of_staff_inputs(),
        replay: Some(recording),
        ..tiny_profile_options(&dir)
    };
    let mut replay = Run::create(ir, "main", options, bindings(Vec::new()))
        .expect("a replay needs no bindings")
        .with_client(Box::new(NeverJev));
    let replayed = drive(&mut replay, |_| None);
    assert_eq!(replayed, pauses);
    assert_eq!(
        logged(&log).len(),
        6,
        "no adapter was called during the replay"
    );
    assert_eq!(replay.usage(), run.usage());
}

/* -------------------------------------------------------------------------- */
/* fix_issue.jev                                                               */
/* -------------------------------------------------------------------------- */

/// Answers `harness.read_agent` per iteration and the prelude's `stuck`
/// judgments the same way every time.
struct HarnessJev {
    iterations: Mutex<u32>,
}

impl JevClient for HarnessJev {
    fn send(&self, request: &JevRequest) -> Result<JevResponse, JevError> {
        let ids: Vec<&str> = request.questions.iter().map(Question::id).collect();
        let answers = if ids.contains(&"claims_done") {
            let mut n = self.iterations.lock().expect("not poisoned");
            *n += 1;
            let claims_done = [0.9, 0.5, 0.5][(*n as usize - 1).min(2)];
            Answers::default()
                .prob("claims_done", claims_done)
                .each("off_scope", &[0.1, 0.15])
                .label("next", "keep_working", 0.9)
        } else {
            Answers::default()
                .prob("same", 0.1)
                .level("progress", 2, 0.8)
        };
        let mut response = answers.respond(request);
        response.usage = JevUsage {
            tokens: 100,
            usd: None,
        };
        Ok(response)
    }
}

#[test]
fn fix_issue_runs_end_to_end_under_fakes() {
    // Spec 14.1a under fakes: the harness loop waits, shapes, judges, nudges
    // on a false claim of completion and ends when the tests pass; `main`
    // then opens the pull request.
    let dir = temp_dir("fix-issue");
    let log = call_log();
    let ir = example("fix_issue.jev");
    let inputs = BTreeMap::from([(
        "issue".to_string(),
        record(&[
            ("title", "Lexer panics on tabs"),
            ("body", "Fix the panic"),
            ("branch", "fix/tabs"),
        ]),
    )]);
    let options = RunOptions {
        inputs,
        ..tiny_profile_options(&dir)
    };
    let tree = FakeTool::new("tree", &log)
        .verb("create", vec![Value::None])
        .verb(
            "diff",
            vec![Value::Record(BTreeMap::from([(
                "files".to_string(),
                Value::List(vec![
                    Value::Text("src/lexer.rs".into()),
                    Value::Text("README.md".into()),
                ]),
            )]))],
        )
        .verb(
            "test_summary",
            vec![
                Value::Text("1 failed".into()),
                Value::Text("1 failed".into()),
                Value::Text("all passed".into()),
            ],
        )
        .verb(
            "tests_pass",
            vec![
                Value::Bool(false),
                Value::Bool(false),
                Value::Bool(false),
                Value::Bool(false),
                Value::Bool(true),
            ],
        )
        .verb(
            "open_pr",
            vec![Value::Text("https://example.com/pr/1".into())],
        );
    let agent = FakeAgent::new(
        "claude",
        &log,
        vec![
            observation("waiting", "Looking at the lexer", "$ cargo test"),
            observation("waiting", "Looking at the lexer", "$ cargo test"),
            observation("waiting", "Done, all tests pass", "$ cargo test"),
            observation("waiting", "Done, all tests pass", "$ cargo test"),
            observation("waiting", "Fixed the remaining test", "$ cargo test"),
            observation("waiting", "Fixed the remaining test", "$ cargo test"),
        ],
    );
    let mut run = Run::create(
        ir.clone(),
        "main",
        options,
        bindings(vec![
            ("claude", Box::new(agent)),
            ("tree", Box::new(tree)),
            ("me", Box::new(FakePerson::new("me", &log))),
        ]),
    )
    .expect("starts")
    .with_client(Box::new(HarnessJev {
        iterations: Mutex::new(0),
    }));
    let pauses = drive(&mut run, |_| None);
    let kinds: Vec<PauseKind> = pauses.iter().map(Pause::kind).collect();
    assert_eq!(
        kinds,
        vec![
            PauseKind::Waiting,
            PauseKind::Waiting,
            PauseKind::Waiting,
            PauseKind::Done
        ],
        "{pauses:?}"
    );
    let Pause::Waiting { on, common, .. } = &pauses[0] else {
        unreachable!()
    };
    assert_eq!(on, "claude");
    assert_eq!(
        common.task, "harness.watch",
        "a library task's pause carries its qualified name"
    );

    let Pause::Done {
        outputs,
        verified,
        usage,
        ..
    } = &pauses[3]
    else {
        unreachable!()
    };
    assert_eq!(
        outputs["pr_url"],
        Value::Text("https://example.com/pr/1".into())
    );
    assert!(
        !verified,
        "`main` declares no verify; the proof lives in `harness.watch`"
    );
    // Iteration one asks `read_agent` (one request: its four questions fit
    // the tiny profile's cap of four) and the prelude's `stuck` judgments.
    // From iteration two on, the trail holds `tests_pass` twice and `repeats`
    // (spec 8.1) returns early: the trail counts every capability call the
    // runtime made (spec 7.5), read-only tool queries included.
    assert_eq!(usage.calls, 4);
    assert_eq!(usage.tokens, 400);
    assert_eq!(usage.steps, 0, "`main` has no loop");
    assert!(usage.usd > 0.0);

    let calls = logged(&log);
    assert_eq!(calls[0], "tree.create(fix/tabs)");
    assert!(calls[1].starts_with("claude.spawn(in="), "{}", calls[1]);
    let sends: Vec<&String> = calls
        .iter()
        .filter(|c| c.starts_with("claude.send("))
        .collect();
    assert_eq!(sends.len(), 3, "{calls:?}");
    assert!(
        sends[0].contains("Tests fail:\n1 failed"),
        "the false claim of completion was nudged: {}",
        sends[0]
    );
    assert!(
        sends[1].contains("Stop. In three lines") && sends[2].contains("Stop. In three lines"),
        "`stuck` fired on the repeated tool query: {sends:?}"
    );
    assert!(calls.iter().any(|c| c.starts_with("claude.stop(")));
    assert_eq!(calls.last().map(String::as_str), Some("tree.open_pr()"));

    // The state of a `read_agent` request is every declared parameter in
    // full, including `tests`, `recent` and `title`, which no question
    // inspects (spec 6.9); `recent` is a list of runtime-written step
    // records (spec 7.5). The group is one request (spec 6.6).
    let events: Vec<Event> = run.drain_events().into_iter().map(|e| e.event).collect();
    let requests = events_of(&events, "request");
    assert_eq!(requests.len(), 4);
    let Event::Request {
        state, questions, ..
    } = requests[0]
    else {
        unreachable!()
    };
    let state = state.full().expect("full");
    assert_eq!(
        state.keys().collect::<Vec<_>>(),
        vec!["files", "recent", "summary", "tests", "title"]
    );
    assert_eq!(state["title"], "Lexer panics on tabs");
    assert_eq!(state["tests"], "1 failed");
    let recent = state["recent"].as_array().expect("a list");
    assert_eq!(recent[0]["action"], "tests_pass");
    assert_eq!(
        questions.len(),
        4,
        "claims_done, off_scope[0], off_scope[1], next in one request"
    );
    let Event::Request { state, .. } = requests[1] else {
        unreachable!()
    };
    assert_eq!(
        state.full().expect("full").keys().collect::<Vec<_>>(),
        vec!["steps"]
    );
    // A trail is per task (spec 7.5): `main`'s `create` and `spawn` are
    // never followed by an observation in `main`, so its records stay
    // pending; the first records written are `harness.watch`'s own, settled
    // by its `wait`.
    let steps = events_of(&events, "step");
    assert!(steps.len() >= 3, "{steps:?}");
    assert!(
        matches!(steps[0], Event::Step { record } if record.action == "tests_pass" && record.target == "tree" && record.step == 1 && record.changed),
        "{:?}",
        steps[0]
    );
    assert!(
        matches!(steps[1], Event::Step { record } if record.action == "diff" && record.step == 2)
    );
}

/* -------------------------------------------------------------------------- */
/* review_loop.jev                                                             */
/* -------------------------------------------------------------------------- */

/// Keeps the review machine in `working` for two observations, then chooses
/// the two guarded transitions that lead to approval.
struct ReviewMachineJev {
    working_steps: Mutex<u32>,
}

impl JevClient for ReviewMachineJev {
    fn send(&self, request: &JevRequest) -> Result<JevResponse, JevError> {
        let state = request.state["state"].as_str().unwrap_or_default();
        let label = match state {
            "working" => {
                let mut steps = self.working_steps.lock().expect("not poisoned");
                *steps += 1;
                if *steps < 3 { "stay" } else { "finished" }
            }
            "reviewing" => "approved",
            other => panic!("unexpected machine state {other}"),
        };
        let Question::Choice { labels, .. } = &request.questions[0] else {
            panic!("machine request is a Choice")
        };
        assert!(labels.iter().any(|candidate| candidate.name == label));
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
            answers: vec![jevscript_runtime::jev::JevAnswer::Choice {
                id: "event".into(),
                label: label.into(),
                confidence: 0.9,
                probabilities,
            }],
            usage: JevUsage::default(),
            latency_ms: 0,
        })
    }
}

#[test]
fn review_machine_runs_to_guarded_approval_and_replays_without_live_calls() {
    // Spec 14.4 and conformance 4/6: guards expose only legal events, the
    // scripted observations flip on step three, approval is verified, and a
    // recording reproduces identical pauses without Jev or adapter calls.
    let dir = temp_dir("review-machine");
    let recording = dir.join("run.jsonl");
    let ir = example("review_loop.jev");
    let log = call_log();
    let agent = FakeAgent::new(
        "claude",
        &log,
        vec![observation("running", "working", "tail")],
    );
    let tree = FakeTool::new("tree", &log)
        .verb(
            "tests_pass",
            vec![
                Value::Bool(false),
                Value::Bool(false),
                Value::Bool(false),
                Value::Bool(false),
                Value::Bool(true),
                Value::Bool(true),
                Value::Bool(true),
            ],
        )
        .verb(
            "test_summary",
            vec![
                Value::Text("failing".into()),
                Value::Text("failing".into()),
                Value::Text("passing".into()),
                Value::Text("passing".into()),
            ],
        );
    let mut options = tiny_profile_options(&dir);
    options.record = Some(recording.clone());
    let mut run = Run::create(
        ir.clone(),
        "main",
        options,
        bindings(vec![
            ("claude", Box::new(agent)),
            ("tree", Box::new(tree)),
            ("me", Box::new(FakePerson::new("me", &log))),
        ]),
    )
    .expect("starts")
    .with_client(Box::new(ReviewMachineJev {
        working_steps: Mutex::new(0),
    }));
    let pauses = drive(&mut run, |_| None);
    assert!(matches!(&pauses[..], [Pause::Done { .. }]));
    let events: Vec<Event> = run
        .drain_events()
        .into_iter()
        .map(|event| event.event)
        .collect();
    let steps = events_of(&events, "machine_step");
    assert_eq!(steps.len(), 4);
    assert!(
        matches!(steps.last(), Some(Event::MachineStep { state, chosen, to, .. })
        if state == "reviewing" && chosen == "approved" && to == "approved")
    );
    let live_calls = logged(&log);
    assert!(
        live_calls
            .last()
            .is_some_and(|call| call == "me.notify(Ready: 4 steps)")
    );

    let mut replay_options = tiny_profile_options(&dir);
    replay_options.replay = Some(recording);
    let mut replay = Run::create(ir, "main", replay_options, bindings(Vec::new()))
        .expect("replay starts")
        .with_client(Box::new(NeverJev));
    let replayed = drive(&mut replay, |_| None);
    assert_eq!(replayed, pauses);
    assert_eq!(
        logged(&log),
        live_calls,
        "replay made zero live adapter calls"
    );
    assert_eq!(replay.usage(), run.usage());
}
