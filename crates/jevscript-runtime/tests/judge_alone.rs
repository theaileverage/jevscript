//! Running a judgment alone (spec sections 6.9, 6.11 and 11.3).

mod support;

use std::collections::BTreeMap;

use jevscript_runtime::{RuntimeError, Value, run_judgment, run_judgment_with_draw};
use support::*;

fn triage_program() -> jevscript_ir::Ir {
    let mut ir = program(Vec::new(), &[], vec![]);
    ir.judgments.push(judgment(
        "triage",
        &["message", "context"],
        vec![
            (
                "owner",
                sampled(pick("message", &[("code", "a bug"), ("billing", "money")])),
            ),
            ("urgent", feels("message", "needs a reply now")),
        ],
    ));
    batch(&mut ir);
    ir
}

fn state() -> BTreeMap<String, serde_json::Value> {
    BTreeMap::from([
        (
            "message".to_string(),
            serde_json::json!("I was charged twice"),
        ),
        ("context".to_string(), serde_json::json!({"plan": "pro"})),
    ])
}

#[test]
fn a_judgment_run_alone_sends_every_parameter_in_full() {
    // Spec 6.9 and 11.3: one request whose state is exactly the parameters,
    // `context` included although no question reads it.
    let client = Answers::default()
        .label("owner", "billing", 0.9)
        .prob("urgent", 0.7)
        .client();
    let mut draw = || Ok(0.0);
    let outcome = run_judgment_with_draw(
        &triage_program(),
        "triage",
        &state(),
        &client,
        &tiny_profile(),
        &mut draw,
    )
    .expect("runs");
    assert_eq!(outcome.requests.len(), 1);
    assert_eq!(
        serde_json::to_value(&outcome.requests[0].state).expect("json"),
        serde_json::json!({"message": "I was charged twice", "context": {"plan": "pro"}})
    );
    assert_eq!(outcome.answers.len(), 2);
    assert_eq!(outcome.values["urgent"], Value::Prob(0.7));
    let missing = state()
        .into_iter()
        .filter(|(k, _)| k == "message")
        .collect();
    let error = run_judgment_with_draw(
        &triage_program(),
        "triage",
        &missing,
        &client,
        &tiny_profile(),
        &mut draw,
    )
    .expect_err("a missing parameter");
    assert!(error.message.contains("`context`"), "{}", error.message);
}

#[test]
fn a_standalone_sample_true_draws_from_the_distribution() {
    // Spec 6.11: `sample true` on a result always samples that judgment, run
    // alone as much as in a task. The draw is injectable, so the label it
    // lands on is fixed; a real source is used when none is given.
    let client = Answers::default()
        .label("owner", "code", 0.1)
        .prob("urgent", 0.2)
        .client();
    // The fake spreads the mass evenly and adds the confidence to the argmax:
    // code 0.4, billing 0.3, other 0.3 in declared order, so 0.99 lands on
    // `other` and 0.0 on `code`.
    let mut high = || Ok(0.99);
    let outcome = run_judgment_with_draw(
        &triage_program(),
        "triage",
        &state(),
        &client,
        &tiny_profile(),
        &mut high,
    )
    .expect("runs");
    assert!(outcome.sampled);
    let Value::Choice(owner) = &outcome.values["owner"] else {
        panic!("a choice");
    };
    assert_eq!(owner.label, "other");
    assert_eq!(owner.confidence, 0.1, "sampling leaves confidence alone");
    let mut low = || Ok(0.0);
    let outcome = run_judgment_with_draw(
        &triage_program(),
        "triage",
        &state(),
        &client,
        &tiny_profile(),
        &mut low,
    )
    .expect("runs");
    let Value::Choice(owner) = &outcome.values["owner"] else {
        panic!("a choice");
    };
    assert_eq!(owner.label, "code");

    // With the runtime's own source the request still succeeds and lands on
    // one of the labels: no `type_error`, no argmax override.
    let outcome = run_judgment(
        &triage_program(),
        "triage",
        &state(),
        &client,
        &tiny_profile(),
    )
    .expect("runs with a real draw");
    assert!(outcome.sampled);
    let Value::Choice(owner) = &outcome.values["owner"] else {
        panic!("a choice");
    };
    assert!(
        ["code", "billing", "other"].contains(&owner.label.as_str()),
        "{}",
        owner.label
    );
    let _: Result<_, RuntimeError> = run_judgment(
        &triage_program(),
        "nope",
        &state(),
        &client,
        &tiny_profile(),
    );
}
