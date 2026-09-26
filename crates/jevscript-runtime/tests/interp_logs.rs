//! `log` (spec section 5.8), compiled by the real compiler and run under
//! scripted Jev answers: what it records, what it returns, what it never
//! touches, and how recording, replay and redaction treat it.
//!
//! Each test names the rule it checks.

mod support;

use std::collections::BTreeMap;

use jevscript_compiler::compile_source;
use jevscript_ir::{Ir, LogLevel};
use jevscript_runtime::record::{Event, JsonlReplayer, Recorded, Redaction, Replayer};
use jevscript_runtime::run::{Run, RunOptions};
use jevscript_runtime::{Pause, RuntimeErrorCode, Value, run_judgment};
use support::*;

fn compiled(source: &str) -> Ir {
    compile_source(source).unwrap_or_else(|diagnostics| panic!("compiles: {diagnostics:?}"))
}

fn done_outputs(pause: &Pause) -> BTreeMap<String, Value> {
    match pause {
        Pause::Done { outputs, .. } => outputs.clone(),
        other => panic!("expected done, got {other:?}"),
    }
}

fn message_input(message: &str) -> BTreeMap<String, Value> {
    BTreeMap::from([("message".to_string(), text_value(message))])
}

/// Level, message, field names and task of every `log` event, in order.
fn logs(events: &[Event]) -> Vec<(LogLevel, String, Vec<String>, String)> {
    events
        .iter()
        .filter_map(|event| match event {
            Event::Log {
                level,
                message,
                fields,
                task,
                ..
            } => Some((
                *level,
                message
                    .full()
                    .cloned()
                    .unwrap_or_else(|| "<redacted>".into()),
                fields.keys().cloned().collect(),
                task.clone(),
            )),
            _ => None,
        })
        .collect()
}

fn log_fields(events: &[Event], index: usize) -> BTreeMap<String, Value> {
    let Event::Log { fields, .. } = events_of(events, "log")[index] else {
        unreachable!()
    };
    fields
        .iter()
        .map(|(name, value)| (name.clone(), value.full().cloned().expect("full")))
        .collect()
}

const ROUTE: &str = r#"program route

in message: text
out n
out shown

task main:
  log info "routed request" { message, size: len(message) }
  n = log debug len(message)
  shown = [log warn x for x in [1, 2]]
"#;

#[test]
fn a_log_records_its_level_message_and_fields_and_evaluates_to_its_value() {
    // Spec 5.8: the message is the value's text form, the fields are
    // evaluated in place, and the expression is the value itself.
    let dir = temp_dir("log-basic");
    let options = RunOptions {
        inputs: message_input("hello"),
        ..tiny_profile_options(&dir)
    };
    let mut run = start(
        compiled(ROUTE),
        options,
        bindings(Vec::new()),
        Answers::default(),
    );
    let outputs = done_outputs(&run.next().expect("steps"));
    assert_eq!(outputs["n"], number_value(5.0), "`log` returns its value");
    assert_eq!(
        outputs["shown"],
        Value::List(vec![number_value(1.0), number_value(2.0)]),
        "a log inside a comprehension returns each item"
    );
    let events: Vec<Event> = run.drain_events().into_iter().map(|e| e.event).collect();
    assert_eq!(
        logs(&events),
        vec![
            (
                LogLevel::Info,
                "routed request".to_string(),
                vec!["message".to_string(), "size".to_string()],
                "main".to_string()
            ),
            (LogLevel::Debug, "5".to_string(), vec![], "main".to_string()),
            (LogLevel::Warn, "1".to_string(), vec![], "main".to_string()),
            (LogLevel::Warn, "2".to_string(), vec![], "main".to_string()),
        ]
    );
    assert_eq!(
        log_fields(&events, 0),
        BTreeMap::from([
            ("message".to_string(), text_value("hello")),
            ("size".to_string(), number_value(5.0)),
        ])
    );
}

#[test]
fn a_log_never_reaches_jev_never_splits_a_request_and_costs_no_budget() {
    // Spec 5.8: a log is not a judgment, a call, a step or a pause. Two
    // judgments around it are still one request (6.6), the log's own values
    // are not in the state (6.9), and `budget calls 1` is not exceeded.
    let source = r#"program triage

in message: text
in secret: text
out a
out b

task main budget calls 1:
  a = message feels "is urgent"
  log info "between" { a, secret }
  b = message feels "is polite"
  loop max 20:
    log debug secret
"#;
    let dir = temp_dir("log-no-jev");
    let mut inputs = message_input("hello");
    inputs.insert("secret".into(), text_value("hunter2"));
    let options = RunOptions {
        inputs,
        ..tiny_profile_options(&dir)
    };
    let mut run = start(
        compiled(source),
        options,
        bindings(Vec::new()),
        Answers::default().prob("a", 0.8).prob("b", 0.3),
    );
    let pause = run.next().expect("steps");
    assert!(matches!(pause, Pause::Done { .. }), "{pause:?}");
    assert_eq!(run.usage().calls, 1, "one request; logs are free");
    let events: Vec<Event> = run.drain_events().into_iter().map(|e| e.event).collect();
    let requests = events_of(&events, "request");
    assert_eq!(requests.len(), 1, "the log does not split the group");
    let Event::Request {
        state, questions, ..
    } = requests[0]
    else {
        unreachable!()
    };
    assert_eq!(questions.len(), 2);
    let state = state.full().expect("full");
    assert_eq!(state.keys().collect::<Vec<_>>(), vec!["message"]);
    assert!(
        !serde_json::to_string(state)
            .expect("json")
            .contains("hunter2"),
        "nothing a log names reaches Jev"
    );
    assert_eq!(events_of(&events, "log").len(), 21);
    let position = |kind: &str| {
        events
            .iter()
            .position(|e| serde_json::to_value(e).expect("json")["event"] == kind)
            .expect("present")
    };
    assert!(
        position("answers") < position("log"),
        "the log reads `a`, so it runs after the answers"
    );
}

#[test]
fn a_judgment_blocks_logs_run_after_its_answers_and_see_the_results_above_them() {
    // Spec 5.8 and 6.7: a judgment is still one request over its parameters;
    // its log lines run once the answers are in, in written order.
    let source = r#"program triage

in message: text
out urgent

judgment classify(message):
  log debug "asking" { message }
  urgent = message feels "is urgent"
  log info "answered" { urgent }

task main:
  j = classify(message)
  urgent = j.urgent
"#;
    let dir = temp_dir("log-judgment");
    let options = RunOptions {
        inputs: message_input("hello"),
        ..tiny_profile_options(&dir)
    };
    let ir = compiled(source);
    assert_eq!(ir.judgments[0].logs.len(), 2);
    let mut run = start(
        ir,
        options,
        bindings(Vec::new()),
        Answers::default().prob("urgent", 0.8),
    );
    let outputs = done_outputs(&run.next().expect("steps"));
    assert_eq!(outputs["urgent"], Value::Prob(0.8));
    let events: Vec<Event> = run.drain_events().into_iter().map(|e| e.event).collect();
    let kinds: Vec<String> = events
        .iter()
        .map(|e| {
            serde_json::to_value(e).expect("json")["event"]
                .as_str()
                .unwrap_or("")
                .to_string()
        })
        .filter(|kind| matches!(kind.as_str(), "request" | "answers" | "log"))
        .collect();
    assert_eq!(kinds, vec!["request", "answers", "log", "log"]);
    assert_eq!(
        logs(&events)
            .iter()
            .map(|(_, message, _, task)| (message.as_str(), task.as_str()))
            .collect::<Vec<_>>(),
        vec![("asking", "main"), ("answered", "main")]
    );
    assert_eq!(log_fields(&events, 1)["urgent"], Value::Prob(0.8));
}

#[test]
fn a_standalone_judgment_returns_its_logs() {
    // Spec 5.8 and 11.3: a judgment run alone has no recording, so it returns
    // the lines its logs wrote, named after the judgment.
    let source = r#"program triage

judgment classify(message):
  urgent = message feels "is urgent"
  log warn "answered" { urgent }
"#;
    let ir = compiled(source);
    let state = BTreeMap::from([("message".to_string(), serde_json::json!("hello"))]);
    let client = Answers::default().prob("urgent", 0.7).client();
    let outcome = run_judgment(&ir, "classify", &state, &client, &tiny_profile()).expect("runs");
    assert_eq!(outcome.logs.len(), 1);
    let log = &outcome.logs[0];
    assert_eq!(log.level, LogLevel::Warn);
    assert_eq!(log.message, "answered");
    assert_eq!(log.task, "classify");
    assert_eq!(log.fields["urgent"], Value::Prob(0.7));
}

#[test]
fn a_machine_logs_from_observe_guards_and_actions_under_its_machine_name() {
    // Spec 5.8 and 7.8: a log is allowed in every unit and is reported under
    // the machine, as a pause raised there would be.
    let source = r#"program flow

out finished

machine m(x):
  goal "finish"
  observe:
    value log debug x, max 10
  state start:
    on go "ready to go" -> finished when log info x > 0:
      log warn "going" { x }
  state finished done

task main:
  r = m(1, max 3)
  finished = r.done
"#;
    let dir = temp_dir("log-machine");
    let mut run = start(
        compiled(source),
        tiny_profile_options(&dir),
        bindings(Vec::new()),
        Answers::default().label("event", "go", 0.95),
    );
    let outputs = done_outputs(&run.next().expect("steps"));
    assert_eq!(outputs["finished"], Value::Bool(true));
    let events: Vec<Event> = run.drain_events().into_iter().map(|e| e.event).collect();
    assert_eq!(
        logs(&events),
        vec![
            (LogLevel::Debug, "1".into(), vec![], "machine:m".into()),
            (LogLevel::Info, "true".into(), vec![], "machine:m".into()),
            (
                LogLevel::Warn,
                "going".into(),
                vec!["x".into()],
                "machine:m".into()
            ),
        ]
    );
}

fn record_route(
    dir: &std::path::Path,
    redaction: Redaction,
) -> (Ir, std::path::PathBuf, Vec<Event>) {
    let recording = dir.join("run.jsonl");
    let ir = compiled(ROUTE);
    let options = RunOptions {
        inputs: message_input("the secret text"),
        record: Some(recording.clone()),
        redaction,
        ..tiny_profile_options(dir)
    };
    let mut run = start(
        ir.clone(),
        options,
        bindings(Vec::new()),
        Answers::default(),
    );
    assert!(matches!(run.next().expect("steps"), Pause::Done { .. }));
    let streamed = run.drain_events().into_iter().map(|e| e.event).collect();
    (ir, recording, streamed)
}

fn replay_of(ir: Ir, dir: &std::path::Path, recording: std::path::PathBuf) -> Run {
    let options = RunOptions {
        replay: Some(recording),
        ..tiny_profile_options(dir)
    };
    Run::create(ir, "main", options, bindings(Vec::new()))
        .expect("a replay needs no bindings")
        .with_client(Box::new(NeverJev))
}

#[test]
fn replay_reproduces_logs_without_streaming_them_to_the_host_again() {
    // Spec 5.8 and 10.4: every log is in the recording; replay checks each
    // one where it was written and does not emit it to the host a second time.
    let dir = temp_dir("log-replay");
    let (ir, recording, streamed) = record_route(&dir, Redaction::Full);
    assert_eq!(
        events_of(&streamed, "log").len(),
        4,
        "live logs stream once"
    );

    let mut replay = replay_of(ir, &dir, recording);
    let pause = replay.next().expect("replays");
    assert!(matches!(pause, Pause::Done { .. }), "{pause:?}");
    let replayed: Vec<Event> = replay.drain_events().into_iter().map(|e| e.event).collect();
    assert!(
        events_of(&replayed, "log").is_empty(),
        "a replayed log is not re-emitted: {replayed:?}"
    );
    assert_eq!(
        logs(replay.log()),
        logs(&streamed),
        "the replay's log holds exactly the recorded lines"
    );
}

#[test]
fn a_log_that_differs_on_replay_is_replay_diverged() {
    // Spec 5.8 and 10.4: a log's level, message and fields are its identity.
    let dir = temp_dir("log-diverged");
    let (ir, recording, _) = record_route(&dir, Redaction::Full);
    let text = std::fs::read_to_string(&recording).expect("reads");
    assert!(text.contains("\"routed request\""));
    std::fs::write(
        &recording,
        text.replace("\"routed request\"", "\"tampered\""),
    )
    .expect("writes");
    let mut replay = replay_of(ir, &dir, recording);
    let pause = replay.next().expect("steps");
    let Pause::Error { code, message, .. } = &pause else {
        panic!("expected replay_diverged, got {pause:?}");
    };
    assert_eq!(*code, RuntimeErrorCode::ReplayDiverged);
    assert!(message.contains("`info` log"), "{message}");
}

#[test]
fn a_redacted_recording_hides_log_messages_and_fields_and_still_replays() {
    // Spec 5.8 and 10.3: log values may carry state or observations, so a
    // redacted primary stores each as a marker, the private companion keeps
    // them, and replay is exact.
    let dir = temp_dir("log-redacted");
    let (ir, recording, streamed) = record_route(&dir, Redaction::Redact);
    let text = std::fs::read_to_string(&recording).expect("reads");
    for line in text
        .lines()
        .filter(|line| line.contains(r#""event":"log""#))
    {
        assert!(!line.contains("the secret text"), "{line}");
    }
    let mut primary = Vec::new();
    let mut reader = JsonlReplayer::new(&recording);
    while let Some(line) = reader.next().expect("reads") {
        primary.push(line.event);
    }
    let Event::Log {
        level,
        message,
        fields,
        ..
    } = events_of(&primary, "log")[0]
    else {
        unreachable!()
    };
    assert_eq!(*level, LogLevel::Info, "the level stays visible");
    assert!(message.is_redacted());
    assert!(
        fields.keys().eq(["message", "size"]),
        "field names stay visible"
    );
    assert!(fields.values().all(Recorded::is_redacted));
    assert_eq!(
        log_fields(&streamed, 0)["message"],
        text_value("the secret text"),
        "the host's live stream carries logs in full, as it does requests"
    );

    let mut replay = replay_of(ir, &dir, recording);
    let pause = replay.next().expect("replays");
    assert!(matches!(pause, Pause::Done { .. }), "{pause:?}");
    assert_eq!(logs(replay.log()), logs(&streamed));
}

#[test]
fn resuming_after_a_pause_does_not_log_the_same_line_twice() {
    // Spec 5.8 and 10.4: a resumed run walks from the start against its own
    // log; the lines before the pause are served back, not written again.
    let source = r#"program ask

out answer

needs me: person

task main:
  log info "before"
  reply = me.ask "Continue?"
  answer = log info reply.answer
"#;
    let dir = temp_dir("log-resume");
    let recording = dir.join("run.jsonl");
    let options = RunOptions {
        record: Some(recording.clone()),
        ..tiny_profile_options(&dir)
    };
    let log = call_log();
    let mut run = start(
        compiled(source),
        options,
        bindings(vec![("me", Box::new(FakePerson::new("me", &log)))]),
        Answers::default(),
    );
    let pauses = drive(&mut run, |pause| match pause {
        Pause::Confirm { .. } => Some(jevscript_runtime::Resume::Answer {
            answer: "yes".into(),
            text: None,
        }),
        _ => None,
    });
    assert!(
        matches!(pauses.last(), Some(Pause::Done { .. })),
        "{pauses:?}"
    );
    let streamed: Vec<Event> = run.drain_events().into_iter().map(|e| e.event).collect();
    let messages: Vec<String> = logs(&streamed).into_iter().map(|l| l.1).collect();
    assert_eq!(messages, vec!["before", "yes"]);
    let text = std::fs::read_to_string(&recording).expect("reads");
    assert_eq!(
        text.lines()
            .filter(|line| line.contains(r#""event":"log""#))
            .count(),
        2
    );
}

#[test]
fn a_standalone_judgments_logs_evaluate_defs_and_live_clock_and_random_reads() {
    // Spec 5.8 and 11.3: a log line is outside the question, so alone as in a
    // run it may call defs (which may judge) and read `now()` and `random()`.
    // There is no recording, so the reads are live.
    let source = r#"program triage

def ident(x):
  return x

def polite(m):
  p = m feels "is polite"
  return p

judgment classify(message):
  log info now()
  urgent = message feels "is urgent"
  log debug ident(message) { r: random(), polite: polite(message) }
"#;
    let ir = compiled(source);
    let state = BTreeMap::from([("message".to_string(), serde_json::json!("hello"))]);
    let client = Answers::default()
        .prob("urgent", 0.7)
        .prob("p", 0.4)
        .client();
    let outcome = run_judgment(&ir, "classify", &state, &client, &tiny_profile()).expect("runs");
    assert_eq!(
        outcome.requests.len(),
        1,
        "the judgment itself is one request"
    );
    assert_eq!(outcome.logs.len(), 2);
    let clock = &outcome.logs[0].message;
    assert!(
        jevscript_runtime::record::iso8601_to_unix_millis(clock).is_ok(),
        "`now()` read the clock: {clock}"
    );
    let line = &outcome.logs[1];
    assert_eq!(line.message, "hello", "`ident` ran");
    assert_eq!(line.task, "classify");
    let Value::Number(r) = line.fields["r"] else {
        panic!("random() is a number: {:?}", line.fields["r"]);
    };
    assert!((0.0..1.0).contains(&r));
    assert_eq!(
        line.fields["polite"],
        Value::Prob(0.4),
        "a def that judges asks through the same client"
    );

    let only_log = compiled("program t\n\njudgment j(x):\n  log info now()\n");
    let outcome = run_judgment(
        &only_log,
        "j",
        &BTreeMap::from([("x".to_string(), serde_json::json!(1))]),
        &NeverJev,
        &tiny_profile(),
    )
    .expect("a judgment with only a log line runs");
    assert_eq!(outcome.logs.len(), 1);
}

#[test]
fn a_program_with_its_own_log_unit_runs_as_it_did_before_log_existed() {
    // Spec 5.8: with `def log(info)` in the file, `log info x` is a
    // command-form call to it with `info = x`, so `y` is 4 and nothing is
    // logged.
    let source = "program p\nout y\ndef log(info):\n  return info + 1\ntask main:\n  x = 3\n  y = log info x\n";
    let dir = temp_dir("log-unit");
    let mut run = start(
        compiled(source),
        tiny_profile_options(&dir),
        bindings(Vec::new()),
        Answers::default(),
    );
    let outputs = done_outputs(&run.next().expect("steps"));
    assert_eq!(outputs["y"], number_value(4.0));
    let events: Vec<Event> = run.drain_events().into_iter().map(|e| e.event).collect();
    assert!(events_of(&events, "log").is_empty());
}
