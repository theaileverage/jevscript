//! CLI adapters for `jevscript run` (spec section 11.6).

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use jevscript_runtime::capability::{Bindings, CallArgs};
use jevscript_runtime::{Capability, CapabilityKind, Handle, Observation, RuntimeError, Value};
use serde_json::{Value as Json, json};

/// Build an explicitly selected demonstration adapter.
pub fn stub(name: &str, kind: CapabilityKind) -> Box<dyn Capability> {
    eprintln!("[stub] `{name}` is using the built-in {kind:?} demonstration adapter");
    Box::new(StubAdapter {
        name: name.to_string(),
        kind,
        next_handle: 1,
    })
}

/// Start a persistent adapter subprocess using section 11.6's JSONL protocol.
pub fn subprocess(
    name: &str,
    kind: CapabilityKind,
    command: &str,
) -> anyhow::Result<Box<dyn Capability>> {
    Ok(Box::new(SubprocessAdapter::start(name, kind, command)?))
}

/// Convert the IR's capability kind to the runtime adapter kind.
pub fn runtime_kind(kind: jevscript_ir::CapabilityKind) -> CapabilityKind {
    match kind {
        jevscript_ir::CapabilityKind::Agent => CapabilityKind::Agent,
        jevscript_ir::CapabilityKind::Person => CapabilityKind::Person,
        jevscript_ir::CapabilityKind::Llm => CapabilityKind::Llm,
        jevscript_ir::CapabilityKind::Tool => CapabilityKind::Tool,
    }
}

/// Validate and construct the CLI's bindings by declared capability name.
pub fn build(
    ir: &jevscript_ir::Ir,
    commands: &[String],
    stubs: &[String],
) -> anyhow::Result<Bindings> {
    let mut selected: BTreeMap<String, Selection> = BTreeMap::new();
    for item in commands {
        let (name, command) = item
            .split_once('=')
            .ok_or_else(|| anyhow::anyhow!("--bind must be NAME=COMMAND, got `{item}`"))?;
        if name.is_empty() || command.is_empty() {
            anyhow::bail!("--bind must have a non-empty NAME and COMMAND");
        }
        if selected
            .insert(name.to_string(), Selection::Command(command.to_string()))
            .is_some()
        {
            anyhow::bail!("capability `{name}` was bound more than once");
        }
    }
    for name in stubs {
        if selected.insert(name.clone(), Selection::Stub).is_some() {
            anyhow::bail!("capability `{name}` was bound more than once");
        }
    }
    for name in selected.keys() {
        if !ir.needs.iter().any(|need| &need.name == name) {
            anyhow::bail!("`{name}` is not a declared capability");
        }
    }
    let mut bindings = Bindings::new();
    for need in &ir.needs {
        let Some(selection) = selected.remove(&need.name) else {
            continue;
        };
        let kind = runtime_kind(need.kind);
        let adapter = match selection {
            Selection::Stub => stub(&need.name, kind),
            Selection::Command(command) => subprocess(&need.name, kind, &command)?,
        };
        bindings.insert(need.name.clone(), adapter);
    }
    Ok(bindings)
}

enum Selection {
    Stub,
    Command(String),
}

struct StubAdapter {
    name: String,
    kind: CapabilityKind,
    next_handle: u64,
}

impl StubAdapter {
    fn observation(&self) -> Observation {
        Observation {
            status: jevscript_runtime::capability::AgentStatus::Waiting,
            last_message: "built-in stub observation".to_string(),
            tail: "built-in stub".to_string(),
            exit_code: None,
            fields: BTreeMap::new(),
        }
    }
}

impl Capability for StubAdapter {
    fn kind(&self) -> CapabilityKind {
        self.kind
    }

    fn call(&mut self, verb: &str, args: &CallArgs) -> Result<Value, RuntimeError> {
        eprintln!("[stub:{}] {verb} {}", self.name, args_text(args));
        match (self.kind, verb) {
            (CapabilityKind::Agent, "spawn") => {
                let id = format!("stub-{}", self.next_handle);
                self.next_handle += 1;
                Ok(Value::Handle(Handle {
                    capability: self.name.clone(),
                    id,
                    fields: BTreeMap::new(),
                }))
            }
            (CapabilityKind::Agent, "wait") => serde_json::to_value(self.observation())
                .map(|value| Value::from_json(&value))
                .map_err(adapter_json),
            (CapabilityKind::Agent, "send" | "stop") | (CapabilityKind::Person, "notify") => {
                Ok(Value::None)
            }
            (CapabilityKind::Llm, "write") => Ok(args
                .positional
                .first()
                .cloned()
                .unwrap_or(Value::Text(String::new()))),
            (CapabilityKind::Tool, _) => Ok(Value::Record(BTreeMap::new())),
            _ => Ok(Value::Record(BTreeMap::new())),
        }
    }

    fn observe(&mut self, _handle: &Handle) -> Result<Observation, RuntimeError> {
        Ok(self.observation())
    }
}

fn args_text(args: &CallArgs) -> String {
    serde_json::to_string(args).unwrap_or_else(|_| "{}".to_string())
}

struct SubprocessAdapter {
    name: String,
    kind: CapabilityKind,
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
}

impl SubprocessAdapter {
    fn start(name: &str, kind: CapabilityKind, command: &str) -> anyhow::Result<Self> {
        let mut child = Command::new("sh")
            .arg("-c")
            .arg(command)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|error| anyhow::anyhow!("cannot start adapter `{name}`: {error}"))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow::anyhow!("adapter `{name}` has no stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("adapter `{name}` has no stdout"))?;
        Ok(Self {
            name: name.to_string(),
            kind,
            child,
            stdin: Some(stdin),
            stdout: BufReader::new(stdout),
        })
    }

    fn exchange(&mut self, request: Json, field: &str) -> Result<Json, RuntimeError> {
        if self.stdin.is_none() {
            return Err(self.exited());
        }
        let name = self.name.clone();
        let stdin = self.stdin.as_mut().expect("checked above");
        writeln!(stdin, "{request}").map_err(|error| adapter_io(&name, "write", error))?;
        stdin
            .flush()
            .map_err(|error| adapter_io(&name, "flush", error))?;
        let mut line = String::new();
        if self
            .stdout
            .read_line(&mut line)
            .map_err(|error| adapter_io(&name, "read", error))?
            == 0
        {
            return Err(self.exited());
        }
        let reply: Json = serde_json::from_str(line.trim()).map_err(|error| {
            RuntimeError::new(
                jevscript_runtime::RuntimeErrorCode::AdapterError,
                format!("adapter `{}` returned malformed JSON: {error}", self.name),
            )
        })?;
        if let Some(error) = reply.get("error") {
            let message = error
                .get("message")
                .and_then(Json::as_str)
                .unwrap_or("adapter error");
            let retryable = error
                .get("retryable")
                .and_then(Json::as_bool)
                .unwrap_or(false);
            return Err(RuntimeError::new(
                jevscript_runtime::RuntimeErrorCode::AdapterError,
                message,
            )
            .retryable(retryable));
        }
        reply.get(field).cloned().ok_or_else(|| {
            RuntimeError::new(
                jevscript_runtime::RuntimeErrorCode::AdapterError,
                format!("adapter `{}` reply lacks `{field}`", self.name),
            )
        })
    }

    fn exited(&mut self) -> RuntimeError {
        let status = self.child.try_wait().ok().flatten();
        RuntimeError::new(
            jevscript_runtime::RuntimeErrorCode::AdapterError,
            format!("adapter `{}` exited unexpectedly: {status:?}", self.name),
        )
    }
}

impl Capability for SubprocessAdapter {
    fn kind(&self) -> CapabilityKind {
        self.kind
    }

    fn call(&mut self, verb: &str, args: &CallArgs) -> Result<Value, RuntimeError> {
        let request = json!({
            "operation": "call",
            "capability": self.name,
            "verb": verb,
            "args": args,
        });
        let result = self.exchange(request, "result")?;
        if self.kind == CapabilityKind::Agent && verb == "spawn" {
            return serde_json::from_value(result)
                .map(Value::Handle)
                .map_err(adapter_json);
        }
        serde_json::from_value(result).map_err(adapter_json)
    }

    fn observe(&mut self, handle: &Handle) -> Result<Observation, RuntimeError> {
        let request = json!({
            "operation": "observe",
            "capability": self.name,
            "handle": handle,
        });
        let result = self.exchange(request, "observation")?;
        serde_json::from_value(result).map_err(adapter_json)
    }
}

impl Drop for SubprocessAdapter {
    fn drop(&mut self) {
        drop(self.stdin.take());
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}

fn adapter_json(error: serde_json::Error) -> RuntimeError {
    RuntimeError::new(
        jevscript_runtime::RuntimeErrorCode::AdapterError,
        error.to_string(),
    )
}

fn adapter_io(name: &str, action: &str, error: std::io::Error) -> RuntimeError {
    RuntimeError::new(
        jevscript_runtime::RuntimeErrorCode::AdapterError,
        format!("could not {action} adapter `{name}`: {error}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subprocess_protocol_carries_bound_identity_and_retryability() {
        let script = r#"read line
printf '%s\n' '{"error":{"message":"later","retryable":true}}'"#;
        let mut adapter = SubprocessAdapter::start("tree", CapabilityKind::Tool, script).unwrap();
        let error = adapter
            .call("diff", &CallArgs::default())
            .expect_err("the fixture reports an error");
        assert_eq!(
            error.code,
            jevscript_runtime::RuntimeErrorCode::AdapterError
        );
        assert!(error.retryable);
    }
}
