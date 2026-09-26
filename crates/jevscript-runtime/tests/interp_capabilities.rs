//! Capability verbs, the pauses they raise, the trail, replay of a live run
//! and `inject` (spec sections 7.5, 9, 10.2 and 10.4).

mod support;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use jevscript_ir::{BinaryOp, BudgetKey, CapabilityKind};
use jevscript_runtime::capability::{
    CallArgs, Capability, CapabilityKind as RuntimeCapabilityKind, Observation,
};
use jevscript_runtime::record::{EffectIdentity, EffectOperation, Event, StepOutcome};
use jevscript_runtime::run::{Run, RunOptions};
use jevscript_runtime::{
    Handle, Pause, PauseKind, Resume, RunError, RuntimeError, RuntimeErrorCode, Value,
};
use support::*;

fn done_outputs(pause: &Pause) -> BTreeMap<String, Value> {
    match pause {
        Pause::Done { outputs, .. } => outputs.clone(),
        other => panic!("expected done, got {other:?}"),
    }
}

fn events(run: &mut Run) -> Vec<Event> {
    run.drain_events().into_iter().map(|e| e.event).collect()
}

fn retry_once_to_done(run: &mut Run) -> Pause {
    let error = run.next().expect("surfaces external failure");
    assert!(matches!(
        error,
        Pause::Error {
            retryable: true,
            ..
        }
    ));
    run.resume(Resume::Retry { retry: true })
        .expect("retries failed effect");
    assert!(matches!(
        run.next().expect("retry completes"),
        Pause::Done { .. }
    ));
    error
}

fn truncate_before_event(path: &std::path::Path, event: &str) {
    let text = std::fs::read_to_string(path).expect("reads recording");
    let mut kept = Vec::new();
    for line in text.lines() {
        let value: serde_json::Value = serde_json::from_str(line).expect("valid event");
        if value["event"] == event {
            break;
        }
        kept.push(line);
    }
    assert!(
        kept.len() < text.lines().count(),
        "recording had no {event}"
    );
    std::fs::write(path, kept.join("\n") + "\n").expect("truncates recording");
}

struct NeverTool;

impl jevscript_runtime::capability::Capability for NeverTool {
    fn kind(&self) -> jevscript_runtime::capability::CapabilityKind {
        jevscript_runtime::capability::CapabilityKind::Tool
    }

    fn call(
        &mut self,
        verb: &str,
        _args: &jevscript_runtime::capability::CallArgs,
    ) -> Result<Value, jevscript_runtime::RuntimeError> {
        panic!("truncated replay reached tool verb {verb}")
    }
}

struct NeverCapability(RuntimeCapabilityKind);

impl Capability for NeverCapability {
    fn kind(&self) -> RuntimeCapabilityKind {
        self.0
    }

    fn call(&mut self, verb: &str, _args: &CallArgs) -> Result<Value, RuntimeError> {
        panic!("replay reached {:?} capability verb {verb}", self.0)
    }

    fn observe(&mut self, handle: &Handle) -> Result<Observation, RuntimeError> {
        panic!("replay observed live handle {}", handle.id)
    }
}

struct FlakyObserve {
    failed: bool,
}

impl Capability for FlakyObserve {
    fn kind(&self) -> RuntimeCapabilityKind {
        RuntimeCapabilityKind::Agent
    }

    fn call(&mut self, verb: &str, _args: &CallArgs) -> Result<Value, RuntimeError> {
        assert_eq!(verb, "spawn");
        Ok(Value::Handle(Handle {
            capability: "claude".into(),
            id: "agent-1".into(),
            fields: BTreeMap::new(),
        }))
    }

    fn observe(&mut self, _handle: &Handle) -> Result<Observation, RuntimeError> {
        if !self.failed {
            self.failed = true;
            return Err(
                RuntimeError::new(RuntimeErrorCode::AdapterError, "observe flaked").retryable(true),
            );
        }
        Ok(observation("exited", "done", ""))
    }
}

struct FlakyGenerate {
    failed: bool,
}

impl Capability for FlakyGenerate {
    fn kind(&self) -> RuntimeCapabilityKind {
        RuntimeCapabilityKind::Llm
    }

    fn call(&mut self, verb: &str, _args: &CallArgs) -> Result<Value, RuntimeError> {
        assert_eq!(verb, "write");
        if !self.failed {
            self.failed = true;
            return Err(
                RuntimeError::new(RuntimeErrorCode::AdapterError, "generate flaked")
                    .retryable(true),
            );
        }
        Ok(Value::Text("summary".into()))
    }
}

#[test]
fn person_ask_pauses_with_confirm_and_returns_a_pause_result() {
    // Spec 9.2 and 10.2: `person.ask` pauses with `confirm`; the host's
    // `{ answer, text }` becomes a `pause_result`.
    let log = call_log();
    let ir = program(
        vec![need("me", CapabilityKind::Person)],
        &["answer", "note"],
        vec![
            on_line(
                assign(
                    "r",
                    verb_named(
                        "me",
                        "ask",
                        vec![text("Merge it?")],
                        vec![("options", list(vec![text("merge"), text("hold")]))],
                    ),
                ),
                4,
            ),
            assign("answer", field(name("r"), "answer")),
            assign("note", field(name("r"), "text")),
        ],
    );
    let mut run = start(
        ir,
        RunOptions::default(),
        bindings(vec![("me", Box::new(FakePerson::new("me", &log)))]),
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
    assert_eq!(message, "Merge it?");
    assert_eq!(options, &["merge", "hold"]);
    assert_eq!(common.source, at(4));
    assert_eq!(common.task, "main");
    assert!(
        matches!(run.next(), Err(RunError::WrongPayload { .. })),
        "must be resumed first"
    );
    run.resume(Resume::Answer {
        answer: "hold".into(),
        text: Some("wait for CI".into()),
    })
    .expect("resumes");
    let outputs = done_outputs(&run.next().expect("continues"));
    assert_eq!(outputs["answer"], Value::Text("hold".into()));
    assert_eq!(outputs["note"], Value::Text("wait for CI".into()));
    assert!(logged(&log).is_empty(), "a pause is not an adapter call");
}

#[test]
fn person_take_over_pauses_with_escalate_and_notify_does_not_pause() {
    // Spec 9.2.
    let log = call_log();
    let ir = program(
        vec![need("me", CapabilityKind::Person)],
        &["after"],
        vec![
            expr(verb("me", "notify", vec![text("heads up")])),
            expr(verb("me", "take_over", vec![])),
            assign("after", boolean(true)),
        ],
    );
    let mut run = start(
        ir,
        RunOptions::default(),
        bindings(vec![("me", Box::new(FakePerson::new("me", &log)))]),
        Answers::default(),
    );
    let pause = run.next().expect("steps");
    assert!(matches!(pause, Pause::Escalate { .. }), "{pause:?}");
    assert_eq!(logged(&log), vec!["me.notify(heads up)"]);
    run.resume(Resume::Continue { resume: true })
        .expect("resumes");
    let outputs = done_outputs(&run.next().expect("continues"));
    assert_eq!(outputs["after"], Value::Bool(true));
    assert_eq!(
        logged(&log),
        vec!["me.notify(heads up)"],
        "nothing is called twice"
    );
}

#[test]
fn llm_write_sends_only_the_using_value_counts_a_call_and_is_recorded() {
    // Spec 9.3: only `using` is sent; each write is one call, recorded as a
    // `generate` event.
    let log = call_log();
    let mut ir = program(
        vec![need("writer", CapabilityKind::Llm)],
        &["draft"],
        vec![
            assign("secret", text("never sent")),
            assign(
                "draft",
                verb_named(
                    "writer",
                    "write",
                    vec![text("Draft a reply")],
                    vec![("using", text("the mail"))],
                ),
            ),
        ],
    );
    ir.tasks[0].budget = budget(Some(1.0), None, None, None);
    let mut run = start(
        ir,
        RunOptions::default(),
        bindings(vec![("writer", Box::new(FakeLlm::new("writer", &log)))]),
        Answers::default(),
    );
    let outputs = done_outputs(&run.next().expect("steps"));
    assert_eq!(outputs["draft"], Value::Text("draft: the mail".into()));
    assert_eq!(
        logged(&log),
        vec!["writer.write(Draft a reply, using=the mail)"]
    );
    assert_eq!(run.usage().calls, 1);
    let events = events(&mut run);
    let generated = events_of(&events, "generate");
    assert_eq!(generated.len(), 1);
    assert!(
        matches!(generated[0], Event::Generate { instruction, using: Some(Value::Text(u)), output, .. }
        if instruction == "Draft a reply" && u == "the mail" && output == "draft: the mail")
    );

    // A second write is over the one-call budget.
    let mut ir = program(
        vec![need("writer", CapabilityKind::Llm)],
        &[],
        vec![
            expr(verb("writer", "write", vec![text("a")])),
            expr(verb("writer", "write", vec![text("b")])),
        ],
    );
    ir.tasks[0].budget = budget(Some(1.0), None, None, None);
    let mut run = start(
        ir,
        RunOptions::default(),
        bindings(vec![("writer", Box::new(FakeLlm::new("writer", &log)))]),
        Answers::default(),
    );
    let pause = run.next().expect("steps");
    assert!(matches!(pause, Pause::Budget { .. }), "{pause:?}");
}

#[test]
fn agent_wait_pauses_with_waiting_auto_resumes_and_inject_reaches_the_adapter() {
    // Spec 9.1 and 10.2: `wait` pauses with `waiting` and auto-resumes; the
    // host may `inject` a message, which reaches the adapter as `send` and is
    // recorded.
    let log = call_log();
    let ir = program(
        vec![need("claude", CapabilityKind::Agent)],
        &["msg"],
        vec![
            assign(
                "dev",
                verb_named("claude", "spawn", vec![], vec![("prompt", text("fix it"))]),
            ),
            on_line(
                assign(
                    "obs",
                    call_named(
                        field(name("dev"), "wait"),
                        vec![name("idle")],
                        vec![("minutes", num(5.0))],
                    ),
                ),
                6,
            ),
            assign("msg", field(name("obs"), "last_message")),
            expr(field(name("dev"), "stop")),
        ],
    );
    let agent = FakeAgent::new(
        "claude",
        &log,
        vec![observation("waiting", "done here", "$ ok")],
    );
    let mut run = start(
        ir,
        RunOptions::default(),
        bindings(vec![("claude", Box::new(agent))]),
        Answers::default(),
    );
    let pause = run.next().expect("steps");
    let Pause::Waiting {
        on,
        condition,
        timeout_minutes,
        common,
    } = &pause
    else {
        panic!("waiting, got {pause:?}");
    };
    assert_eq!(on, "claude");
    assert_eq!(condition, "idle");
    assert_eq!(*timeout_minutes, 5.0);
    assert_eq!(common.source, at(6));
    run.inject("claude", "hurry up").expect("injects");
    assert!(matches!(run.inject("tree", "x"), Err(RunError::NotPaused)));
    // No resume needed: `waiting` auto-resumes.
    let outputs = done_outputs(&run.next().expect("continues"));
    assert_eq!(outputs["msg"], Value::Text("done here".into()));
    assert_eq!(
        logged(&log),
        vec![
            "claude.spawn(prompt=fix it)",
            "claude.send({\"capability\":\"claude\",\"id\":\"agent-1\"}, hurry up)",
            "claude.wait({\"capability\":\"claude\",\"id\":\"agent-1\"}, idle, minutes=5)",
            "claude.stop({\"capability\":\"claude\",\"id\":\"agent-1\"})",
        ]
    );
    let events = events(&mut run);
    let calls = events_of(&events, "call");
    assert!(
        matches!(calls[1], Event::Call { verb, .. } if verb == "send"),
        "the injection is recorded"
    );
    assert_eq!(calls.len(), 4);
}

#[test]
fn trail_records_are_written_by_the_runtime_from_its_own_calls() {
    // Spec 7.5: a step record per call, `changed` from hashing the next
    // observation, outcomes observed | no_change | error.
    let log = call_log();
    let ir = program(
        vec![
            need("claude", CapabilityKind::Agent),
            need("tree", CapabilityKind::Tool),
        ],
        &["recent", "n"],
        vec![
            assign(
                "dev",
                verb_named("claude", "spawn", vec![], vec![("prompt", text("go"))]),
            ),
            expr(call(field(name("dev"), "send"), vec![text("first nudge")])),
            expr(field(name("dev"), "observe")),
            expr(verb("tree", "create", vec![text("branch")])),
            expr(field(name("dev"), "observe")),
            expr(verb("tree", "fail", vec![])),
            assign("recent", trail(2.0)),
            assign("n", call(name("len"), vec![trail(10.0)])),
        ],
    );
    let agent = FakeAgent::new(
        "claude",
        &log,
        vec![
            observation("running", "same", "same"),
            observation("running", "same", "same"),
        ],
    );
    let tool = FakeTool::new("tree", &log).verb("create", vec![Value::None]);
    let mut run = start(
        ir,
        RunOptions::default(),
        bindings(vec![("claude", Box::new(agent)), ("tree", Box::new(tool))]),
        Answers::default(),
    );
    let pause = run.next().expect("steps");
    // `tree.fail` is `verb_missing`: not retryable, so the run ends there.
    let Pause::Error { code, .. } = &pause else {
        panic!("error, got {pause:?}");
    };
    assert_eq!(*code, RuntimeErrorCode::VerbMissing);
    let events = events(&mut run);
    let steps: Vec<_> = events_of(&events, "step")
        .into_iter()
        .map(|e| match e {
            Event::Step { record } => record.clone(),
            _ => unreachable!(),
        })
        .collect();
    assert_eq!(steps.len(), 4);
    assert_eq!(
        (
            steps[0].step,
            steps[0].action.as_str(),
            steps[0].target.as_str(),
            steps[0].args.as_str()
        ),
        (1, "spawn", "claude", "prompt go")
    );
    assert!(
        steps[0].changed,
        "the first observation differs from nothing"
    );
    assert_eq!(steps[1].action, "send");
    assert_eq!(steps[1].target, "agent-1");
    assert_eq!(steps[1].args, "first nudge");
    assert_eq!(steps[2].action, "create");
    assert!(
        !steps[2].changed,
        "the second observation matched the first"
    );
    assert_eq!(steps[2].outcome, StepOutcome::NoChange);
    assert_eq!(
        (steps[3].action.as_str(), steps[3].outcome),
        ("fail", StepOutcome::Error)
    );
}

#[test]
fn trail_returns_the_last_n_records_of_the_enclosing_task() {
    // Spec 7.5: `trail n` is the last n step records, oldest first.
    let log = call_log();
    let ir = program(
        vec![need("claude", CapabilityKind::Agent)],
        &["recent", "all"],
        vec![
            assign(
                "dev",
                verb_named("claude", "spawn", vec![], vec![("prompt", text("go"))]),
            ),
            expr(call(field(name("dev"), "send"), vec![text("one")])),
            expr(call(field(name("dev"), "send"), vec![text("two")])),
            expr(field(name("dev"), "observe")),
            assign("recent", trail(2.0)),
            assign("all", trail(10.0)),
        ],
    );
    let agent = FakeAgent::new("claude", &log, vec![observation("running", "x", "y")]);
    let mut run = start(
        ir,
        RunOptions::default(),
        bindings(vec![("claude", Box::new(agent))]),
        Answers::default(),
    );
    let outputs = done_outputs(&run.next().expect("steps"));
    let Value::List(recent) = &outputs["recent"] else {
        panic!("a list");
    };
    let Value::List(all) = &outputs["all"] else {
        panic!("a list");
    };
    assert_eq!(all.len(), 3);
    assert_eq!(recent.len(), 2);
    let Value::Record(last) = &recent[1] else {
        panic!("a record");
    };
    assert_eq!(last["action"], Value::Text("send".into()));
    assert_eq!(last["args"], Value::Text("two".into()));
    assert_eq!(last["step"], Value::Number(3.0));
    assert_eq!(last["outcome"], Value::Text("observed".into()));
}

#[test]
fn a_retryable_adapter_error_pauses_and_a_retry_re_executes_only_that_call() {
    // Spec 10.2 and 12: `adapter_error` with the adapter's retryability;
    // `{ retry: true }` re-executes the failed call and nothing before it.
    let log = call_log();
    let ir = program(
        vec![need("tree", CapabilityKind::Tool)],
        &["b"],
        vec![
            expr(verb("tree", "create", vec![text("x")])),
            on_line(assign("b", verb("tree", "flaky", vec![])), 3),
        ],
    );
    let tool = FakeTool::new("tree", &log)
        .verb("create", vec![Value::None])
        .verb("flaky", vec![Value::Text("ok".into())])
        .flaky("flaky", 1);
    let mut run = start(
        ir,
        RunOptions::default(),
        bindings(vec![("tree", Box::new(tool))]),
        Answers::default(),
    );
    let pause = run.next().expect("steps");
    let Pause::Error {
        code,
        retryable,
        common,
        ..
    } = &pause
    else {
        panic!("error, got {pause:?}");
    };
    assert_eq!(*code, RuntimeErrorCode::AdapterError);
    assert!(retryable);
    assert_eq!(common.source, at(3));
    assert_eq!(run.state().paused_on(), Some(PauseKind::Error));
    run.resume(Resume::Retry { retry: true }).expect("retries");
    let outputs = done_outputs(&run.next().expect("continues"));
    assert_eq!(outputs["b"], Value::Text("ok".into()));
    assert_eq!(
        logged(&log),
        vec!["tree.create(x)", "tree.flaky()", "tree.flaky()"],
        "only the failed call ran again"
    );
}

#[test]
fn a_jev_outage_is_retryable_and_the_retry_resends_the_same_request() {
    // Spec 12: `jev_unavailable` is retryable.
    let dir = temp_dir("jev-retry");
    let ir = program(
        Vec::new(),
        &["p"],
        vec![assign("x", text("s")), assign("p", feels("x", "q"))],
    );
    let client = FlakyJev {
        failures: Mutex::new(1),
        answers: Answers::default().prob("p", 0.4),
    };
    let mut run = Run::create(ir, "main", tiny_profile_options(&dir), bindings(Vec::new()))
        .expect("starts")
        .with_client(Box::new(client));
    let pause = run.next().expect("steps");
    assert!(
        matches!(
            &pause,
            Pause::Error {
                code: RuntimeErrorCode::JevUnavailable,
                retryable: true,
                ..
            }
        ),
        "{pause:?}"
    );
    run.resume(Resume::Retry { retry: true }).expect("retries");
    let outputs = done_outputs(&run.next().expect("continues"));
    assert_eq!(outputs["p"], Value::Prob(0.4));
    assert_eq!(run.usage().calls, 1, "a failed attempt is not a call");
}

#[test]
fn failed_external_attempts_record_exact_identity_and_replay_without_services() {
    // Spec 10.3-10.4: request, call, observe and generate failures precede
    // retry handling and replay as the same errors without live services.

    let request_dir = temp_dir("effect-error-request");
    let request_recording = request_dir.join("run.jsonl");
    let request_ir = program(
        Vec::new(),
        &["p"],
        vec![assign("x", text("state")), assign("p", feels("x", "ready"))],
    );
    let mut request_options = tiny_profile_options(&request_dir);
    request_options.record = Some(request_recording.clone());
    let mut request = Run::create(
        request_ir.clone(),
        "main",
        request_options,
        bindings(Vec::new()),
    )
    .expect("request run starts")
    .with_client(Box::new(FlakyJev {
        failures: Mutex::new(1),
        answers: Answers::default().prob("p", 0.7),
    }));
    let request_error = retry_once_to_done(&mut request);
    assert!(request.log().iter().any(|event| matches!(
        event,
        Event::EffectError {
            operation: EffectOperation::Request,
            identity: EffectIdentity::Request { request_id },
            code: RuntimeErrorCode::JevUnavailable,
            message,
            retryable: true,
            ..
        } if request_id == "r1" && message == "503"
    )));
    let mut request_options = tiny_profile_options(&request_dir);
    request_options.replay = Some(request_recording);
    let mut request_replay = Run::create(request_ir, "main", request_options, bindings(Vec::new()))
        .expect("request replay starts")
        .with_client(Box::new(NeverJev));
    assert_eq!(retry_once_to_done(&mut request_replay), request_error);

    let call_dir = temp_dir("effect-error-call");
    let call_recording = call_dir.join("run.jsonl");
    let call_ir = program(
        vec![need("tree", CapabilityKind::Tool)],
        &["result"],
        vec![on_line(
            assign("result", verb("tree", "flaky", vec![text("x")])),
            7,
        )],
    );
    let call_log = call_log();
    let mut call_run = start(
        call_ir.clone(),
        RunOptions {
            record: Some(call_recording.clone()),
            ..RunOptions::default()
        },
        bindings(vec![(
            "tree",
            Box::new(
                FakeTool::new("tree", &call_log)
                    .verb("flaky", vec![Value::Text("ok".into())])
                    .flaky("flaky", 1),
            ),
        )]),
        Answers::default(),
    );
    let call_error = retry_once_to_done(&mut call_run);
    let expected_args = Value::from_json(
        &serde_json::to_value(CallArgs {
            positional: vec![Value::Text("x".into())],
            named: BTreeMap::new(),
        })
        .expect("arguments serialize"),
    );
    assert!(call_run.log().iter().any(|event| matches!(
        event,
        Event::EffectError {
            operation: EffectOperation::Call,
            identity: EffectIdentity::Call { capability, verb, args },
            code: RuntimeErrorCode::AdapterError,
            retryable: true,
            source,
            ..
        } if capability == "tree" && verb == "flaky"
            && args == &expected_args
            && *source == at(7)
    )));
    let mut call_replay = Run::create(
        call_ir,
        "main",
        RunOptions {
            replay: Some(call_recording.clone()),
            ..RunOptions::default()
        },
        bindings(vec![(
            "tree",
            Box::new(NeverCapability(RuntimeCapabilityKind::Tool)),
        )]),
    )
    .expect("call replay starts");
    assert_eq!(retry_once_to_done(&mut call_replay), call_error);
    let tampered = std::fs::read_to_string(&call_recording)
        .expect("reads recording")
        .lines()
        .map(|line| {
            let mut event: serde_json::Value = serde_json::from_str(line).expect("valid event");
            if event["event"] == "effect_error" && event["operation"] == "call" {
                event["identity"]["capability"] = serde_json::json!("other");
            }
            serde_json::to_string(&event).expect("serializes event")
        })
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(&call_recording, tampered + "\n").expect("tampers identity");
    let mut mismatched = Run::create(
        call_run.ir().clone(),
        "main",
        RunOptions {
            replay: Some(call_recording),
            ..RunOptions::default()
        },
        bindings(vec![(
            "tree",
            Box::new(NeverCapability(RuntimeCapabilityKind::Tool)),
        )]),
    )
    .expect("tampered replay starts");
    assert!(matches!(
        mismatched.next().expect("identity mismatch surfaces"),
        Pause::Error {
            code: RuntimeErrorCode::ReplayDiverged,
            ..
        }
    ));

    let observe_dir = temp_dir("effect-error-observe");
    let observe_recording = observe_dir.join("run.jsonl");
    let observe_ir = program(
        vec![need("claude", CapabilityKind::Agent)],
        &["status"],
        vec![
            assign(
                "dev",
                verb_named("claude", "spawn", vec![], vec![("prompt", text("go"))]),
            ),
            on_line(assign("obs", field(name("dev"), "observe")), 11),
            assign("status", field(name("obs"), "status")),
        ],
    );
    let mut observe_run = start(
        observe_ir.clone(),
        RunOptions {
            record: Some(observe_recording.clone()),
            ..RunOptions::default()
        },
        bindings(vec![("claude", Box::new(FlakyObserve { failed: false }))]),
        Answers::default(),
    );
    let observe_error = retry_once_to_done(&mut observe_run);
    assert!(observe_run.log().iter().any(|event| matches!(
        event,
        Event::EffectError {
            operation: EffectOperation::Observe,
            identity: EffectIdentity::Observe { capability, handle },
            code: RuntimeErrorCode::AdapterError,
            source,
            ..
        } if capability == "claude"
            && matches!(handle, Value::Handle(handle) if handle.id == "agent-1")
            && *source == at(11)
    )));
    let mut observe_replay = Run::create(
        observe_ir,
        "main",
        RunOptions {
            replay: Some(observe_recording),
            ..RunOptions::default()
        },
        bindings(vec![(
            "claude",
            Box::new(NeverCapability(RuntimeCapabilityKind::Agent)),
        )]),
    )
    .expect("observe replay starts");
    assert_eq!(retry_once_to_done(&mut observe_replay), observe_error);

    let generate_dir = temp_dir("effect-error-generate");
    let generate_recording = generate_dir.join("run.jsonl");
    let generate_ir = program(
        vec![need("writer", CapabilityKind::Llm)],
        &["draft"],
        vec![on_line(
            assign(
                "draft",
                verb_named(
                    "writer",
                    "write",
                    vec![text("summarize")],
                    vec![("using", text("context"))],
                ),
            ),
            15,
        )],
    );
    let mut generate_run = start(
        generate_ir.clone(),
        RunOptions {
            record: Some(generate_recording.clone()),
            ..RunOptions::default()
        },
        bindings(vec![("writer", Box::new(FlakyGenerate { failed: false }))]),
        Answers::default(),
    );
    let generate_error = retry_once_to_done(&mut generate_run);
    assert!(generate_run.log().iter().any(|event| matches!(
        event,
        Event::EffectError {
            operation: EffectOperation::Generate,
            identity: EffectIdentity::Generate {
                capability,
                instruction,
                using: Some(Value::Text(using)),
            },
            code: RuntimeErrorCode::AdapterError,
            source,
            ..
        } if capability == "writer" && instruction == "summarize"
            && using == "context" && *source == at(15)
    )));
    let mut generate_replay = Run::create(
        generate_ir,
        "main",
        RunOptions {
            replay: Some(generate_recording),
            ..RunOptions::default()
        },
        bindings(vec![(
            "writer",
            Box::new(NeverCapability(RuntimeCapabilityKind::Llm)),
        )]),
    )
    .expect("generate replay starts");
    assert_eq!(retry_once_to_done(&mut generate_replay), generate_error);
}

#[test]
fn jev_client_initialization_failure_is_recorded_and_replayed_without_credentials() {
    // Spec 10.3: client initialization belongs to the request attempt. Run
    // this case in an isolated process so removing a credential cannot race
    // another test that reads the process environment.
    const CHILD: &str = "JEVSCRIPT_INIT_FAILURE_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let status = std::process::Command::new(std::env::current_exe().expect("test executable"))
            .arg("--exact")
            .arg("jev_client_initialization_failure_is_recorded_and_replayed_without_credentials")
            .arg("--nocapture")
            .env(CHILD, "1")
            .env_remove(jevscript_runtime::jev::JEV_API_KEY_ENV)
            .status()
            .expect("starts isolated test process");
        assert!(status.success(), "isolated initialization test failed");
        return;
    }

    let dir = temp_dir("effect-error-client-init");
    let recording = dir.join("run.jsonl");
    let ir = program(
        Vec::new(),
        &["p"],
        vec![assign("x", text("state")), assign("p", feels("x", "ready"))],
    );
    let mut options = tiny_profile_options(&dir);
    options.record = Some(recording.clone());
    let mut run =
        Run::create(ir.clone(), "main", options, bindings(Vec::new())).expect("request run starts");
    let live_error = run.next().expect("missing credential surfaces");
    assert!(matches!(
        &live_error,
        Pause::Error {
            code: RuntimeErrorCode::JevRejected,
            ..
        }
    ));
    let request = run
        .log()
        .iter()
        .position(|event| matches!(event, Event::Request { .. }))
        .expect("request inputs recorded");
    let failure = run
        .log()
        .iter()
        .position(|event| {
            matches!(
                event,
                Event::EffectError {
                    operation: EffectOperation::Request,
                    code: RuntimeErrorCode::JevRejected,
                    ..
                }
            )
        })
        .expect("initialization failure recorded");
    let pause = run
        .log()
        .iter()
        .position(|event| {
            matches!(
                event,
                Event::Pause {
                    kind: PauseKind::Error,
                    ..
                }
            )
        })
        .expect("error pause recorded");
    assert!(request < failure && failure < pause);
    drop(run);

    let mut options = tiny_profile_options(&dir);
    options.replay = Some(recording);
    let mut replay = Run::create(ir, "main", options, bindings(Vec::new()))
        .expect("request replay starts without credentials");
    assert_eq!(
        replay.next().expect("replays initialization failure"),
        live_error
    );
}

#[test]
fn a_manifest_missing_a_verb_the_program_uses_fails_the_start() {
    // Spec 9.4: at `task.start` every verb the IR references on a tool is
    // compared against the manifest; a missing one is `verb_missing`.
    let log = call_log();
    let ir = program(
        vec![need("tree", CapabilityKind::Tool)],
        &[],
        vec![
            expr(verb("tree", "create", vec![text("x")])),
            expr(field(name("tree"), "open_pr")),
        ],
    );
    let tool = FakeTool::new("tree", &log).with_manifest(&["create"]);
    let error = Run::create(
        ir.clone(),
        "main",
        RunOptions::default(),
        bindings(vec![("tree", Box::new(tool))]),
    )
    .expect_err("refuses to start");
    assert_eq!(error.code, RuntimeErrorCode::VerbMissing);
    assert!(error.message.contains("`open_pr`"), "{}", error.message);

    let tool = FakeTool::new("tree", &log).with_manifest(&["create", "open_pr"]);
    Run::create(
        ir,
        "main",
        RunOptions::default(),
        bindings(vec![("tree", Box::new(tool))]),
    )
    .expect("a complete manifest starts");
}

#[test]
fn a_live_run_replays_its_own_recording_with_identical_pauses_and_no_calls() {
    // Spec 10.4: the recording replays with identical pauses in identical
    // order, and neither Jev nor an adapter is called.
    let dir = temp_dir("replay");
    let log = call_log();
    let ir = program(
        vec![
            need("me", CapabilityKind::Person),
            need("tree", CapabilityKind::Tool),
        ],
        &["c", "r"],
        vec![
            assign("x", verb("tree", "read", vec![])),
            assign("p", feels("x", "looks fine")),
            assign("c", pick("x", &[("a", "an a"), ("b", "a b")])),
            on_line(assign("r", verb("me", "ask", vec![text("Go on?")])), 5),
            assign("t", call(name("now"), vec![])),
            assign("u", call(name("random"), vec![])),
            if_else(
                binary(BinaryOp::Gt, name("p"), num(0.5)),
                vec![expr(verb("me", "notify", vec![text("high")]))],
                Some(vec![on_line(escalate("low"), 9)]),
            ),
        ],
    );
    let recording = dir.join("run.jsonl");
    let mut options = tiny_profile_options(&dir);
    options.record = Some(recording.clone());
    let tool = FakeTool::new("tree", &log).verb("read", vec![Value::Text("the file".into())]);
    let answers = Answers::default().prob("p", 0.2).label("c", "b", 0.7);
    let mut run = start(
        ir.clone(),
        options,
        bindings(vec![
            ("me", Box::new(FakePerson::new("me", &log))),
            ("tree", Box::new(tool)),
        ]),
        answers,
    );
    let pauses = drive(&mut run, |pause| match pause {
        Pause::Confirm { .. } => Some(Resume::Answer {
            answer: "yes".into(),
            text: None,
        }),
        Pause::Escalate { .. } => Some(Resume::Continue { resume: true }),
        _ => None,
    });
    let kinds: Vec<PauseKind> = pauses.iter().map(Pause::kind).collect();
    assert_eq!(
        kinds,
        vec![PauseKind::Confirm, PauseKind::Escalate, PauseKind::Done]
    );
    let live_events = events(&mut run);
    assert_eq!(logged(&log).len(), 1, "one adapter call: `tree.read`");

    // Replay: the same program against a Jev that panics and no adapters.
    let mut options = tiny_profile_options(&dir);
    options.replay = Some(recording.clone());
    let mut replay = Run::create(ir.clone(), "main", options, bindings(Vec::new()))
        .expect("a replay needs no bindings")
        .with_client(Box::new(NeverJev));
    assert_eq!(replay.id(), run.id(), "a replay keeps the recorded run id");
    let replayed = drive(&mut replay, |_| None);
    assert_eq!(replayed, pauses, "identical pauses in identical order");
    assert_eq!(logged(&log).len(), 1, "no adapter was called");
    assert_eq!(done_outputs(&replayed[2]), done_outputs(&pauses[2]));
    assert!(
        events(&mut replay).is_empty(),
        "a replay writes nothing new"
    );

    // Replay of the file the run wrote also matches the events it streamed.
    let lines = std::fs::read_to_string(&recording).expect("reads");
    assert_eq!(lines.lines().count(), live_events.len());
}

#[test]
fn replay_eof_before_adapter_or_jev_is_divergence_without_external_calls() {
    // Spec 10.4: exhaustion is a mismatch, not permission to call the world.
    let tool_dir = temp_dir("replay-eof-tool");
    let tool_recording = tool_dir.join("run.jsonl");
    let tool_ir = program(
        vec![need("tree", CapabilityKind::Tool)],
        &[],
        vec![expr(verb("tree", "read", vec![]))],
    );
    let log = call_log();
    let mut run = start(
        tool_ir.clone(),
        RunOptions {
            record: Some(tool_recording.clone()),
            ..RunOptions::default()
        },
        bindings(vec![(
            "tree",
            Box::new(FakeTool::new("tree", &log).verb("read", vec![Value::None])),
        )]),
        Answers::default(),
    );
    drive(&mut run, |_| None);
    truncate_before_event(&tool_recording, "call");
    let mut replay = Run::create(
        tool_ir,
        "main",
        RunOptions {
            replay: Some(tool_recording),
            ..RunOptions::default()
        },
        bindings(vec![("tree", Box::new(NeverTool))]),
    )
    .expect("valid partial recording starts")
    .with_client(Box::new(NeverJev));
    let Pause::Error { code, .. } = replay.next().expect("surfaces tool EOF") else {
        panic!("expected replay divergence");
    };
    assert_eq!(code, RuntimeErrorCode::ReplayDiverged);

    let jev_dir = temp_dir("replay-eof-jev");
    let jev_recording = jev_dir.join("run.jsonl");
    let jev_ir = program(
        Vec::new(),
        &["p"],
        vec![
            assign("x", text("state")),
            assign("p", feels("x", "is ready")),
        ],
    );
    let mut run = start(
        jev_ir.clone(),
        RunOptions {
            record: Some(jev_recording.clone()),
            ..RunOptions::default()
        },
        bindings(Vec::new()),
        Answers::default().prob("p", 0.9),
    );
    drive(&mut run, |_| None);
    truncate_before_event(&jev_recording, "request");
    let mut replay = Run::create(
        jev_ir,
        "main",
        RunOptions {
            replay: Some(jev_recording),
            ..RunOptions::default()
        },
        bindings(Vec::new()),
    )
    .expect("valid partial recording starts")
    .with_client(Box::new(NeverJev));
    let Pause::Error { code, .. } = replay.next().expect("surfaces Jev EOF") else {
        panic!("expected replay divergence");
    };
    assert_eq!(code, RuntimeErrorCode::ReplayDiverged);
}

#[test]
fn replay_eof_before_draw_and_missing_end_are_divergence() {
    // Spec 10.4: replay exhaustion cannot consult randomness or synthesize the
    // missing terminal end event.
    let draw_dir = temp_dir("replay-eof-draw");
    let draw_recording = draw_dir.join("run.jsonl");
    let draw_ir = program(
        Vec::new(),
        &["r"],
        vec![assign("r", call(name("random"), vec![]))],
    );
    let mut run = start(
        draw_ir.clone(),
        RunOptions {
            record: Some(draw_recording.clone()),
            ..RunOptions::default()
        },
        bindings(Vec::new()),
        Answers::default(),
    );
    drive(&mut run, |_| None);
    truncate_before_event(&draw_recording, "draw");
    let mut replay = Run::create(
        draw_ir,
        "main",
        RunOptions {
            replay: Some(draw_recording),
            ..RunOptions::default()
        },
        bindings(Vec::new()),
    )
    .expect("valid partial recording starts")
    .with_client(Box::new(NeverJev));
    let Pause::Error { code, .. } = replay.next().expect("surfaces draw EOF") else {
        panic!("expected replay divergence");
    };
    assert_eq!(code, RuntimeErrorCode::ReplayDiverged);
    assert!(
        events(&mut replay)
            .iter()
            .all(|event| !matches!(event, Event::Draw { .. })),
        "replay exhaustion emitted a new draw"
    );

    let end_dir = temp_dir("replay-eof-end");
    let end_recording = end_dir.join("run.jsonl");
    let end_ir = program(Vec::new(), &[], Vec::new());
    let mut run = start(
        end_ir.clone(),
        RunOptions {
            record: Some(end_recording.clone()),
            ..RunOptions::default()
        },
        bindings(Vec::new()),
        Answers::default(),
    );
    drive(&mut run, |_| None);
    truncate_before_event(&end_recording, "end");
    let mut replay = Run::create(
        end_ir,
        "main",
        RunOptions {
            replay: Some(end_recording),
            ..RunOptions::default()
        },
        bindings(Vec::new()),
    )
    .expect("valid partial recording starts")
    .with_client(Box::new(NeverJev));
    let Pause::Error { code, .. } = replay.next().expect("surfaces missing end") else {
        panic!("expected replay divergence");
    };
    assert_eq!(code, RuntimeErrorCode::ReplayDiverged);
    assert!(
        events(&mut replay).iter().all(|event| !matches!(
            event,
            Event::End {
                kind: PauseKind::Done,
                ..
            }
        )),
        "replay exhaustion synthesized the missing done end"
    );
}

#[test]
fn a_new_host_answer_at_replay_eof_explicitly_forks_live() {
    // Spec 10.4: a new answer to a surfaced pause is the one permitted way to
    // leave a partial replay and continue against live bindings.
    let dir = temp_dir("replay-new-answer");
    let recording = dir.join("run.jsonl");
    let ir = program(
        vec![need("me", CapabilityKind::Person)],
        &[],
        vec![
            assign("r", verb("me", "ask", vec![text("Continue?")])),
            expr(verb("me", "notify", vec![text("continued")])),
        ],
    );
    let source_log = call_log();
    let mut run = start(
        ir.clone(),
        RunOptions {
            record: Some(recording.clone()),
            ..RunOptions::default()
        },
        bindings(vec![("me", Box::new(FakePerson::new("me", &source_log)))]),
        Answers::default(),
    );
    assert!(matches!(
        run.next().expect("surfaces confirm"),
        Pause::Confirm { .. }
    ));
    drop(run);
    assert!(logged(&source_log).is_empty());

    let live_log = call_log();
    let mut replay = Run::create(
        ir,
        "main",
        RunOptions {
            replay: Some(recording),
            ..RunOptions::default()
        },
        bindings(vec![("me", Box::new(FakePerson::new("me", &live_log)))]),
    )
    .expect("partial replay starts")
    .with_client(Box::new(NeverJev));
    assert!(matches!(
        replay.next().expect("replays confirm"),
        Pause::Confirm { .. }
    ));
    replay
        .resume(Resume::Answer {
            answer: "yes".into(),
            text: None,
        })
        .expect("new answer forks live");
    assert!(matches!(
        replay.next().expect("continues live"),
        Pause::Done { .. }
    ));
    assert_eq!(logged(&live_log), vec!["me.notify(continued)"]);
}

#[test]
fn a_replay_leaves_the_recording_when_the_host_answers_differently() {
    // Spec 10.4: the recorded resume is applied unless the host chooses to
    // answer differently, after which the run continues live.
    let dir = temp_dir("replay-diverge-answer");
    let log = call_log();
    let ir = program(
        vec![need("me", CapabilityKind::Person)],
        &["took"],
        vec![
            assign("took", text("")),
            assign("r", verb("me", "ask", vec![text("Which?")])),
            if_else(
                binary(BinaryOp::Eq, field(name("r"), "answer"), text("yes")),
                vec![
                    assign("took", text("yes")),
                    expr(verb("me", "notify", vec![text("went yes")])),
                ],
                Some(vec![
                    assign("took", text("no")),
                    expr(verb("me", "notify", vec![text("went no")])),
                ]),
            ),
        ],
    );
    let recording = dir.join("run.jsonl");
    let options = RunOptions {
        record: Some(recording.clone()),
        ..RunOptions::default()
    };
    let mut run = start(
        ir.clone(),
        options,
        bindings(vec![("me", Box::new(FakePerson::new("me", &log)))]),
        Answers::default(),
    );
    let pauses = drive(&mut run, |_| {
        Some(Resume::Answer {
            answer: "yes".into(),
            text: None,
        })
    });
    assert_eq!(done_outputs(&pauses[1])["took"], Value::Text("yes".into()));

    // Replay with a different answer, with a live adapter bound.
    let options = RunOptions {
        replay: Some(recording.clone()),
        ..RunOptions::default()
    };
    let mut replay = Run::create(
        ir.clone(),
        "main",
        options,
        bindings(vec![("me", Box::new(FakePerson::new("me", &log)))]),
    )
    .expect("starts")
    .with_client(Box::new(NeverJev));
    let pauses = drive(&mut replay, |_| {
        Some(Resume::Answer {
            answer: "no".into(),
            text: None,
        })
    });
    assert_eq!(done_outputs(&pauses[1])["took"], Value::Text("no".into()));
    assert_eq!(
        logged(&log),
        vec!["me.notify(went yes)", "me.notify(went no)"]
    );

    // The same, without a live adapter: going live pauses with an error.
    let options = RunOptions {
        replay: Some(recording),
        ..RunOptions::default()
    };
    let mut replay = Run::create(ir, "main", options, bindings(Vec::new())).expect("starts");
    let pauses = drive(&mut replay, |_| {
        Some(Resume::Answer {
            answer: "no".into(),
            text: None,
        })
    });
    let Pause::Error { code, .. } = &pauses[1] else {
        panic!("error, got {:?}", pauses[1]);
    };
    assert_eq!(*code, RuntimeErrorCode::BindingMissing);
}

#[test]
fn a_changed_program_diverges_from_its_recording_naming_the_source() {
    // Spec 10.4: a mismatch pauses with `replay_diverged` and names the
    // source location.
    let dir = temp_dir("diverged");
    let log = call_log();
    let ir = program(
        vec![need("tree", CapabilityKind::Tool)],
        &[],
        vec![
            expr(verb("tree", "create", vec![text("x")])),
            expr(verb("tree", "test", vec![])),
        ],
    );
    let recording = dir.join("run.jsonl");
    let options = RunOptions {
        record: Some(recording.clone()),
        ..RunOptions::default()
    };
    let tool = FakeTool::new("tree", &log)
        .verb("create", vec![Value::None])
        .verb("test", vec![Value::None]);
    let mut run = start(
        ir,
        options,
        bindings(vec![("tree", Box::new(tool))]),
        Answers::default(),
    );
    drive(&mut run, |_| None);

    let changed = program(
        vec![need("tree", CapabilityKind::Tool)],
        &[],
        vec![
            expr(verb("tree", "create", vec![text("x")])),
            on_line(expr(verb("tree", "open_pr", vec![])), 8),
        ],
    );
    let options = RunOptions {
        replay: Some(recording),
        ..RunOptions::default()
    };
    let mut replay = Run::create(changed, "main", options, bindings(Vec::new())).expect("starts");
    let pause = replay.next().expect("steps");
    let Pause::Error {
        code,
        common,
        message,
        ..
    } = &pause
    else {
        panic!("error, got {pause:?}");
    };
    assert_eq!(*code, RuntimeErrorCode::ReplayDiverged);
    assert_eq!(common.source, at(8));
    assert!(message.contains("open_pr"), "{message}");
    assert!(replay.state().is_ended());
}

#[test]
fn a_sampled_run_replays_exactly() {
    // Spec 6.11: every draw goes through the recorded random source, so a
    // sampled run replays exactly and the answers event notes `sampled`.
    let dir = temp_dir("sampled");
    let ir = program(
        Vec::new(),
        &["c", "l", "p"],
        vec![
            assign("x", text("s")),
            assign("c", sampled(pick("x", &[("a", "an a"), ("b", "a b")]))),
            assign("l", rate("x", &[("low", "nothing"), ("high", "a lot")])),
            assign("p", feels("x", "q")),
        ],
    );
    let recording = dir.join("run.jsonl");
    let mut options = tiny_profile_options(&dir);
    options.record = Some(recording.clone());
    options.sample = Some(jevscript_runtime::rpc::Sample::Seeded { seed: 11 });
    let answers = Answers::default()
        .label("c", "a", 0.1)
        .level("l", 1, 0.5)
        .prob("p", 0.3);
    let mut run = start(ir.clone(), options, bindings(Vec::new()), answers.clone());
    let first = done_outputs(&run.next().expect("steps"));
    let events = events(&mut run);
    let answered = events_of(&events, "answers");
    assert_eq!(answered.len(), 1);
    assert!(matches!(answered[0], Event::Answers { sampled: true, .. }));
    assert_eq!(
        events_of(&events, "draw").len(),
        2 + 2,
        "two sampling draws plus the clock at start and end"
    );

    let mut options = tiny_profile_options(&dir);
    options.replay = Some(recording);
    // A replay is started with the same options as the run (spec 10.4: the
    // same program, inputs and recording); `sample` is one of them.
    options.sample = Some(jevscript_runtime::rpc::Sample::Seeded { seed: 11 });
    let mut replay = Run::create(ir, "main", options, bindings(Vec::new()))
        .expect("starts")
        .with_client(Box::new(NeverJev));
    let again = done_outputs(&replay.next().expect("steps"));
    assert_eq!(first, again);
}

#[test]
fn failed_recording_pair_setup_runs_no_external_effect() {
    // Spec 10.3: failure to create either member of a redacted recording pair
    // happens before the task can call an adapter.
    let dir = temp_dir("record-setup-before-effects");
    let recording = dir.join("run.jsonl");
    let companion = jevscript_runtime::record::companion_path(&recording);
    std::fs::write(&companion, "pre-existing companion").expect("writes fixture");
    let log = call_log();
    let ir = program(
        vec![need("tree", CapabilityKind::Tool)],
        &[],
        vec![expr(verb("tree", "write", vec![text("must not run")]))],
    );
    let tool = FakeTool::new("tree", &log).verb("write", vec![Value::None]);
    let error = Run::create(
        ir,
        "main",
        RunOptions {
            record: Some(recording.clone()),
            redaction: jevscript_runtime::record::Redaction::Redact,
            ..RunOptions::default()
        },
        bindings(vec![("tree", Box::new(tool))]),
    )
    .expect_err("existing companion refuses setup");
    assert_eq!(error.code, RuntimeErrorCode::AdapterError);
    assert!(logged(&log).is_empty(), "adapter effect ran during setup");
    assert!(!recording.exists(), "new primary was rolled back");
    assert_eq!(
        std::fs::read_to_string(companion).expect("reads fixture"),
        "pre-existing companion"
    );
}

#[test]
fn created_and_paused_host_aborts_record_and_replay_exactly() {
    // Spec 10.3: a created abort records start first and no program effect.
    let created_dir = temp_dir("abort-created");
    let created_recording = created_dir.join("run.jsonl");
    let created_log = call_log();
    let created_ir = program(
        vec![need("tree", CapabilityKind::Tool)],
        &[],
        vec![expr(verb("tree", "write", vec![text("must not run")]))],
    );
    let mut created = start(
        created_ir,
        RunOptions {
            record: Some(created_recording.clone()),
            ..RunOptions::default()
        },
        bindings(vec![(
            "tree",
            Box::new(FakeTool::new("tree", &created_log).verb("write", vec![Value::None])),
        )]),
        Answers::default(),
    );
    created.abort().expect("aborts created run");
    assert!(logged(&created_log).is_empty());
    let created_pause = created.current_pause().cloned().expect("stopped pause");
    assert!(matches!(
        &created_pause,
        Pause::Stopped { reason, .. } if reason == "aborted by host"
    ));
    assert_eq!(
        created
            .log()
            .iter()
            .map(|event| match event {
                Event::Start { .. } => "start",
                Event::Abort { .. } => "abort",
                Event::Pause {
                    kind: PauseKind::Stopped,
                    ..
                } => "stopped",
                Event::End {
                    kind: PauseKind::Stopped,
                    ..
                } => "end",
                other => panic!("unexpected created-abort event: {other:?}"),
            })
            .collect::<Vec<_>>(),
        vec!["start", "abort", "stopped", "end"]
    );
    let mut replay = Run::from_recording(created_recording).expect("replay starts");
    assert_eq!(replay.next().expect("replays abort"), created_pause);

    // A paused abort follows the open pause without inventing a resume, and
    // replay consumes it on the next step.
    let paused_dir = temp_dir("abort-paused");
    let paused_recording = paused_dir.join("run.jsonl");
    let paused_ir = program(
        vec![need("me", CapabilityKind::Person)],
        &[],
        vec![
            expr(verb("me", "ask", vec![text("Continue?")])),
            expr(verb("me", "notify", vec![text("must not run")])),
        ],
    );
    let paused_log = call_log();
    let mut paused = start(
        paused_ir,
        RunOptions {
            record: Some(paused_recording.clone()),
            ..RunOptions::default()
        },
        bindings(vec![("me", Box::new(FakePerson::new("me", &paused_log)))]),
        Answers::default(),
    );
    assert!(matches!(
        paused.next().expect("surfaces confirm"),
        Pause::Confirm { .. }
    ));
    paused.abort().expect("aborts paused run");
    let paused_abort = paused.current_pause().cloned().expect("stopped pause");
    assert!(logged(&paused_log).is_empty());
    assert!(
        paused
            .log()
            .iter()
            .any(|event| matches!(event, Event::Abort { .. }))
    );
    assert!(
        paused
            .log()
            .iter()
            .all(|event| !matches!(event, Event::Resume { .. }))
    );
    let mut replay = Run::from_recording(paused_recording).expect("replay starts");
    assert!(matches!(
        replay.next().expect("replays confirm"),
        Pause::Confirm { .. }
    ));
    assert_eq!(replay.next().expect("replays paused abort"), paused_abort);
}

#[test]
fn host_abort_at_a_replayed_pause_discards_the_unused_suffix() {
    // Spec 10.3: a host abort chooses a new boundary during replay instead of
    // applying the recorded answer or retaining effects after that boundary.
    let dir = temp_dir("abort-replay-suffix");
    let recording = dir.join("run.jsonl");
    let ir = program(
        vec![need("me", CapabilityKind::Person)],
        &[],
        vec![
            expr(verb("me", "ask", vec![text("Continue?")])),
            expr(verb("me", "notify", vec![text("recorded suffix")])),
        ],
    );
    let call_log = call_log();
    let mut original = start(
        ir,
        RunOptions {
            record: Some(recording.clone()),
            ..RunOptions::default()
        },
        bindings(vec![("me", Box::new(FakePerson::new("me", &call_log)))]),
        Answers::default(),
    );
    assert!(matches!(
        original.next().expect("surfaces recorded confirm"),
        Pause::Confirm { .. }
    ));
    original
        .resume(Resume::Answer {
            answer: "yes".into(),
            text: None,
        })
        .expect("answers original confirm");
    assert!(matches!(
        original.next().expect("records suffix"),
        Pause::Done { .. }
    ));
    assert_eq!(logged(&call_log), vec!["me.notify(recorded suffix)"]);

    let mut replay = Run::from_recording(recording.clone()).expect("replay starts");
    assert!(matches!(
        replay.next().expect("surfaces replayed confirm"),
        Pause::Confirm { .. }
    ));
    replay.abort().expect("host aborts at replayed pause");
    assert!(matches!(
        replay.current_pause(),
        Some(Pause::Stopped { reason, .. }) if reason == "aborted by host"
    ));
    assert!(
        replay
            .log()
            .iter()
            .all(|event| !matches!(event, Event::Resume { .. }))
    );
    assert!(
        replay
            .log()
            .iter()
            .all(|event| !matches!(event, Event::Call { verb, .. } if verb == "notify"))
    );

    // The thread-safe request follows the same rule while `next` owns the
    // mutable run: it replaces the suffix at the first interpreter boundary.
    let mut replay = Run::from_recording(recording).expect("replay starts again");
    replay.abort_handle().abort();
    assert!(matches!(
        replay.next().expect("host signal aborts replay"),
        Pause::Stopped { ref reason, .. } if reason == "aborted by host"
    ));
    assert!(
        replay
            .log()
            .iter()
            .all(|event| !matches!(event, Event::Resume { .. } | Event::Call { .. }))
    );
}

#[test]
fn inflight_abort_records_the_completed_call_and_prevents_the_next_effect() {
    // Spec 9.6 and 10.3: cancellation waits for the synchronous call result,
    // then records abort and stops before another statement or effect.
    let dir = temp_dir("abort-inflight");
    let recording = dir.join("run.jsonl");
    let log = call_log();
    let ir = program(
        vec![need("tree", CapabilityKind::Tool)],
        &[],
        vec![
            expr(verb("tree", "first", vec![])),
            expr(verb("tree", "second", vec![])),
        ],
    );
    let tool = FakeTool::new("tree", &log)
        .slow("first", 100)
        .verb("first", vec![Value::Text("finished".into())])
        .verb("second", vec![Value::None]);
    let mut run = start(
        ir,
        RunOptions {
            record: Some(recording.clone()),
            ..RunOptions::default()
        },
        bindings(vec![("tree", Box::new(tool))]),
        Answers::default(),
    );
    let abort = run.abort_handle();
    let abort_log = Arc::clone(&log);
    let requester = std::thread::spawn(move || {
        while logged(&abort_log).is_empty() {
            std::thread::yield_now();
        }
        abort.abort();
    });
    let pause = run
        .next()
        .expect("call returns before cancellation settles");
    requester.join().expect("abort requester finishes");
    assert!(matches!(
        &pause,
        Pause::Stopped { reason, .. } if reason == "aborted by host"
    ));
    assert_eq!(logged(&log), vec!["tree.first()"]);
    let call = run
        .log()
        .iter()
        .position(|event| matches!(event, Event::Call { verb, .. } if verb == "first"))
        .expect("completed call recorded");
    let abort = run
        .log()
        .iter()
        .position(|event| matches!(event, Event::Abort { .. }))
        .expect("abort recorded");
    assert!(call < abort);
    let mut replay = Run::from_recording(recording).expect("replay starts");
    assert_eq!(replay.next().expect("replays in-flight abort"), pause);
}

#[test]
fn inflight_abort_beats_retry_after_a_failed_callback() {
    // Spec 9.6 and 10.3: the failed attempt is recorded before cancellation,
    // which still wins over retry and replays at the identical boundary.
    let dir = temp_dir("abort-failed-inflight");
    let recording = dir.join("run.jsonl");
    let log = call_log();
    let ir = program(
        vec![need("tree", CapabilityKind::Tool)],
        &[],
        vec![expr(verb("tree", "flaky", vec![]))],
    );
    let tool = FakeTool::new("tree", &log)
        .slow("flaky", 100)
        .flaky("flaky", 1)
        .verb("flaky", vec![Value::None]);
    let mut run = start(
        ir.clone(),
        RunOptions {
            record: Some(recording.clone()),
            ..RunOptions::default()
        },
        bindings(vec![("tree", Box::new(tool))]),
        Answers::default(),
    );
    let abort = run.abort_handle();
    let abort_log = Arc::clone(&log);
    let requester = std::thread::spawn(move || {
        while logged(&abort_log).is_empty() {
            std::thread::yield_now();
        }
        abort.abort();
    });
    let stopped = run.next().expect("cancellation beats retry");
    assert!(matches!(
        &stopped,
        Pause::Stopped { reason, .. } if reason == "aborted by host"
    ));
    requester.join().expect("abort requester finishes");
    assert_eq!(logged(&log), vec!["tree.flaky()"]);
    let failed = run
        .log()
        .iter()
        .position(|event| matches!(event, Event::EffectError { .. }))
        .expect("failed attempt recorded");
    let aborted = run
        .log()
        .iter()
        .position(|event| matches!(event, Event::Abort { .. }))
        .expect("abort recorded");
    assert!(failed < aborted);
    assert!(run.log().iter().all(|event| !matches!(
        event,
        Event::Pause {
            kind: PauseKind::Error,
            ..
        }
    )));
    let mut replay = Run::create(
        ir,
        "main",
        RunOptions {
            replay: Some(recording),
            ..RunOptions::default()
        },
        bindings(vec![(
            "tree",
            Box::new(NeverCapability(RuntimeCapabilityKind::Tool)),
        )]),
    )
    .expect("replay starts");
    assert_eq!(
        replay.next().expect("replays failed attempt then abort"),
        stopped
    );
}

#[test]
fn redaction_keeps_the_recording_replayable() {
    // Spec 10.3: `redact` stores a hash and a token count for state and
    // observations, and control flow still replays.
    let dir = temp_dir("redact");
    let log = call_log();
    let ir = program(
        vec![need("claude", CapabilityKind::Agent)],
        &["p", "status"],
        vec![
            assign(
                "dev",
                verb_named("claude", "spawn", vec![], vec![("prompt", text("go"))]),
            ),
            assign("obs", field(name("dev"), "observe")),
            assign("status", field(name("obs"), "status")),
            assign("p", feels("obs.last_message", "is secret")),
        ],
    );
    let recording = dir.join("run.jsonl");
    let mut options = tiny_profile_options(&dir);
    options.record = Some(recording.clone());
    options.redaction = jevscript_runtime::record::Redaction::Redact;
    let agent = FakeAgent::new(
        "claude",
        &log,
        vec![observation("exited", "the secret text", "")],
    );
    let mut run = start(
        ir.clone(),
        options,
        bindings(vec![("claude", Box::new(agent))]),
        Answers::default().prob("p", 0.9),
    );
    let first = done_outputs(&run.next().expect("steps"));
    let text = std::fs::read_to_string(&recording).expect("reads");
    assert!(!text.contains("the secret text"), "{text}");

    let mut options = tiny_profile_options(&dir);
    options.replay = Some(recording);
    let mut replay = Run::create(ir, "main", options, bindings(Vec::new()))
        .expect("starts")
        .with_client(Box::new(NeverJev));
    let again = done_outputs(&replay.next().expect("steps"));
    assert_eq!(again["p"], first["p"], "judgments replay from the log");
    assert_eq!(
        again["status"],
        Value::Text("exited".into()),
        "the recorded pause is what the host sees again"
    );
}

#[test]
fn redacted_wait_replay_restores_values_for_direct_branches_without_adapter_calls() {
    // Spec 10.3: an agent.wait result is private in the primary file, but its
    // full status and text are restored before direct control flow replays.
    let dir = temp_dir("redacted-wait-branch");
    let log = call_log();
    let ir = program(
        vec![need("claude", CapabilityKind::Agent)],
        &["branch"],
        vec![
            assign(
                "dev",
                verb_named("claude", "spawn", vec![], vec![("prompt", text("go"))]),
            ),
            assign(
                "obs",
                call_named(
                    field(name("dev"), "wait"),
                    vec![name("idle")],
                    vec![("minutes", num(5.0))],
                ),
            ),
            if_else(
                binary(BinaryOp::Eq, field(name("obs"), "status"), text("exited")),
                vec![if_else(
                    field(name("obs"), "last_message"),
                    vec![assign("branch", text("finished"))],
                    Some(vec![assign("branch", text("wrong-text"))]),
                )],
                Some(vec![assign("branch", text("wrong-status"))]),
            ),
        ],
    );
    let recording = dir.join("run.jsonl");
    let mut options = RunOptions {
        record: Some(recording.clone()),
        redaction: jevscript_runtime::record::Redaction::Redact,
        ..RunOptions::default()
    };
    let agent = FakeAgent::new(
        "claude",
        &log,
        vec![observation(
            "exited",
            "private wait branch text",
            "private tail",
        )],
    );
    let mut run = start(
        ir.clone(),
        options.clone(),
        bindings(vec![("claude", Box::new(agent))]),
        Answers::default(),
    );
    let first = drive(&mut run, |_| None);
    let first_outputs = done_outputs(first.last().expect("done"));
    assert_eq!(first_outputs["branch"], Value::Text("finished".into()));
    let calls_after_live = logged(&log).len();
    let primary = std::fs::read_to_string(&recording).expect("reads primary");
    assert!(!primary.contains("private wait branch text"), "{primary}");
    assert!(!primary.contains("private tail"), "{primary}");

    options.record = None;
    options.replay = Some(recording);
    options.redaction = jevscript_runtime::record::Redaction::Full;
    let mut replay = Run::create(ir, "main", options, bindings(Vec::new()))
        .expect("validated companion starts replay")
        .with_client(Box::new(NeverJev));
    let replayed = drive(&mut replay, |_| None);
    let replayed_outputs = done_outputs(replayed.last().expect("done"));
    assert_eq!(replayed_outputs, first_outputs);
    assert_eq!(
        logged(&log).len(),
        calls_after_live,
        "replay made no adapter call"
    );
}

#[test]
fn a_task_calls_a_library_task_under_the_module_mapping() {
    // Spec 3.9: a library reaches only the capabilities it was handed, under
    // the importer's names; its pauses carry the qualified task name.
    let log = call_log();
    let mut ir = program(
        vec![
            need("tree", CapabilityKind::Tool),
            need("me", CapabilityKind::Person),
        ],
        &["r"],
        vec![assign("r", call_unit("lib.check", vec![num(2.0)]))],
    );
    ir.modules.push(jevscript_ir::Module {
        alias: "lib".into(),
        program: "lib".into(),
        path: Some("./lib.jev".into()),
        mapping: vec![
            jevscript_ir::NeedMapping {
                inner: "repo".into(),
                outer: "tree".into(),
                kind: CapabilityKind::Tool,
            },
            jevscript_ir::NeedMapping {
                inner: "owner".into(),
                outer: "me".into(),
                kind: CapabilityKind::Person,
            },
        ],
        span: Default::default(),
    });
    ir.tasks.push(task(
        "lib.check",
        vec![param("n")],
        budget(Some(10.0), None, None, None),
        thresholds(None, None, None, None),
        vec![
            assign("v", verb("repo", "version", vec![])),
            on_line(expr(verb("owner", "ask", vec![text("ok?")])), 12),
            ret(Some(binary(BinaryOp::Mul, name("v"), name("n")))),
        ],
    ));
    let tool = FakeTool::new("tree", &log).verb("version", vec![Value::Number(21.0)]);
    let mut run = start(
        ir,
        RunOptions::default(),
        bindings(vec![
            ("tree", Box::new(tool)),
            ("me", Box::new(FakePerson::new("me", &log))),
        ]),
        Answers::default(),
    );
    let pause = run.next().expect("steps");
    let Pause::Confirm { common, .. } = &pause else {
        panic!("confirm, got {pause:?}");
    };
    assert_eq!(common.task, "lib.check");
    assert_eq!(common.source, at(12));
    run.resume(Resume::Answer {
        answer: "yes".into(),
        text: None,
    })
    .expect("resumes");
    let outputs = done_outputs(&run.next().expect("continues"));
    assert_eq!(outputs["r"], Value::Number(42.0));
    assert_eq!(logged(&log), vec!["tree.version()"]);
}

#[test]
fn minutes_run_while_the_runtime_blocks_and_are_checked_before_the_next_effect() {
    // Spec 7.1 and 9.6: `minutes` is wall time excluding host pauses and keeps
    // running while the runtime blocks on an adapter; exceeding it pauses with
    // `budget` before the next effect runs, and before `done`.
    let log = call_log();
    let mut ir = program(
        vec![need("tree", CapabilityKind::Tool)],
        &["b"],
        vec![
            expr(verb("tree", "slow", vec![])),
            assign("b", verb("tree", "mutate", vec![])),
        ],
    );
    // One millisecond of budget; the adapter takes twenty.
    ir.tasks[0].budget = budget(Some(50.0), Some(0.001 / 60.0), None, None);
    let tool = FakeTool::new("tree", &log)
        .verb("slow", vec![Value::None])
        .slow("slow", 20)
        .verb("mutate", vec![Value::Text("done".into())]);
    let mut run = start(
        ir,
        RunOptions::default(),
        bindings(vec![("tree", Box::new(tool))]),
        Answers::default(),
    );
    let pause = run.next().expect("steps");
    let Pause::Budget {
        key, used, limit, ..
    } = &pause
    else {
        panic!("budget, got {pause:?}");
    };
    assert_eq!(*key, BudgetKey::Minutes);
    assert!(used > limit, "{used} over {limit}");
    assert_eq!(
        logged(&log),
        vec!["tree.slow()"],
        "the second verb did not run"
    );
    run.resume(Resume::Extend {
        extend: BTreeMap::from([(BudgetKey::Minutes, 100.0)]),
    })
    .expect("extends");
    let outputs = done_outputs(&run.next().expect("continues"));
    assert_eq!(outputs["b"], Value::Text("done".into()));
    assert_eq!(logged(&log), vec!["tree.slow()", "tree.mutate()"]);

    // With the slow call as the whole task, the run still cannot end `done`
    // over budget: the check comes before `done`.
    let mut ir = program(
        vec![need("tree", CapabilityKind::Tool)],
        &[],
        vec![expr(verb("tree", "slow", vec![]))],
    );
    ir.tasks[0].budget = budget(Some(50.0), Some(0.001 / 60.0), None, None);
    let tool = FakeTool::new("tree", &log)
        .verb("slow", vec![Value::None])
        .slow("slow", 20);
    let mut run = start(
        ir,
        RunOptions::default(),
        bindings(vec![("tree", Box::new(tool))]),
        Answers::default(),
    );
    let pause = run.next().expect("steps");
    assert!(
        matches!(
            &pause,
            Pause::Budget {
                key: BudgetKey::Minutes,
                ..
            }
        ),
        "{pause:?}"
    );
}

#[test]
fn a_task_call_that_pauses_closes_the_callers_batch_in_a_compiled_program() {
    // Spec 6.6 and 3.9: batching never crosses a call boundary, so a judgment
    // before a task call and one after it are two requests, and the task's
    // pause sits between them in the recording.
    let source = "program t\n\nneeds me: person\n\ntask helper(y):\n  r = me.ask \"?\"\n  return y\n\ntask main:\n  x = \"x\"\n  a = x feels \"a\"\n  h = helper(x)\n  b = x feels \"b\"\n";
    let ir =
        jevscript_compiler::compile_source(source).unwrap_or_else(|d| panic!("compiles: {d:?}"));
    let log = call_log();
    let answers = Answers::default().prob("a", 0.1).prob("b", 0.2);
    let mut run = start(
        ir,
        RunOptions::default(),
        bindings(vec![("me", Box::new(FakePerson::new("me", &log)))]),
        answers,
    );
    let pauses = drive(&mut run, |pause| match pause {
        Pause::Confirm { .. } => Some(Resume::Answer {
            answer: "yes".into(),
            text: None,
        }),
        _ => None,
    });
    let kinds: Vec<PauseKind> = pauses.iter().map(Pause::kind).collect();
    assert_eq!(kinds, vec![PauseKind::Confirm, PauseKind::Done]);
    let events = events(&mut run);
    let order: Vec<String> = events
        .iter()
        .filter_map(|e| match e {
            Event::Request { questions, .. } => Some(format!(
                "request({})",
                questions
                    .iter()
                    .map(|q| q.id())
                    .collect::<Vec<_>>()
                    .join(",")
            )),
            Event::Pause { kind, .. } => Some(format!("pause({})", kind.as_str())),
            _ => None,
        })
        .collect();
    assert_eq!(
        order,
        vec!["request(a)", "pause(confirm)", "request(b)", "pause(done)"]
    );
    assert_eq!(run.usage().calls, 2);
}

#[test]
fn a_return_before_an_effectful_verify_loop_never_reads_the_condition() {
    // Spec 7.4: returning before the verify loop is unverified completion,
    // and the skipped condition's capability read does not happen.
    let log = call_log();
    let ir = program(
        vec![need("tree", CapabilityKind::Tool)],
        &["r"],
        vec![
            assign("r", verb("tree", "create", vec![text("x")])),
            if_else(boolean(true), vec![ret(None)], None),
            until_verify(field(name("tree"), "tests_pass"), 3.0, vec![brk()]),
        ],
    );
    let tool = FakeTool::new("tree", &log)
        .verb("create", vec![Value::None])
        .verb("tests_pass", vec![Value::Bool(true)]);
    let mut run = start(
        ir,
        RunOptions::default(),
        bindings(vec![("tree", Box::new(tool))]),
        Answers::default(),
    );
    let pause = run.next().expect("steps");
    let Pause::Done { verified, .. } = &pause else {
        panic!("done, got {pause:?}");
    };
    assert!(!verified);
    assert_eq!(
        logged(&log),
        vec!["tree.create(x)"],
        "`tests_pass` was never read"
    );

    // Reaching the loop, the condition is read for the loop and once more
    // at the end of the task (spec 7.4).
    let ir = program(
        vec![need("tree", CapabilityKind::Tool)],
        &[],
        vec![until_verify(
            field(name("tree"), "tests_pass"),
            3.0,
            vec![brk()],
        )],
    );
    let tool =
        FakeTool::new("tree", &log).verb("tests_pass", vec![Value::Bool(false), Value::Bool(true)]);
    let mut run = start(
        ir,
        RunOptions::default(),
        bindings(vec![("tree", Box::new(tool))]),
        Answers::default(),
    );
    let Pause::Done { verified, .. } = run.next().expect("steps") else {
        panic!("done");
    };
    assert!(verified, "false in the loop, true at the end");
    assert_eq!(
        logged(&log),
        vec!["tree.create(x)", "tree.tests_pass()", "tree.tests_pass()"]
    );
}

/// `main` calls `helper`, whose tool verb sleeps twenty milliseconds and fails
/// retryably on its first attempt. One task or the other has a one-millisecond
/// minutes budget.
fn nested_flaky_slow(
    main_minutes: f64,
    helper_minutes: f64,
    log: &CallLog,
) -> jevscript_runtime::Run {
    let mut ir = program(
        vec![need("tree", CapabilityKind::Tool)],
        &["r"],
        vec![assign("r", call_unit("helper", vec![]))],
    );
    ir.tasks[0].budget = budget(Some(50.0), Some(main_minutes), None, None);
    ir.tasks.push(task(
        "helper",
        vec![],
        budget(Some(50.0), Some(helper_minutes), None, None),
        thresholds(None, None, None, None),
        vec![ret(Some(verb("tree", "slow", vec![])))],
    ));
    batch(&mut ir);
    let tool = FakeTool::new("tree", log)
        .verb("slow", vec![Value::Text("ok".into())])
        .slow("slow", 20)
        .flaky("slow", 1);
    start(
        ir,
        RunOptions::default(),
        bindings(vec![("tree", Box::new(tool))]),
        Answers::default(),
    )
}

#[test]
fn minutes_are_checked_after_a_retryable_failure_before_the_effect_is_retried() {
    // Spec 7.1 and 9.6: the clock ran through the failed attempt, so once the
    // host retries, an exhausted minutes budget pauses before the adapter is
    // invoked again, naming the task whose limit was hit.
    for (main_minutes, helper_minutes, expected_task) in
        [(1.0, 0.001 / 60.0, "helper"), (0.001 / 60.0, 1.0, "main")]
    {
        let log = call_log();
        let mut run = nested_flaky_slow(main_minutes, helper_minutes, &log);
        let pause = run.next().expect("steps");
        assert!(
            matches!(
                &pause,
                Pause::Error {
                    code: RuntimeErrorCode::AdapterError,
                    retryable: true,
                    ..
                }
            ),
            "{pause:?}"
        );
        run.resume(Resume::Retry { retry: true }).expect("retries");
        let pause = run.next().expect("continues");
        let Pause::Budget { key, common, .. } = &pause else {
            panic!("budget, got {pause:?}");
        };
        assert_eq!(*key, BudgetKey::Minutes);
        assert_eq!(
            common.task, expected_task,
            "{main_minutes} {helper_minutes}"
        );
        assert_eq!(
            logged(&log),
            vec!["tree.slow()"],
            "not invoked a second time before the extension"
        );
        run.resume(Resume::Extend {
            extend: BTreeMap::from([(BudgetKey::Minutes, 100.0)]),
        })
        .expect("extends");
        let outputs = done_outputs(&run.next().expect("finishes"));
        assert_eq!(outputs["r"], Value::Text("ok".into()));
        assert_eq!(logged(&log), vec!["tree.slow()", "tree.slow()"]);
    }
}
