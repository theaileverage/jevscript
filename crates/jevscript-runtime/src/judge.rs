//! Running one judgment alone (spec section 11.3).
//!
//! `Program.judgment(name).run(state)` performs exactly one Jev request over
//! the judgment's parameters and returns the answers, with the lines its
//! `log`s wrote (spec section 5.8). It needs no bindings, no
//! recording and no run: this is the path for evaluating questions against
//! labelled examples, and what `jevscript judge`, `jevscript eval` and the
//! `judgment.run` JSON-RPC method call.

use std::collections::BTreeMap;

use jevscript_ir::Ir;

use crate::answer::{self, Draw};
use crate::error::{RuntimeError, RuntimeErrorCode};
use crate::eval::Env;
use crate::jev::{JevAnswer, JevClient, JevRequest};
use crate::profile::Profile;
use crate::record::LogRecord;
use crate::rng::Rng;
use crate::state::{self, Ask};
use crate::value::Value;

/// What running a judgment alone produced.
#[derive(Debug, Clone, PartialEq)]
pub struct JudgmentOutcome {
    /// One answer per question, as Jev gave them. This is what the JSON-RPC
    /// method answers with.
    pub answers: Vec<JevAnswer>,
    /// The answers as values, keyed by result name (spec section 6.7).
    pub values: BTreeMap<String, Value>,
    /// The requests that were sent: one, unless an `each` had to be split at
    /// the profile's question cap (spec section 6.5).
    pub requests: Vec<JevRequest>,
    /// Whether a label or level was drawn rather than taken as the argmax,
    /// because a result carried `sample true` (spec section 6.11).
    pub sampled: bool,
    /// The judgment's `log` lines, in the order they ran (spec section 5.8).
    /// There is no run and no recording, so they are returned instead.
    pub logs: Vec<LogRecord>,
}

/// Run the judgment `name` of `ir` against `state`, whose keys are the
/// judgment's parameters, through `client`.
///
/// A result written with `sample true` draws its label or level from Jev's
/// distribution here too (spec section 6.11): the language form is not
/// overridden by running alone. The draws come from a fresh clock-seeded
/// source; [`run_judgment_with_draw`] takes the source instead. There is no
/// run, so no run-level `sample` option applies and nothing is recorded.
///
/// # Errors
///
/// `type_error` if the program has no such judgment or the state misses a
/// parameter, and whatever building or sending the request raises.
pub fn run_judgment(
    ir: &Ir,
    name: &str,
    state: &BTreeMap<String, serde_json::Value>,
    client: &dyn JevClient,
    profile: &Profile,
) -> Result<JudgmentOutcome, RuntimeError> {
    let mut rng = Rng::from_clock();
    let mut draw = || Ok(rng.next_f64());
    run_judgment_with_draw(ir, name, state, client, profile, &mut draw)
}

/// [`run_judgment`] with an explicit source of draws in `[0, 1)`, so that a
/// sampled result can be reproduced.
///
/// # Errors
///
/// As [`run_judgment`], plus whatever `draw` raises.
pub fn run_judgment_with_draw(
    ir: &Ir,
    name: &str,
    state: &BTreeMap<String, serde_json::Value>,
    client: &dyn JevClient,
    profile: &Profile,
    draw: &mut Draw<'_>,
) -> Result<JudgmentOutcome, RuntimeError> {
    let judgment = ir.judgment(name).ok_or_else(|| {
        RuntimeError::new(
            RuntimeErrorCode::TypeError,
            format!("`{}` declares no judgment `{name}`", ir.program),
        )
    })?;
    let mut env = Env::new();
    for param in &judgment.params {
        let value = state.get(param).ok_or_else(|| {
            RuntimeError::new(
                RuntimeErrorCode::TypeError,
                format!("judgment `{name}` needs `{param}` in the state"),
            )
        })?;
        env.define(param.clone(), Value::from_json(value));
    }
    let asks: Vec<Ask<'_>> = judgment
        .results
        .iter()
        .map(|result| Ask {
            name: result.name.as_str(),
            judge: &result.question,
        })
        .collect();
    // Spec 6.9: a judgment block's state is exactly its parameters, in full.
    let plan = state::build_judgment_request(&asks, &judgment.params, &mut env, profile)?;
    let mut answers = Vec::new();
    for request in &plan.requests {
        let response = client.send(request)?;
        answers.extend(response.answers);
    }
    let answered = answer::answers_to_values(&plan, &answers, false, draw)?;
    // Log lines run after the answers, each seeing the results above it,
    // with the permitted effects of a run but nothing recorded (spec 5.8).
    let logs = crate::interp::standalone_judgment_logs(
        ir,
        profile,
        client,
        judgment,
        &mut env,
        &answered.values,
    )?;
    Ok(JudgmentOutcome {
        answers,
        values: answered.values,
        requests: plan.requests,
        sampled: answered.sampled,
        logs,
    })
}
