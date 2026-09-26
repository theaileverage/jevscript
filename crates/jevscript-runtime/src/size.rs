//! The request size check (spec section 6.10).
//!
//! Before a request is sent, the runtime estimates the tokens of its state and
//! of each question. If the state plus all questions exceed the profile's
//! `total_tokens`, or the state plus the largest question exceed
//! `state_plus_question_tokens`, the run pauses with `state_too_large` naming
//! the largest subject. Both limits come from the [`Profile`]; nothing here is
//! a number.

use crate::error::{RuntimeError, RuntimeErrorCode};
use crate::jev::JevRequest;
use crate::profile::Profile;
use crate::tokens::{TokenEstimator, estimator_for};

/// How big a request is, in the profile's estimated tokens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestSize {
    /// The state's JSON.
    pub state_tokens: u64,
    /// Each question's JSON, in request order.
    pub question_tokens: Vec<u64>,
    /// The subject path whose value costs the most tokens, and that cost. This
    /// is what a `state_too_large` error names.
    pub largest_subject: Option<(String, u64)>,
}

impl RequestSize {
    /// The state plus every question.
    pub fn total(&self) -> u64 {
        self.state_tokens + self.question_tokens.iter().sum::<u64>()
    }

    /// The state plus the largest single question.
    pub fn state_plus_largest_question(&self) -> u64 {
        self.state_tokens + self.question_tokens.iter().copied().max().unwrap_or(0)
    }
}

/// Measure `request` with the profile's estimator.
pub fn measure(request: &JevRequest, profile: &Profile) -> RequestSize {
    let estimator = estimator_for(&profile.tokenizer);
    measure_with(request, estimator.as_ref())
}

/// Measure `request` with an explicit estimator.
pub fn measure_with(request: &JevRequest, estimator: &dyn TokenEstimator) -> RequestSize {
    let state_json = serde_json::to_string(&request.state).unwrap_or_default();
    let question_tokens = request
        .questions
        .iter()
        .map(|q| estimator.estimate(&serde_json::to_string(q).unwrap_or_default()))
        .collect();
    let largest_subject = request
        .state
        .iter()
        .map(|(root, value)| (root.clone(), estimator.estimate(&value.to_string())))
        .max_by_key(|(_, tokens)| *tokens);
    RequestSize {
        state_tokens: estimator.estimate(&state_json),
        question_tokens,
        largest_subject,
    }
}

/// The section 6.10 check: `state_too_large` if the request is over either of
/// the profile's limits, naming the largest subject.
///
/// # Errors
///
/// [`RuntimeErrorCode::StateTooLarge`], which is not retryable: the program
/// must `shape` or `focus` the state down (spec section 7.2).
pub fn check(size: &RequestSize, profile: &Profile) -> Result<(), RuntimeError> {
    let largest = size.largest_subject.as_ref().map_or_else(
        || "(no subject)".to_string(),
        |(path, tokens)| format!("`{path}` ({tokens} tokens)"),
    );
    let total = size.total();
    if total > profile.total_tokens {
        return Err(RuntimeError::new(
            RuntimeErrorCode::StateTooLarge,
            format!(
                "the state plus all questions is {total} tokens, over the {} limit of `{}`; the largest subject is {largest}",
                profile.total_tokens, profile.model
            ),
        ));
    }
    let per_question = size.state_plus_largest_question();
    if per_question > profile.state_plus_question_tokens {
        return Err(RuntimeError::new(
            RuntimeErrorCode::StateTooLarge,
            format!(
                "the state plus the largest question is {per_question} tokens, over the {} per-question limit of `{}`; the largest subject is {largest}",
                profile.state_plus_question_tokens, profile.model
            ),
        ));
    }
    Ok(())
}

/// Measure and check in one call.
///
/// # Errors
///
/// As [`check`].
pub fn check_request(request: &JevRequest, profile: &Profile) -> Result<RequestSize, RuntimeError> {
    let size = measure(request, profile);
    check(&size, profile)?;
    Ok(size)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jev::Question;
    use crate::profile::Tokenizer;
    use serde_json::json;
    use std::collections::BTreeMap;

    /// A tiny profile so the checks fire on small fixtures. Every number here
    /// is test data, not a limit the runtime knows.
    fn tiny_profile(total: u64, per_question: u64) -> Profile {
        Profile {
            model: "jev-tiny".into(),
            endpoint: "http://localhost".into(),
            total_tokens: total,
            state_plus_question_tokens: per_question,
            max_questions_per_request: 4,
            max_criteria_per_question: 4,
            tokenizer: Tokenizer::Chars4,
            price_per_million_input_usd: 1.0,
            price_per_million_output_usd: 0.0,
            aliases: None,
        }
    }

    fn request() -> JevRequest {
        JevRequest {
            state: BTreeMap::from([
                ("small".to_string(), json!("x")),
                ("big".to_string(), json!("a".repeat(400))),
            ]),
            model: "jev-tiny".into(),
            questions: vec![
                Question::Noul {
                    id: "a".into(),
                    path: "big".into(),
                    condition: "is long".into(),
                    instruction: None,
                },
                Question::Noul {
                    id: "b".into(),
                    path: "small".into(),
                    condition: "is short and this question is the longer of the two".into(),
                    instruction: None,
                },
            ],
        }
    }

    #[test]
    fn a_request_under_both_limits_passes() {
        let size = check_request(&request(), &tiny_profile(100_000, 100_000)).expect("fits");
        assert!(size.state_tokens >= 100);
        assert_eq!(size.question_tokens.len(), 2);
    }

    #[test]
    fn over_total_tokens_names_the_largest_subject() {
        // Spec 6.10: state plus all questions over the total limit.
        let size = measure(&request(), &tiny_profile(1, 1));
        let error = check(&size, &tiny_profile(size.total() - 1, 100_000)).expect_err("too large");
        assert_eq!(error.code, RuntimeErrorCode::StateTooLarge);
        assert!(!error.retryable);
        assert!(error.message.contains("`big`"), "{}", error.message);
        assert!(error.message.contains("all questions"), "{}", error.message);
    }

    #[test]
    fn over_state_plus_question_tokens_names_the_largest_subject() {
        // Spec 6.10: state plus the largest question over the per-question
        // limit, even when the total limit is fine.
        let size = measure(&request(), &tiny_profile(1, 1));
        let error = check(
            &size,
            &tiny_profile(100_000, size.state_plus_largest_question() - 1),
        )
        .expect_err("too large");
        assert_eq!(error.code, RuntimeErrorCode::StateTooLarge);
        assert!(error.message.contains("`big`"), "{}", error.message);
        assert!(
            error.message.contains("largest question"),
            "{}",
            error.message
        );
        // And exactly at the limit it passes.
        check(
            &size,
            &tiny_profile(100_000, size.state_plus_largest_question()),
        )
        .expect("fits");
    }
}
