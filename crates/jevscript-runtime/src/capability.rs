//! Capabilities (spec section 9).
//!
//! Nothing inside a program reaches the world except through a capability the
//! host bound at start. The kind fixes the verbs; `tool` is the open one.
//!
//! Adapters live on the host side of the runtime boundary (spec section 11.5):
//! the runtime never links against tmux, a browser or an agent CLI, and reaches
//! an adapter through the `capability.call` and `capability.observe` JSON-RPC
//! requests. The traits here are what the interpreter sees; the JSON-RPC bridge
//! is one implementation of them, and an in-process adapter is another.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::error::RuntimeError;
use crate::value::{Handle, PauseResult, Value};

/// The four capability kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityKind {
    /// Something that runs a task in the world over time and can be observed.
    Agent,
    /// A human in the loop.
    Person,
    /// A text generation model.
    Llm,
    /// Anything else; verbs are open.
    Tool,
}

impl CapabilityKind {
    /// The verbs this kind accepts, or `None` for `tool`, whose verbs are open
    /// and are checked at run time instead (spec section 9.4).
    pub const fn verbs(self) -> Option<&'static [&'static str]> {
        match self {
            Self::Agent => Some(&["spawn", "observe", "send", "wait", "stop"]),
            Self::Person => Some(&["ask", "notify", "take_over"]),
            Self::Llm => Some(&["write"]),
            Self::Tool => None,
        }
    }

    /// Whether `verb` is defined for this kind. Always true for `tool`.
    pub fn accepts(self, verb: &str) -> bool {
        match self.verbs() {
            Some(verbs) => verbs.contains(&verb),
            None => true,
        }
    }
}

/// What an `agent` observation carries at least (spec section 9.1). An adapter
/// may add fields; programs should `shape` before judging any of them, because
/// `tail` is agent-written text and can contain anything.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Observation {
    /// What the agent is doing.
    pub status: AgentStatus,
    /// The agent's most recent complete message to the user.
    pub last_message: String,
    /// The last screen, or roughly 4k tokens of transcript.
    pub tail: String,
    /// The process's exit code, once it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// Anything else the adapter attached.
    #[serde(default, flatten)]
    pub fields: BTreeMap<String, serde_json::Value>,
}

/// What an observed agent is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[allow(missing_docs, reason = "each variant is the status it names")]
pub enum AgentStatus {
    Running,
    Waiting,
    Exited,
}

/// The arguments of one capability call, positional then named.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct CallArgs {
    /// Positional arguments, in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub positional: Vec<Value>,
    /// Named arguments. Unknown names are passed through to the adapter
    /// unchecked (spec section 9.1).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub named: BTreeMap<String, Value>,
}

impl CallArgs {
    /// The named argument `name`, if it was supplied.
    pub fn named(&self, name: &str) -> Option<&Value> {
        self.named.get(name)
    }

    /// The `index`th positional argument, if it was supplied.
    pub fn positional(&self, index: usize) -> Option<&Value> {
        self.positional.get(index)
    }
}

/// What the interpreter sees of any adapter (spec section 9.5): `call(verb,
/// args)`, and `observe(handle)` for `agent` kinds. Every call and its result is
/// recorded.
///
/// Adapters must return structured records, never opaque blobs, so that `shape`
/// and `trail` can work on them.
pub trait Capability: Send {
    /// Which kind this is, and therefore which verbs the compiler accepted.
    fn kind(&self) -> CapabilityKind;

    /// Run one verb.
    ///
    /// A verb written on a handle (`dev.send "..."`, `dev.wait idle`) reaches
    /// the adapter with that handle as the first positional argument, so an
    /// adapter that owns several agents can tell which one is meant.
    ///
    /// # Errors
    ///
    /// `verb_missing` for a `tool` verb the adapter does not implement, and
    /// `adapter_error` for anything the adapter itself raised.
    fn call(&mut self, verb: &str, args: &CallArgs) -> Result<Value, RuntimeError>;

    /// The manifest a `tool` adapter declares its verbs with, if it has one
    /// (spec section 9.4). The runtime compares the verbs the program uses
    /// against it before the run starts, so a mismatch surfaces before any
    /// model call. Other kinds have fixed verbs and need none.
    fn manifest(&self) -> Option<ToolManifest> {
        None
    }

    /// Observe a handle. `agent` kinds implement this; others do not.
    ///
    /// # Errors
    ///
    /// `adapter_error` if the adapter cannot observe the handle.
    fn observe(&mut self, _handle: &Handle) -> Result<Observation, RuntimeError> {
        Err(RuntimeError::new(
            crate::error::RuntimeErrorCode::VerbMissing,
            "this capability cannot be observed",
        ))
    }
}

/// An `agent`: something that runs a task in the world over time (spec 9.1).
///
/// Implementing this is the ergonomic path; [`Capability`] is what the
/// interpreter calls. The verbs are exactly the spec's.
pub trait AgentCapability: Send {
    /// `spawn in <handle>?, prompt <text>, ...`. Named arguments after `prompt`
    /// are adapter-specific and are passed through unchecked.
    ///
    /// # Errors
    ///
    /// `adapter_error` if the agent cannot be started.
    fn spawn(&mut self, args: &CallArgs) -> Result<Handle, RuntimeError>;

    /// `<handle>.observe`.
    ///
    /// # Errors
    ///
    /// `adapter_error` if the agent cannot be observed.
    fn observe(&mut self, handle: &Handle) -> Result<Observation, RuntimeError>;

    /// `<handle>.send <text>`.
    ///
    /// # Errors
    ///
    /// `adapter_error` if the message cannot be delivered.
    fn send(&mut self, handle: &Handle, text: &str) -> Result<(), RuntimeError>;

    /// `<handle>.wait idle, minutes <n>`. Pauses the run with `waiting`.
    ///
    /// # Errors
    ///
    /// `adapter_error` if the wait cannot be set up.
    fn wait(&mut self, handle: &Handle, minutes: f64) -> Result<Observation, RuntimeError>;

    /// `<handle>.stop`.
    ///
    /// # Errors
    ///
    /// `adapter_error` if the agent cannot be stopped.
    fn stop(&mut self, handle: &Handle) -> Result<(), RuntimeError>;
}

/// A `person`: a human in the loop (spec section 9.2).
pub trait PersonCapability: Send {
    /// `ask <text>, options [<text>...]?`. Pauses the run with `confirm`.
    ///
    /// # Errors
    ///
    /// `adapter_error` if the person cannot be reached.
    fn ask(&mut self, text: &str, options: &[String]) -> Result<PauseResult, RuntimeError>;

    /// `notify <text>`. Sends without pausing.
    ///
    /// # Errors
    ///
    /// `adapter_error` if the person cannot be reached.
    fn notify(&mut self, text: &str) -> Result<(), RuntimeError>;

    /// `take_over`. Pauses the run with `escalate`.
    ///
    /// # Errors
    ///
    /// `adapter_error` if the handover cannot be arranged.
    fn take_over(&mut self) -> Result<(), RuntimeError>;
}

/// An `llm`: a text generation model (spec section 9.3).
///
/// Only the `using` value is sent as context; the model never sees the
/// program's variables. Each `write` counts one call against the budget.
pub trait LlmCapability: Send {
    /// `write "<instruction>" using <value>?`.
    ///
    /// # Errors
    ///
    /// `adapter_error` if the model cannot be reached.
    fn write(&mut self, instruction: &str, using: Option<&Value>) -> Result<String, RuntimeError>;
}

/// A `tool`: anything else (spec section 9.4).
///
/// Verbs are open. A verb the adapter does not implement fails at run time with
/// `verb_missing`, naming the verb, so a program can be checked against an
/// adapter by running it in replay mode with an empty recording.
pub trait ToolCapability: Send {
    /// Run `verb`.
    ///
    /// # Errors
    ///
    /// `verb_missing` if the adapter does not implement `verb`.
    fn call(&mut self, verb: &str, args: &CallArgs) -> Result<Value, RuntimeError>;
}

/// What a host may pass when it binds a `tool` (spec section 9.4).
///
/// At `task.start` the runtime compares every verb the IR references on that
/// capability against the manifest and refuses to start with `verb_missing` if
/// one is absent, so a mismatch surfaces before any model call.
/// `jevscript check --tools <manifest.json>` runs the same comparison without
/// starting a run.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ToolManifest {
    /// Verb name to its signature.
    pub verbs: BTreeMap<String, ManifestVerb>,
}

impl ToolManifest {
    /// The verbs in `wanted` that this manifest does not declare.
    ///
    /// This is the whole of the bind-time check: a verb the adapter never
    /// mentions cannot be called, whatever the program believes.
    pub fn missing<'a>(&self, wanted: impl IntoIterator<Item = &'a str>) -> Vec<String> {
        wanted
            .into_iter()
            .filter(|verb| !self.verbs.contains_key(*verb))
            .map(str::to_string)
            .collect()
    }
}

/// One verb of a [`ToolManifest`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ManifestVerb {
    /// Its positional parameter names.
    #[serde(default)]
    pub params: Vec<String>,
    /// What it returns, as one of the names in spec section 9.4.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub returns: Option<String>,
}

/// The capabilities a run was started with, by program-scoped name.
///
/// A `needs` that is not bound fails the start with `binding_missing` (spec
/// section 3.4).
pub type Bindings = BTreeMap<String, Box<dyn Capability>>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_kind_accepts_exactly_its_verbs() {
        assert!(CapabilityKind::Agent.accepts("spawn"));
        assert!(!CapabilityKind::Agent.accepts("ask"));
        assert!(CapabilityKind::Person.accepts("take_over"));
        assert!(!CapabilityKind::Llm.accepts("spawn"));
        assert!(CapabilityKind::Llm.accepts("write"));
    }

    #[test]
    fn a_manifest_names_what_the_adapter_is_missing() {
        let manifest = ToolManifest {
            verbs: BTreeMap::from([(
                "tests_pass".to_string(),
                ManifestVerb {
                    params: Vec::new(),
                    returns: Some("bool".to_string()),
                },
            )]),
        };
        assert!(manifest.missing(["tests_pass"]).is_empty());
        assert_eq!(
            manifest.missing(["tests_pass", "open_pr"]),
            vec!["open_pr".to_string()]
        );
    }

    #[test]
    fn tool_verbs_are_open() {
        assert!(CapabilityKind::Tool.accepts("open_pr"));
        assert!(CapabilityKind::Tool.accepts("anything_at_all"));
        assert_eq!(CapabilityKind::Tool.verbs(), None);
    }
}
