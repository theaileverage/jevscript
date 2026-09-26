//! Machine execution (spec section 7.8).
//!
//! A machine is walked from the beginning on every resume, just like a task.
//! Requests, decisions, pauses and action effects are served from the driver's
//! log until it runs out, so the local state and event history are rebuilt
//! deterministically without retaining a continuation (spec section 10.4).

use std::collections::BTreeMap;

use jevscript_ir::{BudgetKey, Machine, Transition, Verdict};

use crate::answer;
use crate::capability::CallArgs;
use crate::error::{RuntimeError, RuntimeErrorCode};
use crate::jev::{ChoiceLabel, Instruction, JevAnswer, JevRequest, Question};
use crate::pause::{Pause, Resume};
use crate::record::Event;
use crate::size;
use crate::value::Value;

use super::{
    DEFAULT_CALLS, Exec, Flow, Frame, FrameKind, GATE_CONFIRM_MESSAGE, GATE_ESCALATE_REASON,
    GATE_STOP_REASON, Interpreter, TaskFrame,
};

const MACHINE_QUESTION_ID: &str = "event";
const MACHINE_QUESTION: &str = "Which enabled event should happen next to advance the goal?";
const STAY: &str = "stay";
const STAY_DESCRIPTION: &str = "nothing in the observation calls for a transition yet";
const NO_ENABLED_EVENTS: &str = "no_enabled_events";
const RECENT_EVENTS: usize = 6;

/// One completed machine decision, used by `recent` and the result (7.8).
#[derive(Clone)]
struct MachineEvent {
    step: u64,
    from: String,
    event: String,
    to: String,
}

impl MachineEvent {
    fn value(&self) -> Value {
        Value::Record(BTreeMap::from([
            ("step".to_string(), Value::Number(self.step as f64)),
            ("from".to_string(), Value::Text(self.from.clone())),
            ("event".to_string(), Value::Text(self.event.clone())),
            ("to".to_string(), Value::Text(self.to.clone())),
        ]))
    }
}

impl<'a, 'c> Interpreter<'a, 'c> {
    /// Run a machine call to a terminal state or its required `max` bound.
    pub(super) fn call_machine(
        &mut self,
        machine: &'a Machine,
        mut args: CallArgs,
    ) -> Result<Value, RuntimeError> {
        let max = args
            .named
            .remove("max")
            .and_then(|value| value.as_number())
            .ok_or_else(|| {
                RuntimeError::new(
                    RuntimeErrorCode::TypeError,
                    format!(
                        "machine `{}` needs the numeric named argument `max`",
                        machine.name
                    ),
                )
                .at(machine.span)
            })?;
        let module = self.module_of(&machine.name);
        let mut env = self.env_for(&module);
        self.bind_params(&mut env, &machine.name, &machine.params, args)?;
        if let Err(halt) = self.enter_machine(machine, module) {
            return Err(self.halted(halt));
        }
        let result = self.exec_machine(machine, &mut env, max);
        self.frames.pop();
        match result {
            Ok(value) => Ok(value),
            Err(halt) => Err(self.halted(halt)),
        }
    }

    fn enter_machine(&mut self, machine: &Machine, module: String) -> Exec<()> {
        let started_ms = self.clock_ms()?;
        let mut budget = machine.budget.clone();
        if budget.calls.is_none() {
            budget.calls = Some(DEFAULT_CALLS);
        }
        let id = self.allocate_frame_id();
        self.frames.push(Frame {
            id,
            unit: machine.name.clone(),
            module,
            kind: FrameKind::Machine(Box::new(TaskFrame {
                budget,
                thresholds: machine.thresholds.clone(),
                has_verify: false,
                verify_reached: false,
                verified: false,
                gate_done: false,
                calls: 0.0,
                usd: 0.0,
                steps: 0.0,
                started_ms,
                paused_ms: 0,
                trail: Vec::new(),
                pending_steps: Vec::new(),
                next_step: 1,
            })),
            state: Some(machine.initial.clone()),
            loop_depth: 0,
        });
        Ok(())
    }

    fn exec_machine(
        &mut self,
        machine: &'a Machine,
        env: &mut crate::eval::Env,
        max: f64,
    ) -> Exec<Value> {
        let mut current = machine.initial.clone();
        let mut steps = 0_u64;
        let mut events = Vec::new();
        let mut verified = false;

        loop {
            self.set_machine_state(&current);
            let Some(state) = machine.states.iter().find(|state| state.name == current) else {
                let error = RuntimeError::new(
                    RuntimeErrorCode::TypeError,
                    format!("machine `{}` has no state `{current}`", machine.name),
                )
                .at(machine.span);
                return Err(self.end_with_error(error));
            };
            self.span = state.span;
            if state.done || steps as f64 >= max {
                return Ok(machine_result(
                    &current, steps, state.done, verified, &events,
                ));
            }
            self.preflight_machine_step()?;

            let obs = self.machine_observe(env, &machine.observe)?;
            env.assign("obs", obs.clone());
            let mut enabled = Vec::new();
            for transition in &state.transitions {
                self.span = transition.span;
                if match &transition.when {
                    Some(guard) => self.truthy(env, guard)?,
                    None => true,
                } {
                    enabled.push(transition);
                }
            }

            let step = steps + 1;
            if enabled.is_empty() {
                self.emit_machine_step(Event::MachineStep {
                    machine: machine.name.clone(),
                    state: current.clone(),
                    enabled: vec![STAY.to_string()],
                    chosen: STAY.to_string(),
                    probabilities: BTreeMap::new(),
                    confidence: 0.0,
                    to: current.clone(),
                })?;
                self.pause(Pause::Escalate {
                    common: self.common(state.span),
                    reason: NO_ENABLED_EVENTS.to_string(),
                    context: BTreeMap::new(),
                })?;
                // Spec 7.8 records before the pause, but only an explicit
                // resume completes and counts this no-decision stay.
                self.complete_machine_step()?;
                steps = step;
                events.push(MachineEvent {
                    step,
                    from: current.clone(),
                    event: STAY.to_string(),
                    to: current.clone(),
                });
                continue;
            }

            // Spec 7.8 step 5: a request field is evaluated only for a step
            // that will actually send a request. Terminal, exhausted and
            // no-enabled steps return or pause before touching the goal.
            let goal = match &machine.goal {
                Some(goal) => self.eval(env, goal)?,
                None => Value::None,
            };
            let request = self.machine_request(env, &current, &goal, &obs, &events, &enabled)?;
            let response = self.jev_request(&request, self.sample_run);
            let response = self.lift(response)?;
            let (chosen, probabilities, confidence) =
                self.machine_choice(response.answers, &enabled)?;
            let transition = enabled.iter().find(|t| t.event == chosen).copied();
            let risk = transition.is_some_and(|t| t.risky);
            let verdict = machine_verdict(&machine.thresholds, risk, confidence);
            let mut permitted = transition;

            match verdict {
                Verdict::Proceed => {}
                Verdict::Confirm => {
                    self.span = transition.map_or(state.span, |t| t.span);
                    let resume = self.pause(Pause::Confirm {
                        common: self.common(self.span),
                        message: GATE_CONFIRM_MESSAGE.to_string(),
                        options: Pause::default_options(),
                        context: BTreeMap::from([
                            ("risk".to_string(), Value::Number(f64::from(risk))),
                            ("confidence".to_string(), Value::Number(confidence)),
                        ]),
                    })?;
                    if !matches!(resume, Resume::Answer { answer, .. } if answer == "yes") {
                        permitted = None;
                    }
                }
                Verdict::Escalate => {
                    self.span = transition.map_or(state.span, |t| t.span);
                    self.pause(Pause::Escalate {
                        common: self.common(self.span),
                        reason: GATE_ESCALATE_REASON.to_string(),
                        context: BTreeMap::from([
                            ("risk".to_string(), Value::Number(f64::from(risk))),
                            ("confidence".to_string(), Value::Number(confidence)),
                        ]),
                    })?;
                    // Section 7.8 settles explicit escalation resumption as
                    // a counted stay: the action does not run and we observe
                    // again on the next step.
                    permitted = None;
                }
                Verdict::Stop => permitted = None,
            }

            let to = permitted.map_or_else(|| current.clone(), |t| t.target.clone());
            self.complete_machine_step()?;
            steps = step;
            self.emit_machine_step(Event::MachineStep {
                machine: machine.name.clone(),
                state: current.clone(),
                enabled: enabled
                    .iter()
                    .map(|t| t.event.clone())
                    .chain([STAY.to_string()])
                    .collect(),
                chosen: chosen.clone(),
                probabilities,
                confidence,
                to: to.clone(),
            })?;
            let effective_event = if permitted.is_some() {
                chosen.clone()
            } else {
                STAY.to_string()
            };
            events.push(MachineEvent {
                step,
                from: current.clone(),
                event: effective_event,
                to: to.clone(),
            });

            if verdict == Verdict::Stop {
                return Err(self.stop(GATE_STOP_REASON.to_string(), self.span));
            }
            let Some(transition) = permitted else {
                continue;
            };
            self.span = transition.span;
            let _flow: Flow = self.exec_block(env, &transition.body)?;
            current = transition.target.clone();
            verified = transition.when.is_some()
                && machine
                    .states
                    .iter()
                    .find(|candidate| candidate.name == current)
                    .is_some_and(|candidate| candidate.done);
        }
    }

    fn machine_observe(
        &mut self,
        env: &mut crate::eval::Env,
        fields: &[jevscript_ir::ShapeField],
    ) -> Exec<Value> {
        let mut record = BTreeMap::new();
        for field in fields {
            self.span = field.span;
            let value = self.eval(env, &field.value)?;
            let value = match field.max {
                Some(max) => self.cap(env, field, value, max, false)?,
                None => value,
            };
            record.insert(field.name.clone(), value);
        }
        Ok(Value::Record(record))
    }

    fn machine_request(
        &mut self,
        env: &mut crate::eval::Env,
        state: &str,
        goal: &Value,
        obs: &Value,
        events: &[MachineEvent],
        enabled: &[&'a Transition],
    ) -> Exec<JevRequest> {
        let criteria = enabled.len() + 1;
        if criteria > self.profile.max_criteria_per_question as usize {
            let error = RuntimeError::new(
                RuntimeErrorCode::PickTooMany,
                format!(
                    "the machine menu has {criteria} labels, over the {} allowed by `{}`",
                    self.profile.max_criteria_per_question, self.profile.model
                ),
            )
            .at(self.span);
            return Err(self.end_with_error(error));
        }
        let labels = enabled
            .iter()
            .map(|transition| {
                self.eval_text(env, &transition.description)
                    .map(|description| {
                        ChoiceLabel::described(transition.event.clone(), description)
                    })
            })
            .collect::<Exec<Vec<_>>>()?
            .into_iter()
            .chain([ChoiceLabel::described(STAY, STAY_DESCRIPTION)])
            .collect();
        let skip = events.len().saturating_sub(RECENT_EVENTS);
        let recent: Vec<Value> = events[skip..].iter().map(MachineEvent::value).collect();
        let request = JevRequest {
            state: BTreeMap::from([
                (
                    "state".to_string(),
                    serde_json::Value::String(state.to_string()),
                ),
                ("goal".to_string(), goal.to_json()),
                ("obs".to_string(), obs.to_json()),
                ("recent".to_string(), Value::List(recent).to_json()),
            ]),
            model: self.profile.model.clone(),
            questions: vec![Question::Choice {
                id: MACHINE_QUESTION_ID.to_string(),
                path: "obs".to_string(),
                question: Some(MACHINE_QUESTION.to_string()),
                labels,
                instruction: Some(Instruction {
                    compare: vec![
                        "state".to_string(),
                        "goal".to_string(),
                        "recent".to_string(),
                    ],
                    ..Instruction::default()
                }),
            }],
        };
        match size::check_request(&request, self.profile) {
            Ok(_) => Ok(request),
            Err(error) => Err(self.end_with_error(error)),
        }
    }

    fn machine_choice(
        &mut self,
        answers: Vec<JevAnswer>,
        enabled: &[&Transition],
    ) -> Exec<(String, BTreeMap<String, f64>, f64)> {
        let answer = answers
            .into_iter()
            .find(|answer| answer.id() == MACHINE_QUESTION_ID);
        let Some(JevAnswer::Choice {
            label,
            confidence,
            probabilities,
            ..
        }) = answer
        else {
            let error = RuntimeError::new(
                RuntimeErrorCode::JevRejected,
                "machine question `event` did not receive a Choice answer",
            )
            .at(self.span);
            return Err(self.end_with_error(error));
        };
        let labels: Vec<String> = enabled
            .iter()
            .map(|transition| transition.event.clone())
            .chain([STAY.to_string()])
            .collect();
        if !labels.contains(&label) {
            let error = RuntimeError::new(
                RuntimeErrorCode::JevRejected,
                format!("machine question `event` returned unknown label `{label}`"),
            )
            .at(self.span);
            return Err(self.end_with_error(error));
        }
        if let Some(label) = probabilities.keys().find(|label| !labels.contains(label)) {
            let error = RuntimeError::new(
                RuntimeErrorCode::JevRejected,
                format!(
                    "machine question `event` returned a probability for unknown label `{label}`"
                ),
            )
            .at(self.span);
            return Err(self.end_with_error(error));
        }
        let chosen = if self.sample_run {
            let draw = match self.driver.draw_random(self.span) {
                Ok(draw) => draw,
                Err(error) => return Err(self.end_with_error(error)),
            };
            answer::draw_key(&labels, &probabilities, draw)
        } else {
            label
        };
        if !labels.contains(&chosen) {
            let error = RuntimeError::new(
                RuntimeErrorCode::JevRejected,
                format!("machine question `event` returned unknown label `{chosen}`"),
            )
            .at(self.span);
            return Err(self.end_with_error(error));
        }
        Ok((chosen, probabilities, confidence))
    }

    fn preflight_machine_step(&mut self) -> Exec<()> {
        self.check(BudgetKey::Steps, 1.0)?;
        Ok(())
    }

    fn complete_machine_step(&mut self) -> Exec<()> {
        // Observation and guards may call a nested machine, whose completed
        // steps count against this frame (7.1). Re-check the reservation made
        // before observation so this step cannot overrun what remains.
        self.check(BudgetKey::Steps, 1.0)?;
        self.add(BudgetKey::Steps, 1.0);
        Ok(())
    }

    fn emit_machine_step(&mut self, event: Event) -> Exec<()> {
        match self.driver.emit(event, self.span) {
            Ok(()) => Ok(()),
            Err(error) => Err(self.end_with_error(error)),
        }
    }

    fn set_machine_state(&mut self, state: &str) {
        if let Some(frame) = self
            .frames
            .iter_mut()
            .rev()
            .find(|frame| matches!(frame.kind, FrameKind::Machine(_)))
        {
            frame.state = Some(state.to_string());
        }
    }
}

fn machine_verdict(thresholds: &jevscript_ir::Thresholds, risky: bool, confidence: f64) -> Verdict {
    if thresholds
        .stop_confidence
        .is_some_and(|threshold| confidence < threshold)
    {
        Verdict::Stop
    } else if thresholds
        .risk_confirm
        .is_some_and(|threshold| f64::from(risky) >= threshold)
    {
        Verdict::Confirm
    } else if thresholds
        .min_confidence
        .is_some_and(|threshold| confidence < threshold)
    {
        Verdict::Escalate
    } else {
        Verdict::Proceed
    }
}

fn machine_result(
    state: &str,
    steps: u64,
    done: bool,
    verified: bool,
    events: &[MachineEvent],
) -> Value {
    Value::Record(BTreeMap::from([
        ("state".to_string(), Value::Text(state.to_string())),
        ("steps".to_string(), Value::Number(steps as f64)),
        ("done".to_string(), Value::Bool(done)),
        ("verified".to_string(), Value::Bool(done && verified)),
        (
            "events".to_string(),
            Value::List(events.iter().map(MachineEvent::value).collect()),
        ),
    ]))
}
