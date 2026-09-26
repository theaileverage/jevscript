//! Conformance checks over recordings (spec section 15).
//!
//! Items 2, 3, 4 and 6 of the conformance list are stated in terms of what a
//! recording shows: which questions travelled together (`request` events),
//! what state went with them, which pauses were raised and in what order, and
//! which events a machine was offered (`machine_step` events). The functions
//! here read a recording back and say where it departs from the spec, so that
//! any program run under any adapters can be checked after the fact.
//!
//! Item 1 is the compiler's test suite, item 5 the SDKs', and item 7 is a
//! source scan in this crate's tests: no limit or price may be written into
//! code (spec section 10.6). Item 9 is `log` (spec section 5.8): every log is
//! one `log` event outside any Jev request, and a replay reproduces the same
//! logs in the same order.

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};

use jevscript_ir::{Expr, Ir, Stmt};
use jevscript_runtime::jev::Question;
use jevscript_runtime::pause::{Pause, PauseKind};
use jevscript_runtime::profile::Profile;
use jevscript_runtime::record::{Event, RecordedEvent};
use serde_json::Value as Json;

/// Where a recording departs from the spec.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    /// Which conformance item (spec section 15) was broken.
    pub item: u8,
    /// The `seq` of the event that shows it, if one does.
    pub seq: Option<u64>,
    /// What went wrong.
    pub message: String,
}

impl Violation {
    fn new(item: u8, seq: Option<u64>, message: impl Into<String>) -> Self {
        Self {
            item,
            seq,
            message: message.into(),
        }
    }
}

/// Run every recording-based check that needs no host configuration.
///
/// This proves batching group membership (item 2), state construction (item
/// 3), and the machine-menu invariants observable from a recording (item 6).
/// A self-contained recording carries its resolved profile, so this also proves
/// that `each` chunks obey that profile's question cap. Legacy recordings have
/// no execution metadata and fall back to cap-independent batching checks; use
/// [`check_recording_with_profile`] when their selected profile is known.
pub fn check_recording(ir: &Ir, events: &[RecordedEvent]) -> Vec<Violation> {
    match events
        .iter()
        .find_map(|event| event.execution.as_ref().map(|execution| &execution.profile))
    {
        Some(profile) => check_recording_with_profile(ir, events, profile),
        None => {
            let mut violations = check_batching(ir, events);
            violations.extend(check_state(ir, events));
            violations.extend(check_machine_menus(ir, events));
            violations.extend(check_logs(events));
            violations
        }
    }
}

/// Run every recording-based check using the selected model profile.
///
/// In addition to [`check_recording`], this proves that request chunks obey
/// `max_questions_per_request` (spec sections 6.5, 10.6 and 15.2).
pub fn check_recording_with_profile(
    ir: &Ir,
    events: &[RecordedEvent],
    profile: &Profile,
) -> Vec<Violation> {
    let mut violations = check_batching_with_profile(ir, events, profile);
    violations.extend(check_state(ir, events));
    violations.extend(check_machine_menus(ir, events));
    violations.extend(check_logs(events));
    violations
}

/* -------------------------------------------------------------------------- */
/* Item 2: batching                                                            */
/* -------------------------------------------------------------------------- */

/// A request group as the IR declares it (spec section 6.6): its unit and number.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct GroupKey {
    unit: String,
    group: u32,
}

/// One answer id in a request group, in section 6.5/6.6 planning order.
#[derive(Debug, Clone, PartialEq, Eq)]
struct GroupAsk {
    name: String,
    each: bool,
}

/// Every request group in the IR. Keeping order and duplicates is important:
/// the runtime flattens asks in this order before applying the profile cap.
fn request_groups(ir: &Ir) -> BTreeMap<GroupKey, Vec<GroupAsk>> {
    let mut groups: BTreeMap<GroupKey, Vec<GroupAsk>> = BTreeMap::new();
    let mut add = |name: &str, each: bool, key: GroupKey| {
        groups.entry(key).or_default().push(GroupAsk {
            name: name.to_string(),
            each,
        });
    };
    for judgment in &ir.judgments {
        for result in &judgment.results {
            add(
                &result.name,
                result.question.each,
                GroupKey {
                    unit: judgment.name.clone(),
                    group: result.question.request_group,
                },
            );
        }
    }
    for task in &ir.tasks {
        walk_body(&task.body, &task.name, &mut add);
    }
    for def in &ir.defs {
        walk_body(&def.body, &def.name, &mut add);
    }
    for machine in &ir.machines {
        for state in &machine.states {
            for transition in &state.transitions {
                walk_body(&transition.body, &machine.name, &mut add);
            }
        }
    }
    groups
}

fn walk_body(body: &[Stmt], unit: &str, add: &mut impl FnMut(&str, bool, GroupKey)) {
    for stmt in body {
        match stmt {
            Stmt::Assign {
                root,
                value: Expr::Judge(judge),
                ..
            } => add(
                root,
                judge.each,
                GroupKey {
                    unit: unit.to_string(),
                    group: judge.request_group,
                },
            ),
            Stmt::If {
                branches,
                otherwise,
                ..
            } => {
                for branch in branches {
                    walk_body(&branch.body, unit, add);
                }
                if let Some(body) = otherwise {
                    walk_body(body, unit, add);
                }
            }
            Stmt::For { body, .. } | Stmt::Loop { body, .. } | Stmt::Until { body, .. } => {
                walk_body(body, unit, add);
            }
            Stmt::Gate { arms, .. } => {
                for arm in arms {
                    walk_body(&arm.body, unit, add);
                }
            }
            _ => {}
        }
    }
}

/// A section 6.5 answer id, with its `each` index separated from its base.
#[derive(Debug, Clone, PartialEq, Eq)]
struct AnswerId<'a> {
    base: &'a str,
    index: Option<usize>,
}

fn answer_id(id: &str) -> Option<AnswerId<'_>> {
    let Some(open) = id.rfind('[') else {
        return (!id.is_empty()).then_some(AnswerId {
            base: id,
            index: None,
        });
    };
    let digits = id.get(open + 1..id.len().checked_sub(1)?)?;
    if id.as_bytes().last() != Some(&b']')
        || id[..open].contains(['[', ']'])
        || digits.is_empty()
        || !digits.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    Some(AnswerId {
        base: &id[..open],
        index: Some(digits.parse().ok()?),
    })
}

fn group_matches(ids: &[AnswerId<'_>], asks: &[GroupAsk]) -> bool {
    let mut position = 0;
    for ask in asks {
        if ask.each {
            let mut expected = 0;
            while ids
                .get(position)
                .is_some_and(|id| id.base == ask.name && id.index == Some(expected))
            {
                position += 1;
                expected += 1;
            }
        } else {
            let Some(id) = ids.get(position) else {
                return false;
            };
            if id.base != ask.name || id.index.is_some() {
                return false;
            }
            position += 1;
        }
    }
    position == ids.len()
}

/// Whether one non-empty request could be a cap-sized slice of this group's
/// flattened asks. This is used only to choose the applicable state rule; the
/// batching checker separately proves that all slices form a complete group.
fn group_chunk_matches(ids: &[AnswerId<'_>], asks: &[GroupAsk]) -> bool {
    let mut previous_ask = None;
    let mut previous_index = None;
    for id in ids {
        let start = previous_ask.unwrap_or(0);
        let Some((ask_index, ask)) = asks
            .iter()
            .enumerate()
            .skip(start)
            .find(|(_, ask)| ask.name == id.base && ask.each == id.index.is_some())
        else {
            return false;
        };
        if previous_ask == Some(ask_index) {
            if ask.each && id.index != previous_index.map(|index| index + 1) {
                return false;
            }
        } else if previous_ask.is_some() && ask.each && id.index != Some(0) {
            return false;
        }
        previous_ask = Some(ask_index);
        previous_index = id.index;
    }
    !ids.is_empty()
}

fn is_machine_request(ir: &Ir, questions: &[Question]) -> bool {
    !ir.machines.is_empty()
        && matches!(
            questions,
            [Question::Choice {
                id,
                path,
                instruction: Some(instruction),
                ..
            }] if id == "event"
                && path == "obs"
                && instruction.compare == ["state", "goal", "recent"]
        )
}

/// Item 2: every `request` event holds exactly one request group's questions,
/// and a group is split across requests only when an `each` exceeded the
/// profile's question cap (spec sections 6.5 and 6.6).
///
/// A machine step's Choice is its own request and has no `Judge` node; it is
/// recognised by its single question whose id is the machine's name and is
/// skipped here (item 6 checks it).
pub fn check_batching(ir: &Ir, events: &[RecordedEvent]) -> Vec<Violation> {
    check_batching_inner(ir, events, None)
}

/// Item 2 with the selected profile: in addition to group membership, every
/// chunk is bounded by and split exactly at `max_questions_per_request`.
pub fn check_batching_with_profile(
    ir: &Ir,
    events: &[RecordedEvent],
    profile: &Profile,
) -> Vec<Violation> {
    check_batching_inner(ir, events, Some(profile.max_questions_per_request as usize))
}

fn check_batching_inner(
    ir: &Ir,
    events: &[RecordedEvent],
    question_cap: Option<usize>,
) -> Vec<Violation> {
    let groups = request_groups(ir);
    let mut violations = Vec::new();
    let mut requests = Vec::new();
    for event in events {
        let Event::Request { questions, .. } = &event.event else {
            continue;
        };
        if is_machine_request(ir, questions) {
            continue;
        }
        if questions.is_empty() {
            violations.push(Violation::new(
                2,
                Some(event.seq),
                "an empty `request` cannot represent a judgment group; an empty `each` emits no question",
            ));
            continue;
        }
        let ids: Vec<&str> = questions.iter().map(Question::id).collect();
        let unique: BTreeSet<&str> = ids.iter().copied().collect();
        if unique.len() != ids.len() {
            violations.push(Violation::new(
                2,
                Some(event.seq),
                format!("request has duplicate question ids: {ids:?}"),
            ));
            continue;
        }
        if ids.iter().any(|id| answer_id(id).is_none()) {
            violations.push(Violation::new(
                2,
                Some(event.seq),
                format!("request has a malformed question id: {ids:?}"),
            ));
            continue;
        }
        requests.push((event.seq, questions.as_slice()));
    }

    // Partition the request stream into logical group instances. Trying every
    // declared group avoids the old lexicographic choice when names collide:
    // a recording passes only if at least one complete, ordered partition
    // exists. The recording has no unit id, so identical groups are
    // intentionally indistinguishable.
    let mut reachable = vec![false; requests.len() + 1];
    reachable[0] = true;
    for start in 0..requests.len() {
        if !reachable[start] {
            continue;
        }
        let mut ids = Vec::new();
        for end in start..requests.len() {
            let questions = requests[end].1;
            ids.extend(questions.iter().filter_map(|q| answer_id(q.id())));
            let chunk_count = end - start + 1;
            for asks in groups.values() {
                if !group_matches(&ids, asks) {
                    continue;
                }
                if chunk_count > 1 && !asks.iter().any(|ask| ask.each) {
                    continue;
                }
                if let Some(cap) = question_cap
                    && (cap == 0
                        || requests[start..end]
                            .iter()
                            .any(|(_, chunk)| chunk.len() != cap)
                        || questions.len() > cap
                        || (chunk_count > 1
                            && !asks.iter().any(|ask| {
                                ask.each
                                    && ids.iter().filter(|id| id.base == ask.name).count() > cap
                            })))
                {
                    continue;
                }
                reachable[end + 1] = true;
            }
        }
    }
    if !reachable[requests.len()] {
        let start = reachable
            .iter()
            .rposition(|reachable| *reachable)
            .unwrap_or(0)
            .min(requests.len().saturating_sub(1));
        if let Some((seq, questions)) = requests.get(start) {
            let ids: Vec<&str> = questions.iter().map(|q| q.id()).collect();
            let cap = question_cap
                .map(|cap| format!(" within the profile cap of {cap}"))
                .unwrap_or_default();
            violations.push(Violation::new(
                2,
                Some(*seq),
                format!(
                    "request sequence beginning with {ids:?} does not form a complete declared request group{cap}"
                ),
            ));
        }
    }
    violations
}

/* -------------------------------------------------------------------------- */
/* Item 3: state                                                               */
/* -------------------------------------------------------------------------- */

/// One step of a state path.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum PathStep {
    Field(String),
    Index(usize),
}

/// Parse `obs.summary`, `files[3]` or `e.body` into a root and its steps.
fn parse_path(path: &str) -> Option<(String, Vec<PathStep>)> {
    let mut root = String::new();
    let mut steps = Vec::new();
    let mut rest = path;
    let end = rest.find(['.', '[']).unwrap_or(rest.len());
    if end == 0 {
        return None;
    }
    root.push_str(&rest[..end]);
    rest = &rest[end..];
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix('.') {
            let end = after.find(['.', '[']).unwrap_or(after.len());
            if end == 0 {
                return None;
            }
            steps.push(PathStep::Field(after[..end].to_string()));
            rest = &after[end..];
        } else if let Some(after) = rest.strip_prefix('[') {
            let end = after.find(']')?;
            let index = after[..end].parse::<usize>().ok()?;
            steps.push(PathStep::Index(index));
            rest = after.get(end + 1..)?;
        } else {
            return None;
        }
    }
    Some((root, steps))
}

/// The paths a request names, as a trie: a node with no children is a path
/// end, and everything under it may be sent in full.
#[derive(Debug, Default)]
struct Trie {
    children: BTreeMap<PathStep, Trie>,
    /// A path ends here, so the whole value at this node is allowed.
    whole: bool,
}

impl Trie {
    fn insert(&mut self, steps: &[PathStep]) {
        match steps.split_first() {
            None => self.whole = true,
            Some((first, rest)) => self.children.entry(first.clone()).or_default().insert(rest),
        }
    }
}

/// Every state path a request reads: the questions' subjects and their
/// `compare` paths (spec sections 6.8 and 6.9).
fn request_paths(questions: &[Question]) -> Vec<String> {
    let mut paths = Vec::new();
    for question in questions {
        paths.push(question.path().to_string());
        if let Some(instruction) = question_instruction(question) {
            paths.extend(instruction.compare.iter().cloned());
        }
    }
    paths
}

fn question_instruction(question: &Question) -> Option<&jevscript_runtime::jev::Instruction> {
    match question {
        Question::Noul { instruction, .. }
        | Question::Choice { instruction, .. }
        | Question::Score { instruction, .. } => instruction.as_ref(),
    }
}

/// Item 3: the state sent with a request has exactly the shape section 6.9
/// assigns to that request kind.
///
/// Inline judgments carry only subject and `compare` paths; named judgments
/// carry every parameter in full; and machine choices carry exactly `state`,
/// `goal`, `obs`, and the last six `recent` event records. Redacted state
/// cannot be re-examined, so it is deliberately skipped (spec section 10.3).
/// If identical question ids and ordering occur in both a named and inline
/// group, the recording has no per-request provenance; this checker accepts a
/// state valid for either origin instead of inventing an identity.
pub fn check_state(ir: &Ir, events: &[RecordedEvent]) -> Vec<Violation> {
    let mut violations = Vec::new();
    for event in events {
        let Event::Request {
            state, questions, ..
        } = &event.event
        else {
            continue;
        };
        let Some(state) = state.full() else {
            continue; // redacted: control flow replays, the payload is gone
        };
        if is_machine_request(ir, questions) {
            check_machine_state(state, event.seq, &mut violations);
            continue;
        }
        let ids: Option<Vec<AnswerId<'_>>> = questions
            .iter()
            .map(|question| answer_id(question.id()))
            .collect();
        let groups = request_groups(ir);
        let matching_units: BTreeSet<&str> = ids
            .as_ref()
            .map(|ids| {
                groups
                    .iter()
                    .filter(|(_, asks)| group_matches(ids, asks) || group_chunk_matches(ids, asks))
                    .map(|(key, _)| key.unit.as_str())
                    .collect()
            })
            .unwrap_or_default();
        let named_units: BTreeSet<&str> = ir
            .judgments
            .iter()
            .map(|judgment| judgment.name.as_str())
            .collect();
        let can_be_inline = matching_units.is_empty()
            || matching_units
                .iter()
                .any(|unit| !named_units.contains(unit));
        let mut candidates = Vec::new();
        if can_be_inline {
            candidates.push(check_inline_state(state, questions, event.seq));
        }
        for judgment in &ir.judgments {
            if matching_units.contains(judgment.name.as_str()) {
                let params = judgment.params.iter().cloned().collect();
                candidates.push(exact_roots(state, &params, event.seq, "judgment parameter"));
            }
        }
        if candidates.is_empty() {
            candidates.push(check_inline_state(state, questions, event.seq));
        }
        violations.extend(
            candidates
                .into_iter()
                .min_by_key(Vec::len)
                .unwrap_or_default(),
        );
    }
    violations
}

fn check_inline_state(
    state: &BTreeMap<String, Json>,
    questions: &[Question],
    seq: u64,
) -> Vec<Violation> {
    let mut violations = Vec::new();
    let mut roots: BTreeMap<String, Trie> = BTreeMap::new();
    for path in request_paths(questions) {
        match parse_path(&path) {
            Some((root, steps)) => roots.entry(root).or_default().insert(&steps),
            None => violations.push(Violation::new(
                3,
                Some(seq),
                format!("question names malformed state path `{path}`"),
            )),
        }
    }
    for root in state.keys() {
        if !roots.contains_key(root) {
            violations.push(Violation::new(
                3,
                Some(seq),
                format!("state root `{root}` is not named by any subject in the request"),
            ));
        }
    }
    for (root, trie) in &roots {
        match state.get(root) {
            None => violations.push(Violation::new(
                3,
                Some(seq),
                format!("subject root `{root}` is missing from the state"),
            )),
            Some(value) => check_value(value, trie, root, seq, &mut violations),
        }
    }
    violations
}

fn exact_roots(
    state: &BTreeMap<String, Json>,
    expected: &BTreeSet<String>,
    seq: u64,
    description: &str,
) -> Vec<Violation> {
    let actual: BTreeSet<String> = state.keys().cloned().collect();
    let mut violations = Vec::new();
    for extra in actual.difference(expected) {
        violations.push(Violation::new(
            3,
            Some(seq),
            format!("state root `{extra}` is not a {description}"),
        ));
    }
    for missing in expected.difference(&actual) {
        violations.push(Violation::new(
            3,
            Some(seq),
            format!("{description} `{missing}` is missing from the state"),
        ));
    }
    violations
}

fn check_machine_state(state: &BTreeMap<String, Json>, seq: u64, violations: &mut Vec<Violation>) {
    let expected: BTreeSet<String> = ["state", "goal", "obs", "recent"]
        .into_iter()
        .map(str::to_string)
        .collect();
    violations.extend(exact_roots(state, &expected, seq, "machine state field"));
    for key in ["state", "goal"] {
        if state.get(key).is_some_and(|value| !value.is_string()) {
            violations.push(Violation::new(
                3,
                Some(seq),
                format!("machine state field `{key}` must be text"),
            ));
        }
    }
    if state.get("obs").is_some_and(|value| !value.is_object()) {
        violations.push(Violation::new(
            3,
            Some(seq),
            "machine state field `obs` must be a record",
        ));
    }
    if let Some(recent) = state.get("recent") {
        let Some(records) = recent.as_array() else {
            violations.push(Violation::new(
                3,
                Some(seq),
                "machine state field `recent` must be a list",
            ));
            return;
        };
        if records.len() > 6 {
            violations.push(Violation::new(
                3,
                Some(seq),
                format!(
                    "machine state has {} `recent` events, not the last six",
                    records.len()
                ),
            ));
        }
        let fields: BTreeSet<&str> = ["step", "from", "event", "to"].into_iter().collect();
        for (index, record) in records.iter().enumerate() {
            let Some(record) = record.as_object() else {
                violations.push(Violation::new(
                    3,
                    Some(seq),
                    format!("machine state `recent[{index}]` is not an event record"),
                ));
                continue;
            };
            let actual: BTreeSet<&str> = record.keys().map(String::as_str).collect();
            if actual != fields
                || !record.get("step").is_some_and(Json::is_u64)
                || ["from", "event", "to"]
                    .into_iter()
                    .any(|key| !record.get(key).is_some_and(Json::is_string))
            {
                violations.push(Violation::new(
                    3,
                    Some(seq),
                    format!(
                        "machine state `recent[{index}]` must contain only numeric `step` and text `from`, `event`, `to`"
                    ),
                ));
            }
        }
    }
}

fn check_value(value: &Json, trie: &Trie, at: &str, seq: u64, violations: &mut Vec<Violation>) {
    if trie.whole {
        return;
    }
    match value {
        Json::Object(fields) => {
            if trie
                .children
                .keys()
                .any(|step| matches!(step, PathStep::Index(_)))
            {
                violations.push(Violation::new(
                    3,
                    Some(seq),
                    format!("`{at}` is a record but a subject path indexes it as a list"),
                ));
            }
            for (key, child) in fields {
                match trie.children.get(&PathStep::Field(key.clone())) {
                    Some(sub) => check_value(child, sub, &format!("{at}.{key}"), seq, violations),
                    None => violations.push(Violation::new(
                        3,
                        Some(seq),
                        format!("`{at}.{key}` was sent but no subject path reaches it"),
                    )),
                }
            }
            for step in trie.children.keys() {
                if let PathStep::Field(key) = step
                    && !fields.contains_key(key)
                {
                    violations.push(Violation::new(
                        3,
                        Some(seq),
                        format!("required subject path `{at}.{key}` is missing from the state"),
                    ));
                }
            }
        }
        Json::Array(items) => {
            if trie
                .children
                .keys()
                .any(|step| matches!(step, PathStep::Field(_)))
            {
                violations.push(Violation::new(
                    3,
                    Some(seq),
                    format!("`{at}` is a list but a subject path reads it as a record"),
                ));
            }
            for (index, child) in items.iter().enumerate() {
                match trie.children.get(&PathStep::Index(index)) {
                    Some(sub) => {
                        check_value(child, sub, &format!("{at}[{index}]"), seq, violations)
                    }
                    None if child.is_null() => {}
                    None => violations.push(Violation::new(
                        3,
                        Some(seq),
                        format!("`{at}[{index}]` was sent but no subject path reaches it"),
                    )),
                }
            }
            for step in trie.children.keys() {
                if let PathStep::Index(index) = step
                    && *index >= items.len()
                {
                    violations.push(Violation::new(
                        3,
                        Some(seq),
                        format!("required subject path `{at}[{index}]` is missing from the state"),
                    ));
                }
            }
        }
        _ => violations.push(Violation::new(
            3,
            Some(seq),
            format!("`{at}` is a scalar but the subject paths go deeper"),
        )),
    }
}

/* -------------------------------------------------------------------------- */
/* Item 4: pauses                                                              */
/* -------------------------------------------------------------------------- */

/// A pause surfaced to the host for conformance item 4 (spec section 15).
///
/// Event timestamps and sequence numbers are recording metadata; the event's
/// kind and the complete host-visible pause payload must replay identically.
#[derive(Debug, Clone, PartialEq)]
pub struct RecordedPause {
    /// The kind written on the recording event.
    pub kind: PauseKind,
    /// Every common and kind-specific field surfaced to the host.
    pub payload: Pause,
}

/// The complete host-visible pauses a recording raised, in order.
pub fn pause_sequence(events: &[RecordedEvent]) -> Vec<RecordedPause> {
    events
        .iter()
        .filter_map(|event| match &event.event {
            Event::Pause { kind, payload } => Some(RecordedPause {
                kind: *kind,
                payload: payload.clone(),
            }),
            _ => None,
        })
        .collect()
}

/// Item 4: a replay raises identical pauses in identical order. Returns the
/// first position where the kind or any host-visible payload field differs.
pub fn pauses_diverge_at(original: &[RecordedEvent], replay: &[RecordedEvent]) -> Option<usize> {
    let a = pause_sequence(original);
    let b = pause_sequence(replay);
    (0..a.len().max(b.len())).find(|&i| a.get(i) != b.get(i))
}

/* -------------------------------------------------------------------------- */
/* Item 9: logs                                                                */
/* -------------------------------------------------------------------------- */

/// Item 9: a `log` is recorded on its own, never as part of a Jev request
/// (spec section 5.8). A `log` event between a `request` and the `answers` or
/// `effect_error` that settles it would mean the log was evaluated while the
/// request was out, which only a question could do.
pub fn check_logs(events: &[RecordedEvent]) -> Vec<Violation> {
    let mut violations = Vec::new();
    let mut open_request: Option<&str> = None;
    for event in events {
        match &event.event {
            Event::Request { request_id, .. } => open_request = Some(request_id),
            Event::Answers { .. } | Event::EffectError { .. } => open_request = None,
            Event::Log { level, .. } => {
                if let Some(request_id) = open_request {
                    violations.push(Violation::new(
                        9,
                        Some(event.seq),
                        format!(
                            "a `{}` log was recorded while request `{request_id}` was unanswered",
                            level.as_str()
                        ),
                    ));
                }
            }
            _ => {}
        }
    }
    violations
}

/// The `log` events a recording holds, in order, without their recording
/// metadata: exactly what a replay must reproduce (spec section 5.8).
pub fn log_sequence(events: &[RecordedEvent]) -> Vec<Event> {
    events
        .iter()
        .filter(|event| matches!(event.event, Event::Log { .. }))
        .map(|event| event.event.clone())
        .collect()
}

/// Item 9: a replay reproduces the same logs in the same order. Returns the
/// first position where a level, message, field, task or source differs.
pub fn logs_diverge_at(original: &[RecordedEvent], replay: &[RecordedEvent]) -> Option<usize> {
    let a = log_sequence(original);
    let b = log_sequence(replay);
    (0..a.len().max(b.len())).find(|&i| a.get(i) != b.get(i))
}

/* -------------------------------------------------------------------------- */
/* Item 6: machine menus                                                       */
/* -------------------------------------------------------------------------- */

/// Item 6: every `machine_step` offers exactly the events the recording says
/// were enabled, plus `stay` (spec section 7.8).
///
/// This check proves that `stay` and every unguarded event are present in
/// declaration order, no undeclared or duplicate event is present, the chosen
/// label and destination are legal, and probabilities cover the menu. It also
/// recognises the empty-distribution no-decision sentinel only when the next
/// event is its matching `no_enabled_events` pause.
///
/// It cannot prove whether an arbitrary `when` expression was true from the
/// menu alone; guard evaluation is code and its inputs/results are not recorded
/// on `machine_step` (spec section 7.8).
pub fn check_machine_menus(ir: &Ir, events: &[RecordedEvent]) -> Vec<Violation> {
    let mut violations = Vec::new();
    for (event_index, event) in events.iter().enumerate() {
        let Event::MachineStep {
            machine,
            state,
            enabled,
            chosen,
            probabilities,
            confidence,
            to,
        } = &event.event
        else {
            continue;
        };
        let Some(declared) = ir.machine(machine) else {
            violations.push(Violation::new(
                6,
                Some(event.seq),
                format!("machine `{machine}` is not declared"),
            ));
            continue;
        };
        let Some(declared_state) = declared.states.iter().find(|s| &s.name == state) else {
            violations.push(Violation::new(
                6,
                Some(event.seq),
                format!("machine `{machine}` has no state `{state}`"),
            ));
            continue;
        };
        if declared_state.done {
            violations.push(Violation::new(
                6,
                Some(event.seq),
                format!("terminal state `{state}` cannot have a machine step"),
            ));
        }
        let menu: BTreeSet<&str> = enabled.iter().map(String::as_str).collect();
        if menu.len() != enabled.len() {
            violations.push(Violation::new(
                6,
                Some(event.seq),
                format!("step in `{state}` offered a duplicate event: {enabled:?}"),
            ));
        }
        if !menu.contains("stay") {
            violations.push(Violation::new(
                6,
                Some(event.seq),
                format!("step in `{state}` did not offer `stay`"),
            ));
        }
        if enabled.last().is_some_and(|label| label != "stay") {
            violations.push(Violation::new(
                6,
                Some(event.seq),
                format!("step in `{state}` did not offer `stay` last"),
            ));
        }
        for label in &menu {
            if *label == "stay" {
                continue;
            }
            if !declared_state.transitions.iter().any(|t| t.event == *label) {
                violations.push(Violation::new(
                    6,
                    Some(event.seq),
                    format!("`{label}` was offered in `{state}` but is not an event of that state"),
                ));
            }
        }
        for transition in &declared_state.transitions {
            if transition.when.is_none() && !menu.contains(transition.event.as_str()) {
                violations.push(Violation::new(
                    6,
                    Some(event.seq),
                    format!(
                        "unguarded event `{}` was omitted from the menu in `{state}`",
                        transition.event
                    ),
                ));
            }
        }
        let declared_order: Vec<&str> = declared_state
            .transitions
            .iter()
            .map(|transition| transition.event.as_str())
            .collect();
        let offered_order: Vec<&str> = enabled
            .iter()
            .map(String::as_str)
            .filter(|label| *label != "stay")
            .collect();
        let mut next_declared = 0;
        let order_matches = offered_order.iter().all(|offered| {
            let Some(offset) = declared_order[next_declared..]
                .iter()
                .position(|declared| declared == offered)
            else {
                return false;
            };
            next_declared += offset + 1;
            true
        });
        if !order_matches {
            violations.push(Violation::new(
                6,
                Some(event.seq),
                format!("events {offered_order:?} are not in declaration order {declared_order:?}"),
            ));
        }
        if !menu.contains(chosen.as_str()) {
            violations.push(Violation::new(
                6,
                Some(event.seq),
                format!("`{chosen}` was chosen but not offered"),
            ));
        }
        let no_decision = enabled == &["stay"]
            && chosen == "stay"
            && probabilities.is_empty()
            && *confidence == 0.0
            && to == state;
        if no_decision {
            let matching_pause = events.get(event_index + 1).is_some_and(|next| {
                matches!(
                    &next.event,
                    Event::Pause {
                        kind: PauseKind::Escalate,
                        payload: Pause::Escalate { common, reason, .. },
                    } if reason == "no_enabled_events"
                        && common.task == format!("machine:{machine}")
                        && common.state.as_deref() == Some(state.as_str())
                )
            });
            if !matching_pause {
                violations.push(Violation::new(
                    6,
                    Some(event.seq),
                    "empty machine probability sentinel is not followed by its matching `no_enabled_events` pause",
                ));
            }
        } else {
            let keys: BTreeSet<&str> = probabilities.keys().map(String::as_str).collect();
            if keys != menu {
                violations.push(Violation::new(
                    6,
                    Some(event.seq),
                    format!("probabilities are over {keys:?}, not the menu {menu:?}"),
                ));
            }
        }
        for (label, probability) in probabilities {
            if !probability.is_finite() || !(0.0..=1.0).contains(probability) {
                violations.push(Violation::new(
                    6,
                    Some(event.seq),
                    format!("probability for `{label}` is not between zero and one"),
                ));
            }
        }
        let total: f64 = probabilities.values().sum();
        if !no_decision && (total - 1.0).abs() > 1e-9 {
            violations.push(Violation::new(
                6,
                Some(event.seq),
                format!("menu probabilities sum to {total}, not one"),
            ));
        }
        if !confidence.is_finite() || !(0.0..=1.0).contains(confidence) {
            violations.push(Violation::new(
                6,
                Some(event.seq),
                "machine-step confidence is not between zero and one",
            ));
        }
        let expected_to = if chosen == "stay" {
            state.clone()
        } else {
            declared_state
                .transitions
                .iter()
                .find(|t| &t.event == chosen)
                .map(|t| t.target.clone())
                .unwrap_or_else(|| state.clone())
        };
        if to != &expected_to && to != state {
            violations.push(Violation::new(
                6,
                Some(event.seq),
                format!("`{chosen}` should lead to `{expected_to}`, not `{to}`"),
            ));
        }
    }
    violations
}

#[cfg(test)]
mod tests {
    use jevscript_ir::{
        Budget, Judge, JudgeVerb, Judgment, JudgmentResult, Machine, MachineState, Span, Subject,
        Task, Thresholds, Transition,
    };
    use jevscript_runtime::jev::Instruction;
    use jevscript_runtime::profile::Tokenizer;
    use serde_json::json;

    use super::*;

    fn text(t: &str) -> Expr {
        Expr::Text {
            parts: vec![jevscript_ir::TextPart::Literal {
                value: t.to_string(),
            }],
            span: Span::default(),
        }
    }

    fn judge(root: &str, field: Option<&str>, group: u32) -> Judge {
        let state_path = match field {
            Some(f) => format!("{root}.{f}"),
            None => root.to_string(),
        };
        Judge {
            each: false,
            subject: Subject {
                root: root.to_string(),
                path: field
                    .map(|f| {
                        vec![jevscript_ir::SubjectStep::Field {
                            name: f.to_string(),
                        }]
                    })
                    .unwrap_or_default(),
                state_path,
                span: Span::default(),
            },
            verb: JudgeVerb::Feels {
                condition: text("holds"),
            },
            detail: None,
            request_group: group,
            span: Span::default(),
        }
    }

    fn program() -> Ir {
        let mut ir = Ir::empty("p");
        ir.judgments.push(Judgment {
            name: "triage".into(),
            params: vec!["message".into()],
            results: vec![
                JudgmentResult {
                    name: "urgent".into(),
                    question: judge("message", None, 0),
                    span: Span::default(),
                },
                JudgmentResult {
                    name: "risky".into(),
                    question: judge("message", None, 0),
                    span: Span::default(),
                },
            ],
            logs: Vec::new(),
            shape_hash: String::new(),
            span: Span::default(),
        });
        ir.tasks.push(Task {
            name: "main".into(),
            params: Vec::new(),
            budget: Budget::default(),
            thresholds: Thresholds::default(),
            body: vec![
                Stmt::Assign {
                    root: "a".into(),
                    path: Vec::new(),
                    value: Expr::Judge(Box::new(judge("obs", Some("summary"), 0))),
                    span: Span::default(),
                },
                Stmt::Assign {
                    root: "b".into(),
                    path: Vec::new(),
                    value: Expr::Judge(Box::new(judge("obs", Some("tests"), 0))),
                    span: Span::default(),
                },
                Stmt::Assign {
                    root: "c".into(),
                    path: Vec::new(),
                    value: Expr::Judge(Box::new(judge("a", None, 1))),
                    span: Span::default(),
                },
            ],
            span: Span::default(),
        });
        ir.machines.push(Machine {
            name: "review".into(),
            params: Vec::new(),
            budget: Budget::default(),
            thresholds: Thresholds::default(),
            goal: None,
            initial: "working".into(),
            observe: Vec::new(),
            states: vec![
                MachineState {
                    name: "working".into(),
                    done: false,
                    transitions: vec![Transition {
                        event: "finished".into(),
                        description: text("done"),
                        target: "approved".into(),
                        when: None,
                        risky: false,
                        body: Vec::new(),
                        span: Span::default(),
                    }],
                    span: Span::default(),
                },
                MachineState {
                    name: "approved".into(),
                    done: true,
                    transitions: Vec::new(),
                    span: Span::default(),
                },
            ],
            shape_hash: String::new(),
            span: Span::default(),
        });
        ir
    }

    fn noul(id: &str, path: &str) -> Question {
        Question::Noul {
            id: id.into(),
            path: path.into(),
            condition: "holds".into(),
            instruction: None,
        }
    }

    fn request(seq: u64, state: Json, questions: Vec<Question>) -> RecordedEvent {
        let Json::Object(map) = state else {
            panic!("object")
        };
        RecordedEvent {
            ts: String::new(),
            run_id: "run_1".into(),
            seq,
            execution: None,
            event: Event::Request {
                request_id: format!("req_{seq}"),
                state: map.into_iter().collect::<BTreeMap<_, _>>().into(),
                questions,
            },
        }
    }

    fn profile(question_cap: u32) -> Profile {
        Profile {
            model: "test".into(),
            endpoint: "http://invalid.test".into(),
            total_tokens: 1_000,
            state_plus_question_tokens: 500,
            max_questions_per_request: question_cap,
            max_criteria_per_question: 8,
            tokenizer: Tokenizer::Chars4,
            price_per_million_input_usd: 0.0,
            price_per_million_output_usd: 0.0,
            aliases: None,
        }
    }

    #[test]
    fn item_2_a_group_that_travels_together_passes() {
        let events = vec![
            request(
                0,
                json!({"obs": {"summary": "s", "tests": "t"}}),
                vec![noul("a", "obs.summary"), noul("b", "obs.tests")],
            ),
            request(1, json!({"a": 0.2}), vec![noul("c", "a")]),
        ];
        assert!(check_batching(&program(), &events).is_empty());
    }

    #[test]
    fn item_2_a_split_group_is_a_violation() {
        // Spec 6.6 rule 2: `a` and `b` share a group, so sending them apart is
        // wrong unless an `each` overflowed the cap.
        let events = vec![
            request(
                0,
                json!({"obs": {"summary": "s"}}),
                vec![noul("a", "obs.summary")],
            ),
            request(1, json!({"a": 0.2}), vec![noul("c", "a")]),
        ];
        let violations = check_batching(&program(), &events);
        assert_eq!(violations.len(), 1, "{violations:?}");
        assert!(violations[0].message.contains("complete declared"));
    }

    #[test]
    fn item_2_questions_from_two_groups_in_one_request_is_a_violation() {
        let events = vec![request(
            0,
            json!({"obs": {"summary": "s"}, "a": 0.1}),
            vec![noul("a", "obs.summary"), noul("c", "a")],
        )];
        let violations = check_batching(&program(), &events);
        assert!(
            violations
                .iter()
                .any(|v| v.message.contains("complete declared"))
        );
    }

    #[test]
    fn item_2_an_each_split_across_requests_is_fine() {
        // Spec 6.5: an `each` over a long list is split at the profile's cap
        // and recorded as one logical judgment.
        let events = vec![
            request(
                0,
                json!({"obs": {"summary": ["s0", "s1"]}}),
                vec![
                    noul("a[0]", "obs.summary[0]"),
                    noul("a[1]", "obs.summary[1]"),
                ],
            ),
            request(
                1,
                json!({"obs": {"summary": [null, null, "s2"], "tests": "t"}}),
                vec![noul("a[2]", "obs.summary[2]"), noul("b", "obs.tests")],
            ),
        ];
        let mut ir = program();
        ir.tasks[0].body.truncate(2);
        let Stmt::Assign { value, .. } = &mut ir.tasks[0].body[0] else {
            panic!("assignment")
        };
        let Expr::Judge(judge) = value else {
            panic!("judgment")
        };
        judge.each = true;
        assert!(
            check_batching_with_profile(&ir, &events, &profile(2)).is_empty(),
            "{:?}",
            check_batching_with_profile(&ir, &events, &profile(2))
        );

        // Spec 6.6: total overflow alone does not allow splitting. With only
        // two `a` elements, that `each` fits the cap even though `b` does not.
        let mut invalid = events;
        let Event::Request { questions, .. } = &mut invalid[1].event else {
            panic!("request")
        };
        questions.remove(0);
        assert!(!check_batching_with_profile(&ir, &invalid, &profile(2)).is_empty());
    }

    #[test]
    fn item_2_profile_cap_rejects_short_splits_and_oversized_chunks() {
        // Spec 6.5: splitting is automatic only at the selected profile's cap.
        let mut ir = program();
        ir.tasks[0].body.truncate(2);
        let Stmt::Assign { value, .. } = &mut ir.tasks[0].body[0] else {
            panic!("assignment")
        };
        let Expr::Judge(judge) = value else {
            panic!("judgment")
        };
        judge.each = true;

        let short_split = vec![
            request(
                0,
                json!({"obs": {"summary": ["s"]}}),
                vec![noul("a[0]", "obs.summary[0]")],
            ),
            request(
                1,
                json!({"obs": {"tests": "t"}}),
                vec![noul("b", "obs.tests")],
            ),
        ];
        assert!(!check_batching_with_profile(&ir, &short_split, &profile(3)).is_empty());

        let oversized = vec![request(
            0,
            json!({"obs": {"summary": ["s0", "s1"], "tests": "t"}}),
            vec![
                noul("a[0]", "obs.summary[0]"),
                noul("a[1]", "obs.summary[1]"),
                noul("b", "obs.tests"),
            ],
        )];
        assert!(!check_batching_with_profile(&ir, &oversized, &profile(2)).is_empty());
    }

    #[test]
    fn item_2_default_check_uses_the_recorded_profile_cap() {
        // Spec 10.3 makes current recordings self-contained. The default API
        // must therefore enforce their recorded cap without host configuration,
        // while a legacy event stream retains the cap-independent fallback.
        let mut ir = program();
        ir.tasks[0].body.truncate(2);
        let Stmt::Assign { value, .. } = &mut ir.tasks[0].body[0] else {
            panic!("assignment")
        };
        let Expr::Judge(judge) = value else {
            panic!("judgment")
        };
        judge.each = true;
        let oversized = request(
            1,
            json!({"obs": {"summary": ["s0", "s1"], "tests": "t"}}),
            vec![
                noul("a[0]", "obs.summary[0]"),
                noul("a[1]", "obs.summary[1]"),
                noul("b", "obs.tests"),
            ],
        );
        assert!(check_recording(&ir, std::slice::from_ref(&oversized)).is_empty());

        let start = RecordedEvent {
            ts: String::new(),
            run_id: "run_1".into(),
            seq: 0,
            execution: Some(jevscript_runtime::record::ExecutionMetadata {
                ir: ir.clone(),
                profile: profile(2),
                sample: jevscript_runtime::rpc::Sample::On(false),
            }),
            event: Event::Start {
                program: "p".into(),
                task: "main".into(),
                inputs: BTreeMap::new(),
                bindings: Vec::new(),
                ir_version: ir.ir_version.clone(),
            },
        };
        let violations = check_recording(&ir, &[start, oversized]);
        assert!(
            violations.iter().any(|violation| violation.item == 2),
            "{violations:?}"
        );
    }

    #[test]
    fn item_2_empty_each_emits_no_question_but_not_an_empty_request() {
        // Spec 6.5: zero elements contribute zero questions; the other ask in
        // the group still travels normally, while an empty request is invalid.
        let mut ir = program();
        ir.tasks[0].body.truncate(2);
        let Stmt::Assign { value, .. } = &mut ir.tasks[0].body[0] else {
            panic!("assignment")
        };
        let Expr::Judge(judge) = value else {
            panic!("judgment")
        };
        judge.each = true;
        let good = vec![request(
            0,
            json!({"obs": {"tests": "t"}}),
            vec![noul("b", "obs.tests")],
        )];
        assert!(check_batching(&ir, &good).is_empty());

        let bad = vec![request(0, json!({}), Vec::new())];
        assert!(
            check_batching(&ir, &bad)
                .iter()
                .any(|violation| violation.message.contains("empty `request`"))
        );
    }

    #[test]
    fn item_2_ambiguous_names_are_matched_as_complete_groups() {
        // A shared answer id must not be assigned to the lexicographically
        // first group when another complete group is the observable match.
        let mut ir = Ir::empty("p");
        let task = |name: &str, roots: &[&str]| Task {
            name: name.into(),
            params: Vec::new(),
            budget: Budget::default(),
            thresholds: Thresholds::default(),
            body: roots
                .iter()
                .map(|root| Stmt::Assign {
                    root: (*root).into(),
                    path: Vec::new(),
                    value: Expr::Judge(Box::new(judge("obs", Some(root), 0))),
                    span: Span::default(),
                })
                .collect(),
            span: Span::default(),
        };
        ir.tasks.push(task("a_long", &["x", "y"]));
        ir.tasks.push(task("z_short", &["x"]));
        let events = vec![request(
            0,
            json!({"obs": {"x": "x"}}),
            vec![noul("x", "obs.x")],
        )];
        assert!(check_batching(&ir, &events).is_empty());
    }

    #[test]
    fn item_2_duplicate_or_non_contiguous_each_ids_are_violations() {
        let duplicates = vec![request(
            0,
            json!({"obs": {"summary": "s", "tests": "t"}}),
            vec![noul("a", "obs.summary"), noul("a", "obs.tests")],
        )];
        assert!(
            check_batching(&program(), &duplicates)
                .iter()
                .any(|violation| violation.message.contains("duplicate"))
        );

        let mut ir = program();
        ir.tasks[0].body.truncate(2);
        let Stmt::Assign { value, .. } = &mut ir.tasks[0].body[0] else {
            panic!("assignment")
        };
        let Expr::Judge(judge) = value else {
            panic!("judgment")
        };
        judge.each = true;
        let gap = vec![request(
            0,
            json!({"obs": {"summary": ["s0", null, "s2"], "tests": "t"}}),
            vec![
                noul("a[0]", "obs.summary[0]"),
                noul("a[2]", "obs.summary[2]"),
                noul("b", "obs.tests"),
            ],
        )];
        assert!(!check_batching(&ir, &gap).is_empty());
    }

    #[test]
    fn item_3_exactly_the_subjects_pass() {
        let events = vec![request(
            0,
            json!({"obs": {"summary": "s", "tests": "t"}}),
            vec![noul("a", "obs.summary"), noul("b", "obs.tests")],
        )];
        assert!(check_state(&program(), &events).is_empty());
    }

    #[test]
    fn item_3_an_unnamed_root_or_field_is_a_violation() {
        // Spec 6.9: a variable in scope that no subject names does not reach Jev.
        let events = vec![request(
            0,
            json!({"obs": {"summary": "s", "files": []}, "issue": {}}),
            vec![noul("a", "obs.summary")],
        )];
        let violations = check_state(&program(), &events);
        let messages: Vec<&str> = violations.iter().map(|v| v.message.as_str()).collect();
        assert!(
            messages.iter().any(|m| m.contains("`issue`")),
            "{messages:?}"
        );
        assert!(
            messages.iter().any(|m| m.contains("obs.files")),
            "{messages:?}"
        );
    }

    #[test]
    fn item_3_an_indexed_list_carries_null_elsewhere() {
        let ok = vec![request(
            0,
            json!({"files": [null, null, "c"]}),
            vec![noul("x", "files[2]")],
        )];
        assert!(check_state(&program(), &ok).is_empty());
        let bad = vec![request(
            0,
            json!({"files": ["a", null, "c"]}),
            vec![noul("x", "files[2]")],
        )];
        assert_eq!(check_state(&program(), &bad).len(), 1);
    }

    #[test]
    fn item_3_compare_paths_are_allowed_in_the_state() {
        let question = Question::Noul {
            id: "x".into(),
            path: "obs.summary".into(),
            condition: "holds".into(),
            instruction: Some(Instruction {
                compare: vec!["obs.tests".into()],
                ..Instruction::default()
            }),
        };
        let events = vec![request(
            0,
            json!({"obs": {"summary": "s", "tests": "t"}}),
            vec![question],
        )];
        assert!(check_state(&program(), &events).is_empty());
    }

    #[test]
    fn item_3_named_judgment_sends_all_parameters_in_full() {
        // Spec 6.9 gives named judgments a distinct rule: parameters, even an
        // unused one, are full state fields rather than trimmed subject paths.
        let mut ir = Ir::empty("p");
        ir.judgments.push(Judgment {
            name: "inspect".into(),
            params: vec!["obs".into(), "context".into()],
            results: vec![JudgmentResult {
                name: "clear".into(),
                question: judge("obs", Some("summary"), 0),
                span: Span::default(),
            }],
            logs: Vec::new(),
            shape_hash: String::new(),
            span: Span::default(),
        });
        let good = vec![request(
            0,
            json!({
                "obs": {"summary": "s", "tests": "t"},
                "context": {"issue": 7}
            }),
            vec![noul("clear", "obs.summary")],
        )];
        assert!(check_state(&ir, &good).is_empty());

        let missing = vec![request(
            0,
            json!({"obs": {"summary": "s", "tests": "t"}}),
            vec![noul("clear", "obs.summary")],
        )];
        assert!(
            check_state(&ir, &missing)
                .iter()
                .any(|violation| violation.message.contains("`context`"))
        );
    }

    #[test]
    fn item_3_machine_state_has_the_fixed_section_7_8_shape() {
        let choice = Question::Choice {
            id: "event".into(),
            path: "obs".into(),
            question: Some("next event".into()),
            labels: vec![
                jevscript_runtime::jev::ChoiceLabel::described("finished", "done"),
                jevscript_runtime::jev::ChoiceLabel::described("stay", "wait"),
            ],
            instruction: Some(Instruction {
                compare: vec!["state".into(), "goal".into(), "recent".into()],
                ..Instruction::default()
            }),
        };
        let good = vec![request(
            0,
            json!({
                "state": "working",
                "goal": "finish review",
                "obs": {"summary": "s"},
                "recent": [{"step": 0, "from": "new", "event": "start", "to": "working"}]
            }),
            vec![choice.clone()],
        )];
        assert!(check_state(&program(), &good).is_empty());

        let bad = vec![request(
            0,
            json!({
                "state": "working",
                "goal": "finish review",
                "obs": {"summary": "s"},
                "recent": [{"step": 0, "from": "new", "event": "start"}],
                "unrelated": true
            }),
            vec![choice],
        )];
        let messages: Vec<String> = check_state(&program(), &bad)
            .into_iter()
            .map(|violation| violation.message)
            .collect();
        assert!(messages.iter().any(|message| message.contains("unrelated")));
        assert!(messages.iter().any(|message| message.contains("recent[0]")));
    }

    #[test]
    fn item_3_missing_nested_fields_and_indices_are_violations() {
        let missing_field = vec![request(
            0,
            json!({"obs": {}}),
            vec![noul("x", "obs.summary")],
        )];
        assert!(
            check_state(&program(), &missing_field)
                .iter()
                .any(|violation| violation.message.contains("obs.summary"))
        );

        let missing_index = vec![request(
            0,
            json!({"files": [null]}),
            vec![noul("x", "files[2]")],
        )];
        assert!(
            check_state(&program(), &missing_index)
                .iter()
                .any(|violation| violation.message.contains("files[2]"))
        );

        let wrong_container = vec![request(
            0,
            json!({"files": {}}),
            vec![noul("x", "files[2]")],
        )];
        assert!(
            check_state(&program(), &wrong_container)
                .iter()
                .any(|violation| violation.message.contains("indexes it as a list"))
        );
    }

    fn step(seq: u64, enabled: &[&str], chosen: &str, to: &str) -> RecordedEvent {
        RecordedEvent {
            ts: String::new(),
            run_id: "run_1".into(),
            seq,
            execution: None,
            event: Event::MachineStep {
                machine: "review".into(),
                state: "working".into(),
                enabled: enabled.iter().map(|s| (*s).to_string()).collect(),
                chosen: chosen.into(),
                probabilities: enabled.iter().map(|s| ((*s).to_string(), 0.5)).collect(),
                confidence: 0.5,
                to: to.into(),
            },
        }
    }

    #[test]
    fn item_6_a_menu_of_enabled_events_plus_stay_passes() {
        let events = vec![step(0, &["finished", "stay"], "finished", "approved")];
        assert!(check_machine_menus(&program(), &events).is_empty());
    }

    #[test]
    fn item_6_missing_stay_or_an_undeclared_event_is_a_violation() {
        let no_stay = vec![step(0, &["finished"], "finished", "approved")];
        assert!(
            check_machine_menus(&program(), &no_stay)
                .iter()
                .any(|v| v.message.contains("`stay`"))
        );
        let foreign = vec![step(0, &["approved", "stay"], "stay", "working")];
        assert!(
            check_machine_menus(&program(), &foreign)
                .iter()
                .any(|v| v.message.contains("not an event"))
        );
    }

    #[test]
    fn item_6_unguarded_events_and_duplicate_labels_are_checked() {
        // Spec 7.8: unguarded transitions are always enabled. The checker does
        // not guess the truth of arbitrary guarded transitions.
        let omitted = vec![step(0, &["stay"], "stay", "working")];
        assert!(
            check_machine_menus(&program(), &omitted)
                .iter()
                .any(|violation| violation.message.contains("unguarded event"))
        );

        let duplicated = vec![step(0, &["finished", "stay", "stay"], "stay", "working")];
        assert!(
            check_machine_menus(&program(), &duplicated)
                .iter()
                .any(|violation| violation.message.contains("duplicate event"))
        );
    }

    #[test]
    fn item_6_guarded_event_presence_is_not_inferred_from_the_menu() {
        let mut ir = program();
        ir.machines[0].states[0].transitions.push(Transition {
            event: "needs_work".into(),
            description: text("more work"),
            target: "working".into(),
            when: Some(Expr::Name {
                name: "guard_result".into(),
                span: Span::default(),
            }),
            risky: false,
            body: Vec::new(),
            span: Span::default(),
        });
        let events = vec![step(0, &["finished", "stay"], "stay", "working")];
        assert!(check_machine_menus(&ir, &events).is_empty());
    }

    #[test]
    fn item_6_no_enabled_sentinel_requires_its_matching_pause() {
        // Spec 7.8: an empty distribution is not a Jev result. It is valid
        // only as the explicit sentinel immediately before the machine's
        // `no_enabled_events` escalation.
        let mut ir = program();
        ir.machines[0].states[0].transitions[0].when = Some(Expr::Bool {
            value: false,
            span: Span::default(),
        });
        let mut sentinel = step(0, &["stay"], "stay", "working");
        let Event::MachineStep {
            probabilities,
            confidence,
            ..
        } = &mut sentinel.event
        else {
            unreachable!()
        };
        probabilities.clear();
        *confidence = 0.0;
        let pause = RecordedEvent {
            ts: String::new(),
            run_id: "run_1".into(),
            seq: 1,
            execution: None,
            event: Event::Pause {
                kind: PauseKind::Escalate,
                payload: Pause::Escalate {
                    common: jevscript_runtime::pause::PauseCommon {
                        run_id: "run_1".into(),
                        step: 0,
                        task: "machine:review".into(),
                        state: Some("working".into()),
                        source: Span::default(),
                        recording_offset: 0,
                    },
                    reason: "no_enabled_events".into(),
                    context: BTreeMap::new(),
                },
            },
        };

        assert!(check_machine_menus(&ir, &[sentinel.clone(), pause]).is_empty());
        let violations = check_machine_menus(&ir, &[sentinel]);
        assert!(
            violations
                .iter()
                .any(|violation| violation.message.contains("matching `no_enabled_events`")),
            "{violations:?}"
        );
    }

    #[test]
    fn item_6_enabled_events_preserve_declaration_order_and_stay_last() {
        let mut ir = program();
        ir.machines[0].states[0].transitions.push(Transition {
            event: "rejected".into(),
            description: text("needs more work"),
            target: "working".into(),
            when: None,
            risky: false,
            body: Vec::new(),
            span: Span::default(),
        });
        let reversed = vec![step(
            0,
            &["rejected", "finished", "stay"],
            "finished",
            "approved",
        )];
        let violations = check_machine_menus(&ir, &reversed);
        assert!(
            violations
                .iter()
                .any(|violation| violation.message.contains("declaration order")),
            "{violations:?}"
        );

        let misplaced_stay = vec![step(
            0,
            &["finished", "stay", "rejected"],
            "finished",
            "approved",
        )];
        assert!(
            check_machine_menus(&ir, &misplaced_stay)
                .iter()
                .any(|violation| violation.message.contains("`stay` last"))
        );
    }

    #[test]
    fn item_4_pause_sequences_are_compared_in_order() {
        use jevscript_runtime::pause::{Pause, PauseCommon, PauseKind};
        let pause = |seq: u64, reason: &str| RecordedEvent {
            ts: String::new(),
            run_id: "run_1".into(),
            seq,
            execution: None,
            event: Event::Pause {
                kind: PauseKind::Stopped,
                payload: Pause::Stopped {
                    common: PauseCommon {
                        run_id: "run_1".into(),
                        step: seq,
                        task: "main".into(),
                        state: None,
                        source: Span::default(),
                        recording_offset: 0,
                    },
                    reason: reason.into(),
                },
            },
        };
        let a = vec![pause(1, "complete")];
        assert_eq!(pauses_diverge_at(&a, &[pause(1, "complete")]), None);
        // Same kind/task/step but a mutated kind-specific payload must fail.
        assert_eq!(pauses_diverge_at(&a, &[pause(1, "different")]), Some(0));
    }

    fn hard_coded_profile_values(root: &std::path::Path) -> Vec<String> {
        let mut offenders = Vec::new();
        let mut pending = vec![root.to_path_buf()];
        while let Some(directory) = pending.pop() {
            for entry in std::fs::read_dir(&directory).expect("source directory") {
                let path = entry.expect("entry").path();
                if path.is_dir() {
                    pending.push(path);
                    continue;
                }
                if path.extension().is_some_and(|extension| extension == "rs") {
                    let text = std::fs::read_to_string(&path).expect("read source");
                    for literal in ["64000", "32000", "64_000", "32_000", "0.042"] {
                        if text.contains(literal) {
                            offenders.push(format!("{}: {literal}", path.display()));
                        }
                    }
                }
            }
        }
        offenders
    }

    #[test]
    fn item_7_no_limit_or_price_is_written_into_runtime_code() {
        // Spec 10.6: limits live in profiles. The bundled numbers must not
        // appear in top-level or nested Rust modules in the runtime.
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../jevscript-runtime/src");
        let offenders = hard_coded_profile_values(&src);
        assert!(offenders.is_empty(), "{offenders:?}");
    }

    #[test]
    fn item_7_limit_scan_descends_into_nested_modules() {
        let root = std::env::temp_dir().join(format!(
            "jevscript-conformance-limit-scan-{}",
            std::process::id()
        ));
        let nested = root.join("nested/deeper");
        std::fs::create_dir_all(&nested).expect("create fixture");
        std::fs::write(
            root.join("clean.rs"),
            "const RECURSION_LIMIT: usize = 64;\n\
             const MACHINE_RECENT_EVENTS: usize = 6;\n\
             const TRAIL_ARGS_TOKENS: u64 = 200;",
        )
        .expect("write clean fixture");
        std::fs::write(nested.join("limits.rs"), "const TOKENS: u64 = 64_000;")
            .expect("write bad fixture");
        let offenders = hard_coded_profile_values(&root);
        std::fs::remove_dir_all(&root).expect("remove fixture");
        assert_eq!(offenders.len(), 1, "{offenders:?}");
        assert!(offenders[0].contains("nested/deeper/limits.rs"));
        assert!(
            !offenders[0].contains("clean.rs"),
            "normative language constants are not profile-owned limits"
        );
    }

    fn logged(seq: u64, message: &str) -> RecordedEvent {
        RecordedEvent {
            ts: String::new(),
            run_id: "run_1".into(),
            seq,
            execution: None,
            event: jevscript_runtime::record::LogRecord {
                level: jevscript_ir::LogLevel::Info,
                message: message.into(),
                fields: BTreeMap::new(),
                task: "main".into(),
                source: Span::default(),
            }
            .into_event(),
        }
    }

    fn answered(seq: u64, request_seq: u64) -> RecordedEvent {
        RecordedEvent {
            ts: String::new(),
            run_id: "run_1".into(),
            seq,
            execution: None,
            event: Event::Answers {
                request_id: format!("req_{request_seq}"),
                answers: Vec::new(),
                usage: jevscript_runtime::jev::JevUsage {
                    tokens: 0,
                    usd: None,
                },
                latency_ms: 0,
                sampled: false,
            },
        }
    }

    #[test]
    fn item_9_a_log_is_never_recorded_inside_an_open_request() {
        // Spec 5.8: a log is recorded on its own, never while a request is out.
        let events = vec![
            logged(0, "before"),
            request(1, json!({"m": "x"}), Vec::new()),
            answered(2, 1),
            logged(3, "after"),
        ];
        assert!(check_logs(&events).is_empty());
        let inside = vec![
            request(0, json!({"m": "x"}), Vec::new()),
            logged(1, "inside"),
            answered(2, 0),
        ];
        let violations = check_logs(&inside);
        assert_eq!(violations.len(), 1, "{violations:?}");
        assert_eq!(violations[0].item, 9);
        assert_eq!(violations[0].seq, Some(1));
    }

    #[test]
    fn item_9_a_replay_must_reproduce_the_same_logs() {
        // Spec 5.8 and 10.4: the same lines, in the same order.
        let original = vec![logged(0, "a"), logged(1, "b")];
        assert_eq!(logs_diverge_at(&original, &original.clone()), None);
        assert_eq!(
            logs_diverge_at(&original, &[logged(0, "a"), logged(1, "changed")]),
            Some(1)
        );
        assert_eq!(logs_diverge_at(&original, &[logged(0, "a")]), Some(1));
        assert_eq!(log_sequence(&original).len(), 2);
    }
}
