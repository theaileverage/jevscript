//! Jev answers to values, and sampling (spec sections 6.2 to 6.5 and 6.11).
//!
//! A Noul becomes a `prob`, a Choice a `choice` (with `index` and `item` for a
//! `pick among`), a Score a `level` whose `level` is the nearest whole index,
//! `score` the probability-weighted mean and `normalized` that score over
//! `levels - 1`. An `each` collects its per-element answers into a list in
//! element order.
//!
//! Sampling draws the `pick` label or `rate` level from Jev's distribution
//! instead of taking the argmax, per judgment (`sample true`) or per run. The
//! draw comes from the caller so that it goes through the run's recorded random
//! source; `probabilities` and `confidence` are left exactly as Jev gave them.
//! `feels` never samples.

use std::collections::BTreeMap;

use crate::error::{RuntimeError, RuntimeErrorCode};
use crate::jev::JevAnswer;
use crate::state::{Plan, PlannedAsk, PlannedKind};
use crate::value::{Choice, Level, Value};

/// The values a plan's answers became.
#[derive(Debug, Clone, PartialEq)]
pub struct Answered {
    /// One value per ask, keyed by result name.
    pub values: BTreeMap<String, Value>,
    /// Whether any label or level was drawn rather than taken as the argmax,
    /// which the `answers` event records as `sampled: true` (spec 6.11).
    pub sampled: bool,
}

/// A source of draws in `[0, 1)`. The interpreter passes its recorded random
/// source; replay passes the recorded `draw` events.
pub type Draw<'a> = dyn FnMut() -> Result<f64, RuntimeError> + 'a;

/// Turn the answers of every request in `plan` into values.
///
/// `sample_run` is the run's `sample` option (spec section 6.11); a judgment's
/// own `sample true` is in the plan.
///
/// # Errors
///
/// `jev_rejected` if an answer is missing or of the wrong kind for its
/// question, and whatever `draw` raises.
pub fn answers_to_values(
    plan: &Plan,
    answers: &[JevAnswer],
    sample_run: bool,
    draw: &mut Draw<'_>,
) -> Result<Answered, RuntimeError> {
    let by_id: BTreeMap<&str, &JevAnswer> = answers.iter().map(|a| (a.id(), a)).collect();
    let mut values = BTreeMap::new();
    let mut sampled = false;
    for ask in &plan.asks {
        let sample = sample_run || ask.sample;
        let mut converted = Vec::new();
        for id in ask.question_ids() {
            let answer = by_id.get(id.as_str()).ok_or_else(|| {
                RuntimeError::new(
                    RuntimeErrorCode::JevRejected,
                    format!("no answer for question `{id}`"),
                )
            })?;
            let (value, drew) = answer_to_value(ask, answer, sample, draw)?;
            sampled |= drew;
            converted.push(value);
        }
        let value = if ask.each.is_some() {
            Value::List(converted)
        } else {
            converted.pop().expect("one question")
        };
        values.insert(ask.name.clone(), value);
    }
    Ok(Answered { values, sampled })
}

/// One answer to one value; the flag says whether a draw was taken.
fn answer_to_value(
    ask: &PlannedAsk,
    answer: &JevAnswer,
    sample: bool,
    draw: &mut Draw<'_>,
) -> Result<(Value, bool), RuntimeError> {
    let mismatch = |wanted: &str| {
        RuntimeError::new(
            RuntimeErrorCode::JevRejected,
            format!("question `{}` expected a {wanted} answer", answer.id()),
        )
    };
    match (&ask.kind, answer) {
        (PlannedKind::Feels, JevAnswer::Noul { prob, .. }) => Ok((Value::Prob(*prob), false)),
        (
            PlannedKind::Pick { labels },
            JevAnswer::Choice {
                label,
                confidence,
                probabilities,
                ..
            },
        ) => {
            let (label, drew) = maybe_draw(label, labels, probabilities, sample, draw)?;
            Ok((
                Value::Choice(Choice {
                    label,
                    confidence: *confidence,
                    probabilities: probabilities.clone(),
                    index: None,
                    item: None,
                }),
                drew,
            ))
        }
        (
            PlannedKind::PickAmong { labels, items },
            JevAnswer::Choice {
                label,
                confidence,
                probabilities,
                ..
            },
        ) => {
            let (label, drew) = maybe_draw(label, labels, probabilities, sample, draw)?;
            // Spec 6.4a: `label` is `"i<index>"` or `"none"`; `index` and
            // `item` follow it.
            let index = label
                .strip_prefix('i')
                .and_then(|digits| digits.parse::<usize>().ok())
                .filter(|i| *i < items.len());
            Ok((
                Value::Choice(Choice {
                    label,
                    confidence: *confidence,
                    probabilities: probabilities.clone(),
                    index: index.map(|i| i as u32),
                    item: index.map(|i| Box::new(items[i].clone())),
                }),
                drew,
            ))
        }
        (
            PlannedKind::Rate { levels, names },
            JevAnswer::Score {
                level,
                score,
                confidence,
                probabilities,
                ..
            },
        ) => {
            // Probabilities are keyed by name when the levels were named,
            // else by index as text; either way this is their order.
            let keys: Vec<String> = if names.is_empty() {
                (0..*levels).map(|i| i.to_string()).collect()
            } else {
                names.clone()
            };
            let (level, drew) = if sample {
                let key = draw_key(&keys, probabilities, draw()?);
                (
                    keys.iter()
                        .position(|k| *k == key)
                        .unwrap_or(*level as usize) as u32,
                    true,
                )
            } else {
                (*level, false)
            };
            let normalized = if *levels > 1 {
                score / (*levels as f64 - 1.0)
            } else {
                0.0
            };
            Ok((
                Value::Level(Level {
                    level,
                    score: *score,
                    normalized,
                    confidence: *confidence,
                    probabilities: probabilities.clone(),
                    names: names.clone(),
                }),
                drew,
            ))
        }
        (PlannedKind::Feels, _) => Err(mismatch("noul")),
        (PlannedKind::Pick { .. } | PlannedKind::PickAmong { .. }, _) => Err(mismatch("choice")),
        (PlannedKind::Rate { .. }, _) => Err(mismatch("score")),
    }
}

/// The argmax label, or a drawn one when sampling (spec section 6.11).
fn maybe_draw(
    label: &str,
    labels: &[String],
    probabilities: &BTreeMap<String, f64>,
    sample: bool,
    draw: &mut Draw<'_>,
) -> Result<(String, bool), RuntimeError> {
    if !sample {
        return Ok((label.to_string(), false));
    }
    Ok((draw_key(labels, probabilities, draw()?), true))
}

/// The key whose cumulative probability, walking `keys` in order, first
/// exceeds `u`. Probabilities are normalised so a distribution that does not
/// sum to one still draws every key with its share; a key Jev did not report
/// has probability zero and is never drawn.
pub fn draw_key(keys: &[String], probabilities: &BTreeMap<String, f64>, u: f64) -> String {
    let total: f64 = keys
        .iter()
        .map(|k| probabilities.get(k).copied().unwrap_or(0.0).max(0.0))
        .sum();
    if total <= 0.0 {
        return keys.first().cloned().unwrap_or_default();
    }
    let mut cumulative = 0.0;
    for key in keys {
        cumulative += probabilities.get(key).copied().unwrap_or(0.0).max(0.0) / total;
        if u < cumulative {
            return key.clone();
        }
    }
    keys.last().cloned().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jev::JevRequest;
    use crate::rng::Rng;

    fn probs(pairs: &[(&str, f64)]) -> BTreeMap<String, f64> {
        pairs.iter().map(|(k, p)| (k.to_string(), *p)).collect()
    }

    fn plan(asks: Vec<PlannedAsk>) -> Plan {
        Plan {
            requests: vec![JevRequest {
                state: BTreeMap::new(),
                model: "jev-test".into(),
                questions: vec![],
            }],
            asks,
        }
    }

    fn ask(name: &str, kind: PlannedKind, each: Option<usize>, sample: bool) -> PlannedAsk {
        PlannedAsk {
            name: name.into(),
            kind,
            each,
            sample,
        }
    }

    fn no_draw() -> Box<Draw<'static>> {
        Box::new(|| panic!("no draw should be taken"))
    }

    #[test]
    fn a_noul_is_a_prob_and_each_collects_a_list_in_order() {
        // Spec 6.2 and 6.5.
        let plan = plan(vec![
            ask("p", PlannedKind::Feels, None, false),
            ask("ps", PlannedKind::Feels, Some(3), false),
        ]);
        let answers = vec![
            JevAnswer::Noul {
                id: "ps[2]".into(),
                prob: 0.3,
            },
            JevAnswer::Noul {
                id: "p".into(),
                prob: 0.9,
            },
            JevAnswer::Noul {
                id: "ps[0]".into(),
                prob: 0.1,
            },
            JevAnswer::Noul {
                id: "ps[1]".into(),
                prob: 0.2,
            },
        ];
        let answered = answers_to_values(&plan, &answers, true, &mut *no_draw()).expect("converts");
        assert!(!answered.sampled, "feels never samples");
        assert_eq!(answered.values["p"], Value::Prob(0.9));
        assert_eq!(
            answered.values["ps"],
            Value::List(vec![Value::Prob(0.1), Value::Prob(0.2), Value::Prob(0.3)])
        );
    }

    #[test]
    fn a_choice_keeps_the_argmax_without_sampling() {
        // Spec 6.3.
        let plan = plan(vec![ask(
            "c",
            PlannedKind::Pick {
                labels: vec!["stuck".into(), "other".into()],
            },
            None,
            false,
        )]);
        let answers = vec![JevAnswer::Choice {
            id: "c".into(),
            label: "stuck".into(),
            confidence: 0.6,
            probabilities: probs(&[("stuck", 0.8), ("other", 0.2)]),
        }];
        let answered =
            answers_to_values(&plan, &answers, false, &mut *no_draw()).expect("converts");
        assert_eq!(
            answered.values["c"],
            Value::Choice(Choice {
                label: "stuck".into(),
                confidence: 0.6,
                probabilities: probs(&[("stuck", 0.8), ("other", 0.2)]),
                index: None,
                item: None,
            })
        );
        assert!(!answered.sampled);
    }

    #[test]
    fn a_score_is_a_level_with_normalized_and_names() {
        // Spec 6.4: `normalized = score / (levels - 1)`, names kept for `is`.
        let plan = plan(vec![ask(
            "l",
            PlannedKind::Rate {
                levels: 3,
                names: vec!["a".into(), "b".into(), "c".into()],
            },
            None,
            false,
        )]);
        let answers = vec![JevAnswer::Score {
            id: "l".into(),
            level: 1,
            score: 1.2,
            confidence: 0.5,
            probabilities: probs(&[("a", 0.2), ("b", 0.4), ("c", 0.4)]),
        }];
        let answered =
            answers_to_values(&plan, &answers, false, &mut *no_draw()).expect("converts");
        let Value::Level(level) = &answered.values["l"] else {
            panic!()
        };
        assert_eq!(level.level, 1);
        assert_eq!(level.score, 1.2);
        assert!((level.normalized - 0.6).abs() < 1e-12);
        assert_eq!(level.names, vec!["a", "b", "c"]);
    }

    #[test]
    fn pick_among_carries_index_and_item_or_none() {
        // Spec 6.4a.
        let items = vec![Value::Text("first".into()), Value::Text("second".into())];
        let kind = PlannedKind::PickAmong {
            labels: vec!["i0".into(), "i1".into(), "none".into()],
            items,
        };
        let plan = plan(vec![
            ask("c", kind.clone(), None, false),
            ask("n", kind, None, false),
        ]);
        let answers = vec![
            JevAnswer::Choice {
                id: "c".into(),
                label: "i1".into(),
                confidence: 0.9,
                probabilities: probs(&[("i0", 0.05), ("i1", 0.9), ("none", 0.05)]),
            },
            JevAnswer::Choice {
                id: "n".into(),
                label: "none".into(),
                confidence: 0.9,
                probabilities: probs(&[("i0", 0.05), ("i1", 0.05), ("none", 0.9)]),
            },
        ];
        let answered =
            answers_to_values(&plan, &answers, false, &mut *no_draw()).expect("converts");
        let Value::Choice(chosen) = &answered.values["c"] else {
            panic!()
        };
        assert_eq!(chosen.label, "i1");
        assert_eq!(chosen.index, Some(1));
        assert_eq!(chosen.item.as_deref(), Some(&Value::Text("second".into())));
        let Value::Choice(none) = &answered.values["n"] else {
            panic!()
        };
        assert_eq!(none.label, "none");
        assert_eq!(none.index, None);
        assert_eq!(none.item, None);
    }

    #[test]
    fn sampling_draws_deterministically_and_leaves_probabilities_alone() {
        // Spec 6.11: a seeded run draws the same label every time, and
        // `probabilities` and `confidence` are untouched.
        let probabilities = probs(&[("a", 0.2), ("b", 0.5), ("c", 0.3)]);
        let plan = plan(vec![
            ask(
                "c",
                PlannedKind::Pick {
                    labels: vec!["a".into(), "b".into(), "c".into()],
                },
                None,
                false,
            ),
            ask(
                "l",
                PlannedKind::Rate {
                    levels: 3,
                    names: vec![],
                },
                None,
                false,
            ),
            ask("p", PlannedKind::Feels, None, false),
        ]);
        let answers = vec![
            JevAnswer::Choice {
                id: "c".into(),
                label: "b".into(),
                confidence: 0.25,
                probabilities: probabilities.clone(),
            },
            JevAnswer::Score {
                id: "l".into(),
                level: 1,
                score: 1.1,
                confidence: 0.25,
                probabilities: probs(&[("0", 0.2), ("1", 0.5), ("2", 0.3)]),
            },
            JevAnswer::Noul {
                id: "p".into(),
                prob: 0.7,
            },
        ];
        let run = |seed: u64| {
            let mut rng = Rng::seeded(seed);
            let mut draw = || Ok(rng.next_f64());
            answers_to_values(&plan, &answers, true, &mut draw).expect("converts")
        };
        let first = run(7);
        let again = run(7);
        assert_eq!(first, again);
        assert!(first.sampled);
        let Value::Choice(choice) = &first.values["c"] else {
            panic!()
        };
        assert_eq!(choice.probabilities, probabilities);
        assert_eq!(choice.confidence, 0.25);
        assert!(["a", "b", "c"].contains(&choice.label.as_str()));
        let Value::Level(level) = &first.values["l"] else {
            panic!()
        };
        assert_eq!(level.score, 1.1);
        assert!(level.level <= 2);
        assert_eq!(first.values["p"], Value::Prob(0.7));

        // Over many seeds the 20% option is taken sometimes and the argmax
        // is not always taken: the point of sampling.
        let labels: Vec<String> = (0..200)
            .map(|seed| {
                let Value::Choice(c) = &run(seed).values["c"] else {
                    panic!()
                };
                c.label.clone()
            })
            .collect();
        assert!(labels.iter().any(|l| l == "a"));
        assert!(labels.iter().any(|l| l == "c"));
        assert!(labels.iter().filter(|l| *l == "b").count() > 60);
    }

    #[test]
    fn per_judgment_sample_true_samples_that_judgment_only() {
        let plan = plan(vec![
            ask(
                "s",
                PlannedKind::Pick {
                    labels: vec!["a".into(), "b".into()],
                },
                None,
                true,
            ),
            ask(
                "t",
                PlannedKind::Pick {
                    labels: vec!["a".into(), "b".into()],
                },
                None,
                false,
            ),
        ]);
        let answer = |id: &str| JevAnswer::Choice {
            id: id.into(),
            label: "a".into(),
            confidence: 0.0,
            probabilities: probs(&[("a", 0.5), ("b", 0.5)]),
        };
        let answers = vec![answer("s"), answer("t")];
        let mut draws = 0;
        let mut draw = || {
            draws += 1;
            Ok(0.99)
        };
        let answered = answers_to_values(&plan, &answers, false, &mut draw).expect("converts");
        assert_eq!(draws, 1);
        assert!(answered.sampled);
        let Value::Choice(sampled) = &answered.values["s"] else {
            panic!()
        };
        assert_eq!(sampled.label, "b", "0.99 lands on the second half");
        let Value::Choice(argmax) = &answered.values["t"] else {
            panic!()
        };
        assert_eq!(argmax.label, "a");
    }

    #[test]
    fn draw_key_walks_the_cumulative_distribution_in_declared_order() {
        let keys = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let p = probs(&[("a", 0.2), ("b", 0.5), ("c", 0.3)]);
        assert_eq!(draw_key(&keys, &p, 0.0), "a");
        assert_eq!(draw_key(&keys, &p, 0.19), "a");
        assert_eq!(draw_key(&keys, &p, 0.2), "b");
        assert_eq!(draw_key(&keys, &p, 0.69), "b");
        assert_eq!(draw_key(&keys, &p, 0.7), "c");
        assert_eq!(draw_key(&keys, &p, 0.999), "c");
        // A key Jev did not report has probability zero.
        assert_eq!(draw_key(&keys, &probs(&[("c", 1.0)]), 0.0), "c");
    }

    #[test]
    fn a_missing_or_mismatched_answer_is_jev_rejected() {
        let plan = plan(vec![ask("p", PlannedKind::Feels, None, false)]);
        let error = answers_to_values(&plan, &[], false, &mut *no_draw()).expect_err("missing");
        assert_eq!(error.code, RuntimeErrorCode::JevRejected);
        let wrong = vec![JevAnswer::Choice {
            id: "p".into(),
            label: "x".into(),
            confidence: 0.0,
            probabilities: BTreeMap::new(),
        }];
        let error = answers_to_values(&plan, &wrong, false, &mut *no_draw()).expect_err("kind");
        assert_eq!(error.code, RuntimeErrorCode::JevRejected);
    }
}
