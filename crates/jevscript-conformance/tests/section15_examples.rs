//! End-to-end acceptance of the executable section 14 examples (spec section 15).

mod support;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use jevscript_compiler::{Resolver, compile_file};
use jevscript_conformance::{
    check_recording_with_profile, log_sequence, logs_diverge_at, pause_sequence,
};
use jevscript_ir::Ir;
use jevscript_runtime::capability::CapabilityKind;
use jevscript_runtime::jev::{JevClient, JevError, JevRequest, JevResponse, Question};
use jevscript_runtime::judge::run_judgment;
use jevscript_runtime::record::{Event, RecordedEvent};
use jevscript_runtime::run::{Run, RunOptions};
use jevscript_runtime::{Pause, PauseKind, Resume, Value};
use serde_json::json;
use support::*;

fn example_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples")
        .join(name)
}

fn example(name: &str) -> Ir {
    compile_file(&example_path(name), &Resolver::default())
        .unwrap_or_else(|diagnostics| panic!("{name} did not compile: {diagnostics:?}"))
}

fn read_recording(path: &Path) -> Vec<RecordedEvent> {
    std::fs::read_to_string(path)
        .expect("recording can be read")
        .lines()
        .map(|line| serde_json::from_str(line).expect("recording line is valid JSONL"))
        .collect()
}

fn assert_full_and_conformant(ir: &Ir, run: &Run, events: &[RecordedEvent]) {
    assert!(
        events
            .iter()
            .any(|event| matches!(event.event, Event::End { .. })),
        "a complete live recording has an end event"
    );
    for event in events {
        match &event.event {
            Event::Start { .. } => assert!(
                event.execution.is_some(),
                "start must embed self-contained execution metadata"
            ),
            Event::Request { state, .. } => {
                assert!(
                    state.full().is_some(),
                    "request state must be recorded in full"
                )
            }
            Event::Observe { observation, .. } => assert!(
                observation.full().is_some(),
                "observations must be recorded in full"
            ),
            Event::Call { result, .. } => {
                assert!(
                    result.full().is_some(),
                    "call results must be recorded in full"
                )
            }
            _ => {}
        }
    }
    let violations = check_recording_with_profile(ir, events, run.profile());
    assert!(
        violations.is_empty(),
        "conformance violations: {violations:#?}"
    );
}

fn assert_recorded_pauses(events: &[RecordedEvent], surfaced: &[Pause]) {
    let recorded: Vec<Pause> = pause_sequence(events)
        .into_iter()
        .map(|pause| pause.payload)
        .collect();
    assert_same_pauses(&recorded, surfaced, "recorded pauses");
}

fn assert_same_pauses(actual: &[Pause], expected: &[Pause], context: &str) {
    assert_eq!(
        actual, expected,
        "{context} must preserve every surfaced pause payload in order"
    );
}

fn replay_with_panics(
    ir: Ir,
    inputs: BTreeMap<String, Value>,
    recording: PathBuf,
    dir: &Path,
    needs: &[(&str, CapabilityKind)],
) -> Vec<Pause> {
    let options = RunOptions {
        inputs,
        replay: Some(recording),
        ..options(dir)
    };
    let adapters = needs
        .iter()
        .map(|(name, kind)| {
            (
                *name,
                Box::new(NeverCapability(*kind)) as Box<dyn jevscript_runtime::Capability>,
            )
        })
        .collect();
    let mut run = Run::create(ir, "main", options, bindings(adapters))
        .expect("replay starts")
        .with_client(Box::new(NeverJev));
    drive(&mut run, |_| None)
}

fn replay_standalone(recording: PathBuf) -> Vec<Pause> {
    let mut run = Run::from_recording(recording)
        .expect("self-contained recording starts without source or ambient profiles")
        .with_client(Box::new(NeverJev));
    drive(&mut run, |_| None)
}

#[test]
fn item_1_every_section_14_fixture_compiles_from_disk() {
    // Spec 15.1 includes libraries through the runnable importer, and all five
    // root programs compile independently from their verbatim example files.
    for name in [
        "fix_issue_inline.jev",
        "fix_issue.jev",
        "inbox_triage.jev",
        "chief_of_staff.jev",
        "review_loop.jev",
    ] {
        let ir = example(name);
        assert!(!ir.program.is_empty(), "{name}");
    }
    let library = example("lib/agent_loop.jev");
    assert!(library.task("watch").is_some());
}

#[test]
fn inbox_triage_runs_as_one_named_judgment_request() {
    // Spec 14.2 and 11.3: a judgment-only program executes directly, without
    // inventing a task or recording (the standalone judgment API records none).
    let ir = example("inbox_triage.jev");
    let state = BTreeMap::from([(
        "message".to_string(),
        json!("Please fix this production bug before the next hour"),
    )]);
    let answers = Answers::default()
        .prob("urgent", 0.95)
        .label("owner", "code", 0.9)
        .prob("risky", 0.7)
        .level("effort", 1, 0.85);
    let outcome = run_judgment(&ir, "triage", &state, &answers, &tiny_profile())
        .expect("standalone judgment runs");

    assert_eq!(outcome.requests.len(), 1, "the judgment is one batch");
    let request = &outcome.requests[0];
    assert_eq!(
        request.state, state,
        "state is exactly the declared parameter"
    );
    assert_eq!(
        request
            .questions
            .iter()
            .map(Question::id)
            .collect::<Vec<_>>(),
        vec!["urgent", "owner", "risky", "effort"]
    );
    assert_eq!(outcome.values["urgent"], Value::Prob(0.95));
    assert_eq!(outcome.values["owner"].to_text(), "code");
    assert_eq!(outcome.values["effort"].to_text(), "1");
}

struct HarnessJev;

impl JevClient for HarnessJev {
    fn send(&self, request: &JevRequest) -> Result<JevResponse, JevError> {
        let ids: Vec<&str> = request.questions.iter().map(Question::id).collect();
        let answers = if ids.contains(&"claims_done") {
            Answers::default()
                .prob("claims_done", 0.1)
                .each("off_scope", &[0.1, 0.1])
                .label("next", "keep_working", 0.9)
        } else {
            Answers::default()
                .prob("same", 0.1)
                .level("progress", 2, 0.9)
        };
        Ok(answers.response(request))
    }
}

fn issue_inputs() -> BTreeMap<String, Value> {
    BTreeMap::from([(
        "issue".to_string(),
        record(&[
            ("title", "Lexer panics on tabs"),
            ("body", "Fix the panic"),
            ("branch", "fix/tabs"),
        ]),
    )])
}

fn harness_bindings(log: &CallLog) -> jevscript_runtime::capability::Bindings {
    let tree = FakeTool::new("tree", log)
        .verb("create", vec![Value::None])
        .verb(
            "diff",
            vec![Value::Record(BTreeMap::from([(
                "files".to_string(),
                Value::List(vec![
                    Value::Text("src/lexer.rs".to_string()),
                    Value::Text("tests/lexer.rs".to_string()),
                ]),
            )]))],
        )
        .verb("test_summary", vec![Value::Text("all passed".to_string())])
        .verb(
            "tests_pass",
            vec![Value::Bool(false), Value::Bool(true), Value::Bool(true)],
        )
        .verb(
            "open_pr",
            vec![Value::Text("https://example.test/pr/1".to_string())],
        );
    let agent = FakeAgent::new(
        "claude",
        log,
        vec![
            observation("waiting", "Working on the lexer"),
            observation("waiting", "Tests are now passing"),
        ],
    );
    bindings(vec![
        ("claude", Box::new(agent)),
        ("tree", Box::new(tree)),
        ("me", Box::new(FakePerson::new("me", log))),
    ])
}

fn assert_harness_example(name: &str, waiting_task: &str) {
    let dir = temp_dir(name);
    let recording = dir.join("run.jsonl");
    let inputs = issue_inputs();
    let ir = example(name);
    let log = call_log();
    let run_options = RunOptions {
        inputs: inputs.clone(),
        record: Some(recording.clone()),
        ..options(&dir)
    };
    let mut run = Run::create(ir.clone(), "main", run_options, harness_bindings(&log))
        .expect("harness starts")
        .with_client(Box::new(HarnessJev));
    let surfaced = drive(&mut run, |_| None);

    assert_eq!(
        surfaced.iter().map(Pause::kind).collect::<Vec<_>>(),
        vec![PauseKind::Waiting, PauseKind::Done]
    );
    assert_eq!(surfaced[0].common().task, waiting_task);
    let Pause::Done { outputs, .. } = &surfaced[1] else {
        unreachable!()
    };
    assert_eq!(
        outputs["pr_url"],
        Value::Text("https://example.test/pr/1".to_string())
    );
    assert!(
        logged(&log)
            .iter()
            .any(|call| call.starts_with("claude.stop("))
    );

    let events = read_recording(&recording);
    assert_full_and_conformant(&ir, &run, &events);
    assert_recorded_pauses(&events, &surfaced);
    let requests: Vec<_> = events
        .iter()
        .filter_map(|event| match &event.event {
            Event::Request {
                state, questions, ..
            } => Some((state.full().expect("full state"), questions)),
            _ => None,
        })
        .collect();
    assert_eq!(requests.len(), 2, "named read plus prelude stuck batches");
    assert_eq!(
        requests[0].1.iter().map(Question::id).collect::<Vec<_>>(),
        vec!["claims_done", "off_scope[0]", "off_scope[1]", "next"]
    );
    let expected_named_state = if name == "fix_issue.jev" {
        vec!["files", "recent", "summary", "tests", "title"]
    } else {
        vec!["files", "recent", "summary", "tests"]
    };
    assert_eq!(
        requests[0].0.keys().map(String::as_str).collect::<Vec<_>>(),
        expected_named_state,
        "named judgments carry every declared parameter in full"
    );
    assert_eq!(
        requests[1].1.iter().map(Question::id).collect::<Vec<_>>(),
        vec!["same", "progress"]
    );
    assert_eq!(
        requests[1].0.keys().map(String::as_str).collect::<Vec<_>>(),
        vec!["steps"],
        "the prelude def sends only its inline subject root"
    );
    let replayed = replay_with_panics(
        ir.clone(),
        inputs,
        recording.clone(),
        &dir,
        &[
            ("claude", CapabilityKind::Agent),
            ("tree", CapabilityKind::Tool),
            ("me", CapabilityKind::Person),
        ],
    );
    assert_same_pauses(&replayed, &surfaced, "panic-bound replay");
    assert_same_pauses(
        &replay_standalone(recording),
        &surfaced,
        "standalone replay",
    );
}

#[test]
fn coding_harness_inline_is_recorded_checked_and_replayed() {
    assert_harness_example("fix_issue_inline.jev", "main");
}

#[test]
fn coding_harness_library_is_recorded_checked_and_replayed() {
    // This executes the section 14.1a library through its runnable importer,
    // proving the qualified task boundary rather than treating it as a fixture.
    assert_harness_example("fix_issue.jev", "harness.watch");
}

struct TriageJev;

impl JevClient for TriageJev {
    fn send(&self, request: &JevRequest) -> Result<JevResponse, JevError> {
        let body = request.state["e"]["body"].as_str().unwrap_or_default();
        let answers = if body.contains("bug") {
            Answers::default()
                .prob("urgent", 0.2)
                .label("owner", "code", 0.9)
                .prob("risky", 0.1)
        } else if body.contains("pay") {
            Answers::default()
                .prob("urgent", 0.2)
                .label("owner", "code", 0.9)
                .prob("risky", 0.9)
        } else if body.contains("meeting") {
            Answers::default()
                .prob("urgent", 0.9)
                .label("owner", "schedule", 0.3)
                .prob("risky", 0.1)
        } else {
            Answers::default()
                .prob("urgent", 0.1)
                .label("owner", "customer", 0.9)
                .prob("risky", 0.1)
        };
        Ok(answers.response(request))
    }
}

fn dispatcher_inputs() -> BTreeMap<String, Value> {
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

#[test]
fn chief_of_staff_is_recorded_checked_and_replayed() {
    // Spec 14.3: each loop iteration is one three-question request over only
    // `e.body`; the four scripted events also exercise each gate outcome arm.
    let dir = temp_dir("chief-of-staff");
    let recording = dir.join("run.jsonl");
    let inputs = dispatcher_inputs();
    let ir = example("chief_of_staff.jev");
    let log = call_log();
    let mux = FakeTool::new("mux", &log)
        .verb("pane", vec![Value::Text("pane-1".to_string())])
        .verb("enqueue", vec![Value::None]);
    let run_options = RunOptions {
        inputs: inputs.clone(),
        record: Some(recording.clone()),
        ..options(&dir)
    };
    let mut run = Run::create(
        ir.clone(),
        "main",
        run_options,
        bindings(vec![
            (
                "claude",
                Box::new(FakeAgent::new("claude", &log, Vec::new())),
            ),
            ("writer", Box::new(FakeLlm::new("writer", &log))),
            ("me", Box::new(FakePerson::new("me", &log))),
            ("mux", Box::new(mux)),
        ]),
    )
    .expect("dispatcher starts")
    .with_client(Box::new(TriageJev));
    let surfaced = drive(&mut run, |pause| match pause {
        Pause::Confirm { .. } => Some(Resume::Answer {
            answer: "no".to_string(),
            text: None,
        }),
        _ => None,
    });
    assert_eq!(
        surfaced.iter().map(Pause::kind).collect::<Vec<_>>(),
        vec![PauseKind::Confirm, PauseKind::Done]
    );
    let Pause::Confirm { message, .. } = &surfaced[0] else {
        unreachable!()
    };
    assert_eq!(message, "This looks risky: Invoice. Handle it yourself?");

    let events = read_recording(&recording);
    assert_full_and_conformant(&ir, &run, &events);
    assert_recorded_pauses(&events, &surfaced);
    let requests: Vec<_> = events
        .iter()
        .filter_map(|event| match &event.event {
            Event::Request {
                state, questions, ..
            } => Some((state.full().expect("full state"), questions)),
            _ => None,
        })
        .collect();
    assert_eq!(requests.len(), 4);
    for (state, questions) in requests {
        assert_eq!(questions.len(), 3);
        assert_eq!(state.keys().collect::<Vec<_>>(), vec!["e"]);
        assert_eq!(
            state["e"]
                .as_object()
                .expect("e is a record")
                .keys()
                .collect::<Vec<_>>(),
            vec!["body"]
        );
    }

    let replayed = replay_with_panics(
        ir.clone(),
        inputs,
        recording.clone(),
        &dir,
        &[
            ("claude", CapabilityKind::Agent),
            ("writer", CapabilityKind::Llm),
            ("me", CapabilityKind::Person),
            ("mux", CapabilityKind::Tool),
        ],
    );
    assert_same_pauses(&replayed, &surfaced, "panic-bound replay");
    assert_same_pauses(
        &replay_standalone(recording.clone()),
        &surfaced,
        "standalone replay",
    );

    // Item 9 (spec 5.8): one `routed` log per event, outside every request,
    // reproduced exactly by a replay that does not stream them again.
    let logged: Vec<String> = log_sequence(&events)
        .into_iter()
        .map(|event| match event {
            Event::Log { message, .. } => message.full().cloned().expect("full"),
            other => panic!("not a log: {other:?}"),
        })
        .collect();
    assert_eq!(
        logged,
        vec![
            "routed Lexer bug",
            "routed Invoice",
            "routed Sync",
            "routed Login"
        ]
    );
    let mut replay = Run::from_recording(recording)
        .expect("replay starts")
        .with_client(Box::new(NeverJev));
    drive(&mut replay, |_| None);
    let streamed = replay.drain_events();
    assert!(
        streamed
            .iter()
            .all(|line| !matches!(line.event, Event::Log { .. })),
        "a replay does not re-emit logs to the host"
    );
    let replayed_log: Vec<RecordedEvent> = replay
        .log()
        .iter()
        .enumerate()
        .map(|(seq, event)| RecordedEvent {
            ts: String::new(),
            run_id: String::new(),
            seq: seq as u64,
            execution: None,
            event: event.clone(),
        })
        .collect();
    assert_eq!(logs_diverge_at(&events, &replayed_log), None);
}

struct ReviewJev;

impl JevClient for ReviewJev {
    fn send(&self, request: &JevRequest) -> Result<JevResponse, JevError> {
        let state = request.state["state"].as_str().expect("machine state text");
        let recent = request.state["recent"]
            .as_array()
            .expect("machine recent list");
        let label = match (state, recent.len()) {
            ("working", 0) => "claims_done",
            ("nudging", _) => "resumed",
            ("working", _) => "finished",
            ("reviewing", _) => "approved",
            other => panic!("unexpected review machine request: {other:?}"),
        };
        Ok(Answers::default()
            .label("event", label, 0.9)
            .response(request))
    }
}

#[test]
fn review_loop_machine_is_recorded_checked_and_replayed() {
    // Spec 14.4 and 15.6: code guards filter each exact menu, Jev chooses only
    // among that menu, and the guarded terminal entry drives the verified path.
    let dir = temp_dir("review-loop");
    let recording = dir.join("run.jsonl");
    let ir = example("review_loop.jev");
    let log = call_log();
    let tree = FakeTool::new("tree", &log)
        .verb(
            "tests_pass",
            vec![
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
                Value::Text("one failing test".to_string()),
                Value::Text("test running".to_string()),
                Value::Text("all tests pass".to_string()),
                Value::Text("all tests pass".to_string()),
            ],
        );
    let agent = FakeAgent::new(
        "claude",
        &log,
        vec![
            observation("waiting", "Done, but a test still fails"),
            observation("waiting", "Done, but a test still fails"),
            observation("running", "Addressing the failing test"),
            observation("running", "Addressing the failing test"),
            observation("waiting", "Done and tests pass"),
            observation("waiting", "Done and tests pass"),
            observation("waiting", "Ready for review"),
            observation("waiting", "Ready for review"),
        ],
    );
    let run_options = RunOptions {
        record: Some(recording.clone()),
        ..options(&dir)
    };
    let mut run = Run::create(
        ir.clone(),
        "main",
        run_options,
        bindings(vec![
            ("claude", Box::new(agent)),
            ("tree", Box::new(tree)),
            ("me", Box::new(FakePerson::new("me", &log))),
        ]),
    )
    .expect("review machine starts")
    .with_client(Box::new(ReviewJev));
    let surfaced = drive(&mut run, |_| None);
    assert_eq!(
        surfaced.iter().map(Pause::kind).collect::<Vec<_>>(),
        vec![PauseKind::Done]
    );
    assert!(
        logged(&log)
            .iter()
            .any(|call| call == "me.notify(Ready: 4 steps)"),
        "the guarded terminal transition must produce a verified result"
    );

    let events = read_recording(&recording);
    assert_full_and_conformant(&ir, &run, &events);
    assert_recorded_pauses(&events, &surfaced);
    let requests: Vec<_> = events
        .iter()
        .filter_map(|event| match &event.event {
            Event::Request {
                state, questions, ..
            } => Some((state.full().expect("full state"), questions)),
            _ => None,
        })
        .collect();
    assert_eq!(requests.len(), 4, "one Choice request per machine step");
    let expected_menus = [
        vec!["claims_done", "stuck", "asks", "stay"],
        vec!["resumed", "stay"],
        vec!["finished", "stuck", "asks", "stay"],
        vec!["approved", "rejected", "stay"],
    ];
    for ((state, questions), expected_menu) in requests.iter().zip(&expected_menus) {
        assert_eq!(
            state.keys().map(String::as_str).collect::<Vec<_>>(),
            vec!["goal", "obs", "recent", "state"]
        );
        assert_eq!(
            state["obs"]
                .as_object()
                .expect("machine observation record")
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            vec!["status", "summary", "tests"]
        );
        let [
            Question::Choice {
                id,
                path,
                labels,
                instruction,
                ..
            },
        ] = questions.as_slice()
        else {
            panic!("machine request is one Choice")
        };
        assert_eq!(id, "event");
        assert_eq!(path, "obs");
        assert_eq!(
            labels
                .iter()
                .map(|label| label.name.as_str())
                .collect::<Vec<_>>(),
            *expected_menu
        );
        assert_eq!(
            instruction
                .as_ref()
                .expect("machine compare instruction")
                .compare,
            ["state", "goal", "recent"]
        );
    }

    let steps: Vec<_> = events
        .iter()
        .filter_map(|event| match &event.event {
            Event::MachineStep {
                state,
                enabled,
                chosen,
                to,
                ..
            } => Some((state.clone(), enabled.clone(), chosen.clone(), to.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(
        steps,
        vec![
            (
                "working".to_string(),
                vec![
                    "claims_done".to_string(),
                    "stuck".to_string(),
                    "asks".to_string(),
                    "stay".to_string()
                ],
                "claims_done".to_string(),
                "nudging".to_string()
            ),
            (
                "nudging".to_string(),
                vec!["resumed".to_string(), "stay".to_string()],
                "resumed".to_string(),
                "working".to_string()
            ),
            (
                "working".to_string(),
                vec![
                    "finished".to_string(),
                    "stuck".to_string(),
                    "asks".to_string(),
                    "stay".to_string()
                ],
                "finished".to_string(),
                "reviewing".to_string()
            ),
            (
                "reviewing".to_string(),
                vec![
                    "approved".to_string(),
                    "rejected".to_string(),
                    "stay".to_string()
                ],
                "approved".to_string(),
                "approved".to_string()
            ),
        ]
    );

    let replayed = replay_with_panics(
        ir.clone(),
        BTreeMap::new(),
        recording.clone(),
        &dir,
        &[
            ("claude", CapabilityKind::Agent),
            ("tree", CapabilityKind::Tool),
            ("me", CapabilityKind::Person),
        ],
    );
    assert_same_pauses(&replayed, &surfaced, "panic-bound machine replay");
    assert_same_pauses(
        &replay_standalone(recording),
        &surfaced,
        "standalone machine replay",
    );
}
