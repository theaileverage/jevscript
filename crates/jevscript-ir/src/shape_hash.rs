//! Judgment shape hashes (spec section 11.4).
//!
//! Once host code reads `j.next.label` or `l is unchanged`, a program's labels
//! and level names are an API. The compiler emits a hash per judgment over
//! exactly that surface — result names, verbs, label names and level names, in
//! order — and a host can pin it and refuse to run a program whose judgment
//! shapes changed.
//!
//! What the hash deliberately ignores: descriptions, situations, detail blocks,
//! spans and batching. Rewording a description is not a breaking change;
//! renaming a label is. A `pick among` hashes its `by` field but not the
//! runtime items, so the host contract is stable across inputs (spec section
//! 6.4a), and a machine hashes state names, event names and targets but not
//! event descriptions (spec section 7.8).

use sha2::{Digest, Sha256};

use crate::nodes::{JudgeVerb, JudgmentResult, MachineState};

/// The hash over a judgment's answer space.
///
/// The form is `sh1:` followed by 16 hex characters. The prefix is a version:
/// change it if what goes into the hash ever changes, so that a pinned hash
/// fails loudly rather than silently matching.
pub fn shape_hash(name: &str, results: &[JudgmentResult]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"judgment\x1f");
    hasher.update(name.as_bytes());
    for result in results {
        hasher.update(b"\x1eresult\x1f");
        hasher.update(result.name.as_bytes());
        hasher.update(b"\x1f");
        hasher.update(if result.question.each {
            b"each\x1f" as &[u8]
        } else {
            b"one\x1f"
        });
        match &result.question.verb {
            JudgeVerb::Feels { .. } => hasher.update(b"feels"),
            JudgeVerb::Pick { labels } => {
                hasher.update(b"pick");
                for label in labels {
                    hasher.update(b"\x1f");
                    hasher.update(label.name.as_bytes());
                    hasher.update(if label.escape { b"!" as &[u8] } else { b"." });
                }
            }
            JudgeVerb::PickAmong { by, allow_none, .. } => {
                hasher.update(b"pick_among\x1f");
                hasher.update(by.as_deref().unwrap_or("").as_bytes());
                hasher.update(if *allow_none {
                    b"none" as &[u8]
                } else {
                    b"required"
                });
            }
            JudgeVerb::Rate { levels } => {
                hasher.update(b"rate");
                for (index, level) in levels.iter().enumerate() {
                    hasher.update(b"\x1f");
                    // An unnamed level still counts: arity is part of the shape.
                    match &level.name {
                        Some(name) => hasher.update(name.as_bytes()),
                        None => hasher.update(index.to_string().as_bytes()),
                    }
                }
            }
        }
    }
    let digest = hasher.finalize();
    format!("sh1:{}", hex::encode(&digest[..8]))
}

/// The hash over a machine's control flow: state names, whether each is
/// terminal, and every event name with its target (spec section 7.8).
///
/// Event descriptions are excluded, so rewording one is not a breaking change,
/// and a host that pins this hash is pinning the shape it drew.
pub fn machine_shape_hash(name: &str, states: &[MachineState]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"machine\x1f");
    hasher.update(name.as_bytes());
    for state in states {
        hasher.update(b"\x1estate\x1f");
        hasher.update(state.name.as_bytes());
        hasher.update(if state.done { b"!" as &[u8] } else { b"." });
        for transition in &state.transitions {
            hasher.update(b"\x1f");
            hasher.update(transition.event.as_bytes());
            hasher.update(b"->");
            hasher.update(transition.target.as_bytes());
            if transition.risky {
                hasher.update(b"!risky");
            }
        }
    }
    format!("sh1:{}", hex::encode(&hasher.finalize()[..8]))
}

#[cfg(test)]
mod tests {
    use jevscript_syntax::Span;

    use super::*;
    use crate::nodes::{Expr, Judge, PickLabel, Subject};

    fn subject(root: &str) -> Subject {
        Subject {
            root: root.to_string(),
            path: Vec::new(),
            state_path: root.to_string(),
            span: Span::default(),
        }
    }

    fn pick(name: &str, labels: &[(&str, bool)]) -> JudgmentResult {
        JudgmentResult {
            name: name.to_string(),
            question: Judge {
                each: false,
                subject: subject("message"),
                verb: JudgeVerb::Pick {
                    labels: labels
                        .iter()
                        .map(|(label, escape)| PickLabel {
                            name: (*label).to_string(),
                            escape: *escape,
                            description: Some(Expr::Text {
                                parts: Vec::new(),
                                span: Span::default(),
                            }),
                            what: None,
                            not_for: None,
                            examples: None,
                            span: Span::default(),
                        })
                        .collect(),
                },
                detail: None,
                request_group: 0,
                span: Span::default(),
            },
            span: Span::default(),
        }
    }

    #[test]
    fn is_stable_for_the_same_answer_space() {
        let a = pick("owner", &[("code", false), ("other", true)]);
        let b = pick("owner", &[("code", false), ("other", true)]);
        assert_eq!(shape_hash("triage", &[a]), shape_hash("triage", &[b]));
    }

    #[test]
    fn changes_when_a_label_is_renamed() {
        let before = pick("owner", &[("code", false), ("other", true)]);
        let after = pick("owner", &[("engineering", false), ("other", true)]);
        assert_ne!(
            shape_hash("triage", &[before]),
            shape_hash("triage", &[after])
        );
    }

    #[test]
    fn a_machine_hash_ignores_event_descriptions() {
        use crate::nodes::Transition;

        let state = |description: &str| MachineState {
            name: "working".to_string(),
            done: false,
            transitions: vec![Transition {
                event: "finished".to_string(),
                description: Expr::Name {
                    name: description.to_string(),
                    span: Span::default(),
                },
                target: "reviewing".to_string(),
                when: None,
                risky: false,
                body: Vec::new(),
                span: Span::default(),
            }],
            span: Span::default(),
        };
        assert_eq!(
            machine_shape_hash("review", &[state("the agent says it is done")]),
            machine_shape_hash("review", &[state("reworded, same event")]),
        );
    }

    #[test]
    fn a_machine_hash_changes_when_a_transition_moves() {
        use crate::nodes::Transition;

        let target = |to: &str| MachineState {
            name: "working".to_string(),
            done: false,
            transitions: vec![Transition {
                event: "finished".to_string(),
                description: Expr::Text {
                    parts: Vec::new(),
                    span: Span::default(),
                },
                target: to.to_string(),
                when: None,
                risky: false,
                body: Vec::new(),
                span: Span::default(),
            }],
            span: Span::default(),
        };
        assert_ne!(
            machine_shape_hash("review", &[target("reviewing")]),
            machine_shape_hash("review", &[target("nudging")]),
        );
    }

    #[test]
    fn is_prefixed_with_its_version() {
        let hash = shape_hash(
            "triage",
            &[pick("owner", &[("code", false), ("other", true)])],
        );
        assert!(hash.starts_with("sh1:"), "{hash}");
    }
}
