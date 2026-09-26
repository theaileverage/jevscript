//! The run state machine, through the public API only (spec section 10.1).

use std::collections::{BTreeMap, BTreeSet};

use jevscript_ir::{Budget, Ir, Task, Thresholds};
use jevscript_runtime::pause::Resume;
use jevscript_runtime::run::{Run, RunOptions, RunState};
use jevscript_runtime::{PauseKind, RunError};

fn program() -> Ir {
    let mut ir = Ir::empty("inbox_triage");
    ir.tasks.push(Task {
        name: "main".to_string(),
        params: Vec::new(),
        budget: Budget::default(),
        thresholds: Thresholds::default(),
        body: Vec::new(),
        span: Default::default(),
    });
    ir
}

fn run() -> Run {
    Run::create_with_bound_names(program(), "main", RunOptions::default(), &BTreeSet::new())
        .expect("starts")
}

#[test]
fn a_run_starts_created_and_ends_stopped_on_abort() {
    let mut run = run();
    assert_eq!(*run.state(), RunState::Created);
    assert_eq!(run.step(), 0);
    run.abort().expect("a created run can be aborted");
    assert_eq!(*run.state(), RunState::Ended(PauseKind::Stopped));
    assert!(matches!(run.abort(), Err(RunError::Ended(_))));
}

#[test]
fn a_host_cannot_resume_a_run_it_has_not_paused() {
    let mut run = run();
    assert_eq!(
        run.resume(Resume::Answer {
            answer: "yes".to_string(),
            text: None
        }),
        Err(RunError::NotPaused)
    );
}

#[test]
fn a_host_cannot_inject_outside_a_waiting_pause() {
    let mut run = run();
    assert_eq!(run.inject("claude", "hello"), Err(RunError::NotPaused));
}

#[test]
fn an_empty_task_runs_to_done_and_records_start_and_end() {
    // Spec 10.1 and 10.3: a run is stepped to its first pause, and a
    // recording opens with `start` and closes with `end`.
    let mut run = run();
    let pause = run.next().expect("steps to done");
    assert_eq!(pause.kind(), PauseKind::Done);
    assert_eq!(*run.state(), RunState::Ended(PauseKind::Done));
    let events: Vec<String> = run
        .drain_events()
        .into_iter()
        .map(|e| serde_json::to_value(&e.event).expect("serializes")["event"].to_string())
        .collect();
    assert_eq!(events.first().map(String::as_str), Some("\"start\""));
    assert_eq!(events.last().map(String::as_str), Some("\"end\""));
    assert!(matches!(run.next(), Err(RunError::Ended("done"))));
}

#[test]
fn a_machine_pause_names_the_machine_and_its_state() {
    // Spec 7.8: pauses raised inside a machine carry `machine:<qualified name>`
    // as their task and the state they were raised in.
    let pause: jevscript_runtime::Pause = serde_json::from_str(
        r#"{
            "kind": "escalate",
            "run_id": "run_1",
            "step": 4,
            "task": "machine:review",
            "state": "working",
            "source": {
                "start": {"line": 1, "column": 0, "offset": 0},
                "end": {"line": 1, "column": 0, "offset": 0}
            },
            "recording_offset": 0,
            "reason": "no_enabled_events",
            "context": {}
        }"#,
    )
    .expect("a machine escalate deserializes");
    assert_eq!(pause.kind(), PauseKind::Escalate);
    assert_eq!(pause.common().task, "machine:review");
    assert_eq!(pause.common().state.as_deref(), Some("working"));
}

#[test]
fn a_run_selects_a_model_profile() {
    // Spec 10.6: limits come from a profile, and an unknown model id cannot
    // start a run.
    let profiles = jevscript_runtime::Profiles::bundled();
    let profile = profiles
        .resolve("jev-latest")
        .expect("the default model has a profile");
    assert!(
        profile.max_criteria_per_question >= 2,
        "a `pick among` needs a cap to check"
    );
    assert!(profiles.resolve("jev-nope").is_err());
}

#[test]
fn usage_starts_empty() {
    let run = run();
    assert_eq!(run.usage().calls, 0);
    assert!(run.current_pause().is_none());
    assert!(run.options().inputs.is_empty());
    assert_eq!(run.ir().program, "inbox_triage");
    assert_eq!(run.task(), "main");
    assert!(run.id().starts_with("run_"));
    let _: BTreeMap<String, jevscript_runtime::Value> = run.options().inputs.clone();
}
