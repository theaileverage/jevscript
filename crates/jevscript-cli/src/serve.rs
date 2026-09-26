//! The synchronous JSON-RPC 2.0 stdio server (spec section 11.5).
//!
//! Runs are stepped one at a time. During a step a bound capability is bridged
//! back to the host as a JSON-RPC request on the same pipe, and recording
//! events are streamed as notifications when the runtime writes them.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use jevscript_compiler::{Resolver, analyze, analyze_file};
use jevscript_runtime::capability::{Bindings, CallArgs, ToolManifest};
use jevscript_runtime::error::{RunError, RuntimeError, RuntimeErrorCode};
use jevscript_runtime::jev::HttpJevClient;
use jevscript_runtime::profile::{DEFAULT_MODEL, Profiles};
use jevscript_runtime::rpc::{
    BindingParam, CapabilityCallParams, CapabilityObserveParams, EventParams, JudgmentRunParams,
    JudgmentRunResult, Ok as RpcOk, ProgramLoadParams, ProgramLoadResult, RunInjectParams,
    RunParams, RunResumeParams, TaskStartParams, TaskStartResult, host_method, method,
};
use jevscript_runtime::{
    AbortHandle, Capability, CapabilityKind, Handle, Observation, Run, RunOptions, Value,
    run_judgment,
};
use jevscript_syntax::{Diagnostic, parse};
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};

const PARSE_ERROR: i32 = -32700;
const INVALID_REQUEST: i32 = -32600;
const METHOD_NOT_FOUND: i32 = -32601;
const INVALID_PARAMS: i32 = -32602;
const RUNTIME_ERROR: i32 = -32010;
const RUN_PROTOCOL_ERROR: i32 = -32011;
const COMPILE_ERROR: i32 = -32020;

#[derive(Debug, Deserialize)]
struct Request {
    #[serde(default)]
    jsonrpc: String,
    #[serde(default)]
    id: RequestId,
    #[serde(default)]
    method: String,
    #[serde(default)]
    params: Json,
}

#[derive(Debug, Default)]
enum RequestId {
    #[default]
    Missing,
    Present(Json),
}

impl<'de> Deserialize<'de> for RequestId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Json::deserialize(deserializer).map(Self::Present)
    }
}

impl RequestId {
    fn validate(self) -> Result<Option<Json>, ()> {
        match self {
            Self::Missing => Ok(None),
            Self::Present(value) if valid_request_id(&value) => Ok(Some(value)),
            Self::Present(_) => Err(()),
        }
    }
}

#[derive(Debug, Serialize)]
struct RpcError {
    code: i32,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Json>,
}

struct Transport {
    reader: BufReader<std::io::Stdin>,
    writer: std::io::Stdout,
    deferred: VecDeque<String>,
}

impl Transport {
    fn new() -> Self {
        Self {
            reader: BufReader::new(std::io::stdin()),
            writer: std::io::stdout(),
            deferred: VecDeque::new(),
        }
    }

    fn read_stdin(&mut self) -> anyhow::Result<Option<String>> {
        let mut line = String::new();
        loop {
            line.clear();
            if self.reader.read_line(&mut line)? == 0 {
                return Ok(None);
            }
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                return Ok(Some(trimmed.to_string()));
            }
        }
    }

    fn next_request(&mut self) -> anyhow::Result<Option<String>> {
        match self.deferred.pop_front() {
            Some(line) => Ok(Some(line)),
            None => self.read_stdin(),
        }
    }

    fn defer_request(&mut self, line: String) {
        self.deferred.push_back(line);
    }

    fn write(&mut self, value: &Json) -> anyhow::Result<()> {
        writeln!(self.writer, "{value}")?;
        self.writer.flush()?;
        Ok(())
    }
}

struct Server {
    transport: Arc<Mutex<Transport>>,
    programs: BTreeMap<String, jevscript_ir::Ir>,
    runs: BTreeMap<String, Run>,
    next_program: u64,
}

impl Server {
    fn new(transport: Arc<Mutex<Transport>>) -> Self {
        Self {
            transport,
            programs: BTreeMap::new(),
            runs: BTreeMap::new(),
            next_program: 1,
        }
    }

    fn dispatch(&mut self, method_name: &str, params: Json) -> Result<Json, DispatchError> {
        match method_name {
            method::PROGRAM_LOAD => self.program_load(parse_params(params)?),
            method::JUDGMENT_RUN => self.judgment_run(parse_params(params)?),
            method::TASK_START => self.task_start(parse_params(params)?),
            method::RUN_NEXT => self.run_next(parse_params(params)?),
            method::RUN_RESUME => self.run_resume(parse_params(params)?),
            method::RUN_ABORT => self.run_abort(parse_params(params)?),
            method::RUN_INJECT => self.run_inject(parse_params(params)?),
            _ => Err(DispatchError::rpc(
                METHOD_NOT_FOUND,
                "method not found",
                json!({ "kind": "method_not_found", "method": method_name, "known": method::ALL }),
            )),
        }
    }

    fn program_load(&mut self, params: ProgramLoadParams) -> Result<Json, DispatchError> {
        if params.source.is_some() == params.path.is_some() {
            return Err(DispatchError::invalid_params(
                "program.load requires exactly one of `source` or `path`",
            ));
        }
        let mut resolver = Resolver {
            roots: params.paths.iter().map(PathBuf::from).collect(),
            ..Resolver::default()
        };
        resolver.roots.extend(Resolver::from_env().roots);
        let compilation = match params.source {
            Some(source) => {
                let program = parse(&source).map_err(DispatchError::compile)?;
                analyze(&program, params.path.as_deref().map(Path::new), &resolver)
            }
            None => analyze_file(
                Path::new(params.path.as_deref().expect("checked above")),
                &resolver,
            ),
        };
        let ir = compilation
            .ir
            .ok_or_else(|| DispatchError::compile(compilation.diagnostics))?;
        let program_id = format!("program_{}", self.next_program);
        self.next_program += 1;
        let result = ProgramLoadResult {
            program_id: program_id.clone(),
            name: ir.program.clone(),
            inputs: ir.inputs.iter().map(Into::into).collect(),
            needs: ir.needs.iter().map(Into::into).collect(),
            judgments: ir.judgments.iter().map(Into::into).collect(),
        };
        self.programs.insert(program_id, ir);
        serde_json::to_value(result).map_err(DispatchError::serialization)
    }

    fn judgment_run(&mut self, params: JudgmentRunParams) -> Result<Json, DispatchError> {
        let ir = self.program(&params.program_id)?.clone();
        let mut profiles = Profiles::from_env().map_err(DispatchError::runtime)?;
        if let Some(path) = params.profiles.as_deref() {
            profiles
                .overlay_file(Path::new(path))
                .map_err(DispatchError::runtime)?;
        }
        let model = params.model.as_deref().unwrap_or(DEFAULT_MODEL);
        let profile = profiles
            .resolve(model)
            .map_err(DispatchError::runtime)?
            .clone();
        let client = HttpJevClient::for_profile(&profile).map_err(DispatchError::runtime)?;
        let outcome = run_judgment(&ir, &params.name, &params.state, &client, &profile)
            .map_err(DispatchError::runtime)?;
        serde_json::to_value(JudgmentRunResult {
            answers: outcome.values,
            logs: outcome.logs,
        })
        .map_err(DispatchError::serialization)
    }

    fn task_start(&mut self, params: TaskStartParams) -> Result<Json, DispatchError> {
        if !params.paths.is_empty() {
            return Err(DispatchError::invalid_params(
                "`paths` belongs to program.load; task.start uses the already linked program",
            ));
        }
        let ir = self.program(&params.program_id)?.clone();
        let mut seen = BTreeSet::new();
        let mut bindings: Bindings = BTreeMap::new();
        let mut bridge_run_ids = Vec::new();
        let mut bridge_abort_handles = Vec::new();
        for binding in params.bindings {
            if !seen.insert(binding.name.clone()) {
                return Err(DispatchError::invalid_params(format!(
                    "capability `{}` was bound more than once",
                    binding.name
                )));
            }
            let need = ir
                .needs
                .iter()
                .find(|need| need.name == binding.name)
                .ok_or_else(|| {
                    DispatchError::invalid_params(format!(
                        "`{}` is not a declared capability",
                        binding.name
                    ))
                })?;
            let expected = ir_kind_name(need.kind);
            if binding.kind != expected {
                return Err(DispatchError::invalid_params(format!(
                    "capability `{}` is `{expected}`, not `{}`",
                    binding.name, binding.kind
                )));
            }
            let handle_verbs = need
                .signatures
                .iter()
                .filter(|signature| signature.returns == jevscript_ir::ReturnType::Handle)
                .map(|signature| signature.name.clone())
                .collect();
            let bridge = HostCapability::new(binding, Arc::clone(&self.transport), handle_verbs)?;
            bridge_run_ids.push(Arc::clone(&bridge.run_id));
            bridge_abort_handles.push(Arc::clone(&bridge.abort_handle));
            bindings.insert(bridge.name.clone(), Box::new(bridge));
        }
        let options = RunOptions {
            inputs: params.inputs,
            record: params.record.map(PathBuf::from),
            replay: params.replay.map(PathBuf::from),
            redaction: if params.redact.unwrap_or(false) {
                jevscript_runtime::record::Redaction::Redact
            } else {
                jevscript_runtime::record::Redaction::Full
            },
            model: params.model,
            sample: params.sample,
            profiles: params.profiles.map(PathBuf::from),
            paths: Vec::new(),
        };
        let mut run =
            Run::create(ir, &params.name, options, bindings).map_err(DispatchError::runtime)?;
        let run_id = run.id().to_string();
        let abort_handle = run.abort_handle();
        for bridge_run_id in bridge_run_ids {
            *bridge_run_id.lock().expect("bridge run id lock") = run_id.clone();
        }
        for slot in bridge_abort_handles {
            *slot.lock().expect("bridge abort handle lock") = Some(abort_handle.clone());
        }
        let transport = Arc::clone(&self.transport);
        let event_run_id = run_id.clone();
        run.set_event_sink(Box::new(move |event| {
            let notification = json!({
                "jsonrpc": "2.0",
                "method": host_method::EVENT,
                "params": EventParams { run_id: event_run_id.clone(), event: event.clone() }
            });
            transport
                .lock()
                .map_err(|_| {
                    RuntimeError::new(RuntimeErrorCode::AdapterError, "stdio lock poisoned")
                })?
                .write(&notification)
                .map_err(|error| {
                    RuntimeError::new(RuntimeErrorCode::AdapterError, error.to_string())
                })
        }));
        self.runs.insert(run_id.clone(), run);
        serde_json::to_value(TaskStartResult { run_id }).map_err(DispatchError::serialization)
    }

    fn run_next(&mut self, params: RunParams) -> Result<Json, DispatchError> {
        let run = self.run(&params.run_id)?;
        let pause = run.next().map_err(DispatchError::run)?;
        let _ = run.drain_events();
        serde_json::to_value(pause).map_err(DispatchError::serialization)
    }

    fn run_resume(&mut self, params: RunResumeParams) -> Result<Json, DispatchError> {
        self.run(&params.run_id)?
            .resume(params.payload)
            .map_err(DispatchError::run)?;
        serde_json::to_value(RpcOk::default()).map_err(DispatchError::serialization)
    }

    fn run_abort(&mut self, params: RunParams) -> Result<Json, DispatchError> {
        let run = self.run(&params.run_id)?;
        run.abort().map_err(DispatchError::run)?;
        let _ = run.drain_events();
        serde_json::to_value(RpcOk::default()).map_err(DispatchError::serialization)
    }

    fn run_inject(&mut self, params: RunInjectParams) -> Result<Json, DispatchError> {
        let run = self.run(&params.run_id)?;
        run.inject(&params.capability, &params.message)
            .map_err(DispatchError::run)?;
        let _ = run.drain_events();
        serde_json::to_value(RpcOk::default()).map_err(DispatchError::serialization)
    }

    fn program(&self, id: &str) -> Result<&jevscript_ir::Ir, DispatchError> {
        self.programs
            .get(id)
            .ok_or_else(|| DispatchError::invalid_params(format!("unknown program_id `{id}`")))
    }

    fn run(&mut self, id: &str) -> Result<&mut Run, DispatchError> {
        self.runs
            .get_mut(id)
            .ok_or_else(|| DispatchError::invalid_params(format!("unknown run_id `{id}`")))
    }
}

struct HostCapability {
    name: String,
    kind: CapabilityKind,
    manifest: Option<ToolManifest>,
    transport: Arc<Mutex<Transport>>,
    run_id: Arc<Mutex<String>>,
    handle_verbs: BTreeSet<String>,
    abort_handle: Arc<Mutex<Option<AbortHandle>>>,
}

impl HostCapability {
    fn new(
        binding: BindingParam,
        transport: Arc<Mutex<Transport>>,
        handle_verbs: BTreeSet<String>,
    ) -> Result<Self, DispatchError> {
        let kind = match binding.kind.as_str() {
            "agent" => CapabilityKind::Agent,
            "person" => CapabilityKind::Person,
            "llm" => CapabilityKind::Llm,
            "tool" => CapabilityKind::Tool,
            other => {
                return Err(DispatchError::invalid_params(format!(
                    "unknown capability kind `{other}`"
                )));
            }
        };
        Ok(Self {
            name: binding.name,
            kind,
            manifest: binding.manifest,
            transport,
            run_id: Arc::new(Mutex::new(String::new())),
            handle_verbs,
            abort_handle: Arc::new(Mutex::new(None)),
        })
    }

    fn request(&mut self, method_name: &str, params: Json) -> Result<Json, RuntimeError> {
        static NEXT_HOST_ID: AtomicU64 = AtomicU64::new(1);
        let id = NEXT_HOST_ID.fetch_add(1, Ordering::Relaxed);
        let request =
            json!({ "jsonrpc": "2.0", "id": id, "method": method_name, "params": params });
        let mut transport = self.transport.lock().map_err(|_| {
            RuntimeError::new(RuntimeErrorCode::AdapterError, "stdio lock poisoned")
        })?;
        transport.write(&request).map_err(adapter_io)?;
        loop {
            let line = transport.read_stdin().map_err(adapter_io)?.ok_or_else(|| {
                RuntimeError::new(
                    RuntimeErrorCode::AdapterError,
                    "the host closed stdin during a capability call",
                )
            })?;
            let reply: Json = serde_json::from_str(&line).map_err(|error| {
                RuntimeError::new(
                    RuntimeErrorCode::AdapterError,
                    format!("the host returned malformed JSON: {error}"),
                )
            })?;
            // Method-bearing objects are host requests or notifications, even
            // when their numeric id happens to equal this host request's id.
            // Preserve them for the outer dispatcher after this synchronous
            // capability round-trip completes (spec sections 10.5 and 11.5).
            if reply.get("method").and_then(Json::as_str).is_some() {
                if self.handle_abort_request(&reply, &mut transport)? {
                    continue;
                }
                transport.defer_request(line);
                continue;
            }
            if reply.get("id") != Some(&json!(id)) {
                continue;
            }
            if let Some(error) = reply.get("error") {
                return Err(adapter_error(error));
            }
            let result = reply.get("result").cloned().ok_or_else(|| {
                RuntimeError::new(
                    RuntimeErrorCode::AdapterError,
                    "the host reply had neither result nor error",
                )
            })?;
            if let Some(error) = result.get("error") {
                return Err(adapter_error(error));
            }
            return Ok(result);
        }
    }

    fn handle_abort_request(
        &self,
        request: &Json,
        transport: &mut Transport,
    ) -> Result<bool, RuntimeError> {
        if request.get("jsonrpc").and_then(Json::as_str) != Some("2.0")
            || request.get("method").and_then(Json::as_str) != Some(method::RUN_ABORT)
        {
            return Ok(false);
        }
        let requested_run = request
            .get("params")
            .and_then(|params| params.get("run_id"))
            .and_then(Json::as_str);
        let current_run = self.run_id.lock().map_err(|_| {
            RuntimeError::new(RuntimeErrorCode::AdapterError, "run id lock poisoned")
        })?;
        if requested_run != Some(current_run.as_str()) {
            return Ok(false);
        }
        let response_id = match request.get("id") {
            None => None,
            Some(id) if valid_request_id(id) => Some(id.clone()),
            Some(_) => {
                let error = RpcError {
                    code: INVALID_REQUEST,
                    message: "invalid request".to_string(),
                    data: Some(json!({
                        "kind": "invalid_request",
                        "detail": "id must be a string, number or null",
                    })),
                };
                transport
                    .write(&json!({
                        "jsonrpc": "2.0",
                        "id": Json::Null,
                        "error": error,
                    }))
                    .map_err(adapter_io)?;
                return Ok(true);
            }
        };
        let abort = self
            .abort_handle
            .lock()
            .map_err(|_| {
                RuntimeError::new(RuntimeErrorCode::AdapterError, "abort handle lock poisoned")
            })?
            .clone()
            .ok_or_else(|| {
                RuntimeError::new(
                    RuntimeErrorCode::AdapterError,
                    "abort handle was not installed before the capability call",
                )
            })?;
        abort.abort();
        if let Some(id) = response_id {
            transport
                .write(&json!({ "jsonrpc": "2.0", "id": id, "result": RpcOk::default() }))
                .map_err(adapter_io)?;
        }
        Ok(true)
    }
}

impl Capability for HostCapability {
    fn kind(&self) -> CapabilityKind {
        self.kind
    }

    fn call(&mut self, verb: &str, args: &CallArgs) -> Result<Value, RuntimeError> {
        let params = serde_json::to_value(CapabilityCallParams {
            run_id: self.run_id.lock().expect("bridge run id lock").clone(),
            capability: self.name.clone(),
            verb: verb.to_string(),
            args: args.clone(),
        })
        .map_err(|error| RuntimeError::new(RuntimeErrorCode::AdapterError, error.to_string()))?;
        let result = self.request(host_method::CAPABILITY_CALL, params)?;
        let result = result.get("result").cloned().unwrap_or(Json::Null);
        if self.kind == CapabilityKind::Agent && verb == "spawn" {
            return serde_json::from_value(result)
                .map(Value::Handle)
                .map_err(|error| {
                    RuntimeError::new(RuntimeErrorCode::AdapterError, error.to_string())
                });
        }
        if self.kind == CapabilityKind::Tool && self.handle_verbs.contains(verb) {
            let Json::Object(fields) = result else {
                return Err(RuntimeError::new(
                    RuntimeErrorCode::AdapterError,
                    format!("adapter `{}` returned a non-object handle", self.name),
                ));
            };
            let id = fields.get("id").and_then(Json::as_str).ok_or_else(|| {
                RuntimeError::new(
                    RuntimeErrorCode::AdapterError,
                    format!(
                        "adapter `{}` returned a handle without a string id",
                        self.name
                    ),
                )
            })?;
            let extra = fields
                .iter()
                .filter(|(name, _)| !matches!(name.as_str(), "$jev" | "capability" | "id"))
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect();
            return Ok(Value::Handle(Handle {
                capability: self.name.clone(),
                id: id.to_string(),
                fields: extra,
            }));
        }
        serde_json::from_value(result)
            .map_err(|error| RuntimeError::new(RuntimeErrorCode::AdapterError, error.to_string()))
    }

    fn manifest(&self) -> Option<ToolManifest> {
        self.manifest.clone()
    }

    fn observe(&mut self, handle: &Handle) -> Result<Observation, RuntimeError> {
        let params = serde_json::to_value(CapabilityObserveParams {
            run_id: self.run_id.lock().expect("bridge run id lock").clone(),
            capability: self.name.clone(),
            handle: handle.clone(),
        })
        .map_err(|error| RuntimeError::new(RuntimeErrorCode::AdapterError, error.to_string()))?;
        let result = self.request(host_method::CAPABILITY_OBSERVE, params)?;
        serde_json::from_value(result.get("observation").cloned().unwrap_or(Json::Null))
            .map_err(|error| RuntimeError::new(RuntimeErrorCode::AdapterError, error.to_string()))
    }
}

fn adapter_io(error: anyhow::Error) -> RuntimeError {
    RuntimeError::new(RuntimeErrorCode::AdapterError, error.to_string())
}

fn adapter_error(value: &Json) -> RuntimeError {
    let message = value
        .get("message")
        .and_then(Json::as_str)
        .unwrap_or("adapter error");
    let retryable = value
        .get("retryable")
        .and_then(Json::as_bool)
        .unwrap_or(false);
    RuntimeError::new(RuntimeErrorCode::AdapterError, message).retryable(retryable)
}

fn ir_kind_name(kind: jevscript_ir::CapabilityKind) -> &'static str {
    match kind {
        jevscript_ir::CapabilityKind::Agent => "agent",
        jevscript_ir::CapabilityKind::Person => "person",
        jevscript_ir::CapabilityKind::Llm => "llm",
        jevscript_ir::CapabilityKind::Tool => "tool",
    }
}

#[derive(Debug)]
struct DispatchError {
    code: i32,
    message: String,
    data: Json,
}

impl DispatchError {
    fn rpc(code: i32, message: impl Into<String>, data: Json) -> Self {
        Self {
            code,
            message: message.into(),
            data,
        }
    }

    fn invalid_params(message: impl Into<String>) -> Self {
        Self::rpc(INVALID_PARAMS, message, json!({ "kind": "invalid_params" }))
    }

    fn compile(diagnostics: Vec<Diagnostic>) -> Self {
        let message = diagnostics
            .first()
            .map_or("program did not compile", |diagnostic| {
                diagnostic.message.as_str()
            });
        Self::rpc(
            COMPILE_ERROR,
            message,
            json!({ "kind": "compile_error", "diagnostics": diagnostics }),
        )
    }

    fn runtime(error: RuntimeError) -> Self {
        Self::rpc(
            RUNTIME_ERROR,
            &error.message,
            json!({
                "kind": error.code.as_str(),
                "retryable": error.retryable,
                "source": error.span,
            }),
        )
    }

    fn run(error: RunError) -> Self {
        match error {
            RunError::Runtime(error) => Self::runtime(error),
            other => Self::rpc(
                RUN_PROTOCOL_ERROR,
                other.to_string(),
                json!({ "kind": "run_protocol" }),
            ),
        }
    }

    fn serialization(error: serde_json::Error) -> Self {
        Self::rpc(
            RUNTIME_ERROR,
            error.to_string(),
            json!({ "kind": "adapter_error" }),
        )
    }
}

fn parse_params<T: serde::de::DeserializeOwned>(params: Json) -> Result<T, DispatchError> {
    serde_json::from_value(params)
        .map_err(|error| DispatchError::invalid_params(format!("invalid parameters: {error}")))
}

/// Read requests from stdin and answer them on stdout until stdin closes.
pub fn serve() -> anyhow::Result<()> {
    let transport = Arc::new(Mutex::new(Transport::new()));
    let mut server = Server::new(Arc::clone(&transport));
    loop {
        let line = match transport.lock().expect("stdio lock").next_request()? {
            Some(line) => line,
            None => return Ok(()),
        };
        let value: Json = match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(error) => {
                send_error(
                    &transport,
                    Json::Null,
                    DispatchError::rpc(
                        PARSE_ERROR,
                        "parse error",
                        json!({ "kind": "parse_error", "detail": error.to_string() }),
                    ),
                )?;
                continue;
            }
        };
        let request: Request = match serde_json::from_value(value.clone()) {
            Ok(request) => request,
            Err(error) => {
                send_error(
                    &transport,
                    response_id(&value),
                    DispatchError::rpc(
                        INVALID_REQUEST,
                        "invalid request",
                        json!({ "kind": "invalid_request", "detail": error.to_string() }),
                    ),
                )?;
                continue;
            }
        };
        let id = match request.id.validate() {
            Ok(id) => id,
            Err(()) => {
                send_error(
                    &transport,
                    Json::Null,
                    DispatchError::rpc(
                        INVALID_REQUEST,
                        "invalid request",
                        json!({ "kind": "invalid_request", "detail": "id must be a string, number or null" }),
                    ),
                )?;
                continue;
            }
        };
        if request.jsonrpc != "2.0" || request.method.is_empty() {
            send_error(
                &transport,
                id.unwrap_or(Json::Null),
                DispatchError::rpc(
                    INVALID_REQUEST,
                    "invalid request",
                    json!({ "kind": "invalid_request" }),
                ),
            )?;
            continue;
        }
        let result = server.dispatch(&request.method, request.params);
        if let Some(id) = id {
            match result {
                Ok(result) => transport
                    .lock()
                    .expect("stdio lock")
                    .write(&json!({ "jsonrpc": "2.0", "id": id, "result": result }))?,
                Err(error) => send_error(&transport, id, error)?,
            }
        }
    }
}

fn response_id(value: &Json) -> Json {
    value
        .get("id")
        .filter(|id| valid_request_id(id))
        .cloned()
        .unwrap_or(Json::Null)
}

fn valid_request_id(id: &Json) -> bool {
    id.is_null() || id.is_string() || id.is_number()
}

fn send_error(
    transport: &Arc<Mutex<Transport>>,
    id: Json,
    error: DispatchError,
) -> anyhow::Result<()> {
    let rpc_error = RpcError {
        code: error.code,
        message: error.message,
        data: Some(error.data),
    };
    transport
        .lock()
        .expect("stdio lock")
        .write(&json!({ "jsonrpc": "2.0", "id": id, "error": rpc_error }))
}
