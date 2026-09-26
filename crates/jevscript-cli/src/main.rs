//! `jevscript`: the command-line tool (spec section 11.6).
//!
//! Every subcommand in spec section 11.6 is implemented, plus `lsp`, the
//! editor language server of `jevscript-lsp`, which that section's table does
//! not list yet. `run` keeps external effects behind explicit `--bind`
//! subprocesses or visibly selected `--stub` demonstration adapters; `replay`
//! needs only the self-contained recording.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod adapters;
mod diagnostics;
mod serve;
mod setup;

use std::collections::BTreeMap;
use std::io::{BufRead, Read};
use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};
use jevscript_compiler::{Compilation, Resolver, analyze, analyze_file, tool_verbs_referenced};
use jevscript_runtime::capability::ToolManifest;
use jevscript_runtime::jev::{HttpJevClient, JevAnswer};
use jevscript_runtime::profile::{DEFAULT_MODEL, Profiles};
use jevscript_runtime::record::{Event, JsonlReplayer, Recorded, Redaction, Replayer};
use jevscript_runtime::rpc::Sample;
use jevscript_runtime::{
    Pause, Resume, Run, RunError, RunOptions, RuntimeError, Value, run_judgment,
};
use jevscript_syntax::{Diagnostic, parse};
use serde_json::json;

/// Control loops for agents, judged by Jev.
#[derive(Debug, Parser)]
#[command(name = "jevscript", version, about, long_about = None)]
struct Cli {
    /// Which subcommand to run.
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Emit the IR as JSON to stdout, or the compile errors to stderr.
    Compile {
        /// The `.jev` file to compile.
        file: std::path::PathBuf,
    },
    /// Compile and report warnings and errors without emitting IR.
    Check {
        /// The `.jev` file to check.
        file: std::path::PathBuf,
        /// A tool adapter manifest to check every tool verb against, the same
        /// comparison `task.start` makes at bind time (spec section 9.4).
        #[arg(long = "tools", value_name = "manifest.json")]
        tools: Option<std::path::PathBuf>,
    },
    /// Run one judgment against a state and print the answers.
    #[command(after_help = "Reads TYPESAFE_API_KEY. Model profiles come from \
the bundle, JEVSCRIPT_PROFILES, or --profiles.")]
    Judge {
        /// The `.jev` file the judgment lives in.
        file: std::path::PathBuf,
        /// Which judgment.
        judgment: String,
        /// The state, as JSON.
        #[arg(long)]
        state: String,
        /// A Jev model id, which also selects the model profile.
        #[arg(long)]
        model: Option<String>,
        /// A profiles file to layer over the bundled ones (spec section 10.6).
        #[arg(long)]
        profiles: Option<std::path::PathBuf>,
    },
    /// Run a judgment over `{ state, expected }` rows and report accuracy.
    Eval {
        /// The `.jev` file the judgment lives in.
        file: std::path::PathBuf,
        /// Which judgment.
        judgment: String,
        /// A JSONL file of `{ state, expected }` rows.
        #[arg(long)]
        cases: std::path::PathBuf,
        /// A Jev model id, which also selects the model profile.
        #[arg(long)]
        model: Option<String>,
        /// A profiles file to layer over bundled and environment profiles.
        #[arg(long)]
        profiles: Option<std::path::PathBuf>,
    },
    /// Run `main`, answering pauses on the terminal.
    Run {
        /// The `.jev` file to run.
        file: std::path::PathBuf,
        /// The inputs, as JSON.
        #[arg(long)]
        input: Option<String>,
        /// A new path to create for the recording; existing files are refused.
        #[arg(long)]
        record: Option<std::path::PathBuf>,
        /// Store a hash and a token count instead of payloads.
        #[arg(long)]
        redact: bool,
        /// Draw labels and levels from Jev's distribution instead of taking the
        /// argmax (spec section 6.11).
        #[arg(long)]
        sample: bool,
        /// A profiles file to layer over the bundled ones (spec section 10.6).
        #[arg(long)]
        profiles: Option<std::path::PathBuf>,
        /// A module search root for `use` paths that are not relative. Repeat
        /// for more than one (spec section 3.9).
        #[arg(long = "path", value_name = "DIR")]
        paths: Vec<std::path::PathBuf>,
        /// Bind a capability to a persistent JSONL subprocess (NAME=COMMAND).
        #[arg(long = "bind", value_name = "NAME=COMMAND")]
        bindings: Vec<String>,
        /// Bind a declared capability to a visible built-in demonstration stub.
        #[arg(long = "stub", value_name = "NAME")]
        stubs: Vec<String>,
    },
    /// Replay a recording with no model calls.
    Replay {
        /// The recording to replay.
        recording: std::path::PathBuf,
    },
    /// The JSON-RPC stdio server the SDKs use.
    Serve,
    /// Install the bundled coding-agent Skill offline (spec section 11.6).
    Setup {
        /// Agent to install for; repeat for multiple agents.
        #[arg(long = "agent", required = true, value_enum)]
        agents: Vec<setup::Agent>,
        /// Install into the current user's home instead of a project.
        #[arg(long)]
        global: bool,
        /// Make independent copies instead of links at agent paths.
        #[arg(long)]
        copy: bool,
        /// Existing project directory (defaults to the current directory).
        #[arg(long, conflicts_with = "global")]
        project: Option<PathBuf>,
    },
    /// The language server for editors, speaking LSP over stdio.
    Lsp,
}

fn main() {
    let cli = Cli::parse();
    let result: anyhow::Result<CommandOutcome> = match cli.command {
        Command::Serve => serve::serve().map(|()| CommandOutcome::Success),
        Command::Setup {
            agents,
            global,
            copy,
            project,
        } => setup::install(&agents, global, copy, project.as_deref())
            .map(|()| CommandOutcome::Success),
        Command::Lsp => jevscript_lsp::run_stdio()
            .map(|()| CommandOutcome::Success)
            .map_err(|error| anyhow::anyhow!(error)),
        Command::Compile { file } => compile(&file),
        Command::Check { file, tools } => check(&file, tools.as_deref()),
        Command::Judge {
            file,
            judgment,
            state,
            model,
            profiles,
        } => judge(
            &file,
            &judgment,
            &state,
            model.as_deref(),
            profiles.as_deref(),
        )
        .map(|()| CommandOutcome::Success),
        Command::Eval {
            file,
            judgment,
            cases,
            model,
            profiles,
        } => eval(
            &file,
            &judgment,
            &cases,
            model.as_deref(),
            profiles.as_deref(),
        )
        .map(|()| CommandOutcome::Success),
        Command::Run {
            file,
            input,
            record,
            redact,
            sample,
            profiles,
            paths,
            bindings,
            stubs,
        } => run_command(RunCommand {
            file,
            input,
            record,
            redact,
            sample,
            profiles,
            paths,
            bindings,
            stubs,
        }),
        Command::Replay { recording } => replay(&recording),
    };
    let code = match result {
        Ok(CommandOutcome::Success) => return,
        Ok(CommandOutcome::Exit(code)) => code,
        Err(error) => {
            eprintln!("jevscript: {error:#}");
            1
        }
    };
    std::process::exit(code);
}

/// A requested process result returned through `main` so owned resources drop
/// before the CLI terminates (spec section 11.6).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CommandOutcome {
    Success,
    Exit(i32),
}

impl CommandOutcome {
    fn from_success(success: bool) -> Self {
        if success {
            Self::Success
        } else {
            Self::Exit(1)
        }
    }
}

/// `jevscript compile <file>`: the IR as pretty JSON on stdout, or the
/// diagnostics on stderr and exit 1 (spec section 11.6).
fn compile(file: &Path) -> anyhow::Result<CommandOutcome> {
    let compilation = analyze_cli(file, &Resolver::from_env())?;
    report(file, &compilation.diagnostics);
    match compilation.ir {
        Some(ir) => {
            println!("{}", serde_json::to_string_pretty(&ir)?);
            Ok(CommandOutcome::Success)
        }
        None => Ok(CommandOutcome::Exit(1)),
    }
}

/// `jevscript check <file> [--tools <manifest.json>]`: diagnostics only,
/// warnings included; exit 0 when there are no errors (spec section 11.6).
///
/// With `--tools`, every verb the program references on each `tool`
/// capability is compared against the manifest of section 9.4 and each absent
/// one is printed as `verb_missing: <cap>.<verb>`, with exit 1. The file is
/// either one manifest `{ "verbs": { ... } }`, applied to every tool, or a
/// record from capability name to manifest.
fn check(file: &Path, tools: Option<&Path>) -> anyhow::Result<CommandOutcome> {
    let compilation = analyze_cli(file, &Resolver::from_env())?;
    report(file, &compilation.diagnostics);
    let Some(ir) = compilation.ir else {
        return Ok(CommandOutcome::Exit(1));
    };
    let Some(manifest_path) = tools else {
        return Ok(CommandOutcome::Success);
    };
    let manifests = read_manifests(manifest_path)?;
    let mut missing = 0;
    for (capability, verbs) in tool_verbs_referenced(&ir) {
        let Some(manifest) = manifests.get(&capability).or_else(|| manifests.get("")) else {
            for verb in &verbs {
                diagnostics::unlocated(
                    file,
                    "verb_missing",
                    &format!("{capability}.{verb} (no manifest for `{capability}`)"),
                );
                missing += 1;
            }
            continue;
        };
        for verb in &verbs {
            if !manifest.verbs.contains_key(verb) {
                diagnostics::unlocated(file, "verb_missing", &format!("{capability}.{verb}"));
                missing += 1;
            }
        }
    }
    Ok(CommandOutcome::from_success(missing == 0))
}

/// Read a `--tools` file as manifests by capability name. A single manifest
/// (one with a top-level `verbs`) is stored under the empty name and applies
/// to every tool.
fn read_manifests(path: &Path) -> anyhow::Result<BTreeMap<String, ToolManifest>> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| anyhow::anyhow!("cannot read `{}`: {error}", path.display()))?;
    let value: serde_json::Value = serde_json::from_str(&text)
        .map_err(|error| anyhow::anyhow!("`{}` is not JSON: {error}", path.display()))?;
    let mut manifests = BTreeMap::new();
    if value.get("verbs").is_some_and(serde_json::Value::is_object) {
        manifests.insert(String::new(), serde_json::from_value(value)?);
        return Ok(manifests);
    }
    let Some(object) = value.as_object() else {
        anyhow::bail!(
            "`{}` must be a manifest `{{ \"verbs\": {{ ... }} }}` or a record of them by capability name",
            path.display()
        );
    };
    for (name, manifest) in object {
        manifests.insert(name.clone(), serde_json::from_value(manifest.clone())?);
    }
    Ok(manifests)
}

/// Print actionable plain-terminal diagnostics (spec sections 12 and 15.8).
fn report(file: &Path, diagnostics: &[Diagnostic]) {
    diagnostics::report(file, diagnostics);
}

/// Compile a named file or the source piped to `-` (spec section 11.6).
fn analyze_cli(file: &Path, resolver: &Resolver) -> anyhow::Result<Compilation> {
    if file != Path::new("-") {
        return Ok(analyze_file(file, resolver));
    }
    let mut source = String::new();
    std::io::stdin().read_to_string(&mut source)?;
    let _ = diagnostics::STDIN_SOURCE.set(source.clone());
    let program = match parse(&source) {
        Ok(program) => program,
        Err(diagnostics) => {
            return Ok(Compilation {
                ir: None,
                diagnostics: diagnostics
                    .into_iter()
                    .map(|d| d.in_file("<stdin>"))
                    .collect(),
            });
        }
    };
    let mut result = analyze(&program, None, resolver);
    for diagnostic in &mut result.diagnostics {
        if diagnostic.file.is_none() {
            diagnostic.file = Some("<stdin>".to_string());
        }
    }
    Ok(result)
}

fn source_name(file: &Path) -> String {
    if file == Path::new("-") {
        "<stdin>".to_string()
    } else {
        file.display().to_string()
    }
}

fn compile_ir(file: &Path, paths: &[PathBuf]) -> anyhow::Result<jevscript_ir::Ir> {
    let mut resolver = Resolver {
        roots: paths.to_vec(),
        ..Resolver::default()
    };
    resolver.roots.extend(Resolver::from_env().roots);
    let compilation = analyze_cli(file, &resolver)?;
    report(file, &compilation.diagnostics);
    compilation
        .ir
        .ok_or_else(|| anyhow::anyhow!("compile failed"))
}

fn resolved_profile(
    model: Option<&str>,
    profiles: Option<&Path>,
) -> Result<jevscript_runtime::Profile, RuntimeError> {
    let mut available = Profiles::from_env()?;
    if let Some(path) = profiles {
        available.overlay_file(path)?;
    }
    Ok(available.resolve(model.unwrap_or(DEFAULT_MODEL))?.clone())
}

fn runtime_result<T>(file: Option<&str>, result: Result<T, RuntimeError>) -> anyhow::Result<T> {
    result.map_err(|error| {
        diagnostics::runtime_error(file, &error);
        anyhow::anyhow!("runtime failed")
    })
}

/// `jevscript judge`: one standalone judgment (spec section 11.3).
fn judge(
    file: &Path,
    judgment: &str,
    state: &str,
    model: Option<&str>,
    profiles: Option<&Path>,
) -> anyhow::Result<()> {
    let ir = compile_ir(file, &[])?;
    let state = serde_json::from_str(state)
        .map_err(|error| anyhow::anyhow!("--state is not a JSON object: {error}"))?;
    let root = source_name(file);
    let profile = runtime_result(Some(&root), resolved_profile(model, profiles))?;
    let client = runtime_result(Some(&root), HttpJevClient::for_profile(&profile))?;
    let outcome = runtime_result(
        diagnostics::runtime_file(&ir, Some(&root)),
        run_judgment(&ir, judgment, &state, &client, &profile),
    )?;
    for line in outcome.logs {
        print_log(&line.into_event());
    }
    println!("{}", serde_json::to_string_pretty(&outcome.values)?);
    Ok(())
}

#[derive(serde::Deserialize)]
struct EvalCase {
    state: BTreeMap<String, serde_json::Value>,
    expected: BTreeMap<String, serde_json::Value>,
}

#[derive(Default)]
struct Accuracy {
    correct: u64,
    total: u64,
}

/// `jevscript eval`: per-question accuracy with distributions on misses.
fn eval(
    file: &Path,
    judgment: &str,
    cases: &Path,
    model: Option<&str>,
    profiles: Option<&Path>,
) -> anyhow::Result<()> {
    let ir = compile_ir(file, &[])?;
    let root = source_name(file);
    let profile = runtime_result(Some(&root), resolved_profile(model, profiles))?;
    let client = runtime_result(Some(&root), HttpJevClient::for_profile(&profile))?;
    let reader = std::io::BufReader::new(std::fs::File::open(cases)?);
    let mut accuracy: BTreeMap<String, Accuracy> = BTreeMap::new();
    let mut misses = Vec::new();
    for (index, line) in reader.lines().enumerate() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let case: EvalCase = serde_json::from_str(&line)
            .map_err(|error| anyhow::anyhow!("{}:{}: {error}", cases.display(), index + 1))?;
        let outcome = runtime_result(
            diagnostics::runtime_file(&ir, Some(&root)),
            run_judgment(&ir, judgment, &case.state, &client, &profile),
        )?;
        for (name, expected) in case.expected {
            let item = accuracy.entry(name.clone()).or_default();
            item.total += 1;
            let actual = outcome.values.get(&name);
            if actual.is_some_and(|actual| answer_matches(actual, &expected)) {
                item.correct += 1;
            } else {
                let answer = outcome
                    .answers
                    .iter()
                    .find(|answer| answer_id(answer) == name);
                misses.push(json!({
                    "case": index + 1,
                    "question": name,
                    "expected": expected,
                    "actual": actual.map(Value::to_json),
                    "answer": answer,
                }));
            }
        }
    }
    let questions: BTreeMap<_, _> = accuracy
        .into_iter()
        .map(|(name, item)| {
            let ratio = if item.total == 0 {
                0.0
            } else {
                item.correct as f64 / item.total as f64
            };
            (
                name,
                json!({ "correct": item.correct, "total": item.total, "accuracy": ratio }),
            )
        })
        .collect();
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({ "questions": questions, "misses": misses }))?
    );
    Ok(())
}

fn answer_id(answer: &JevAnswer) -> &str {
    match answer {
        JevAnswer::Noul { id, .. } | JevAnswer::Choice { id, .. } | JevAnswer::Score { id, .. } => {
            id
        }
    }
}

fn answer_matches(actual: &Value, expected: &serde_json::Value) -> bool {
    match (actual, expected) {
        (Value::Prob(probability), serde_json::Value::Bool(expected)) => {
            (*probability >= 0.5) == *expected
        }
        (Value::Choice(choice), serde_json::Value::String(expected)) => choice.label == *expected,
        (Value::Level(level), serde_json::Value::String(expected)) => level
            .names
            .get(level.level as usize)
            .is_some_and(|name| name == expected),
        (Value::Level(level), serde_json::Value::Number(expected)) => {
            expected.as_u64() == Some(u64::from(level.level))
        }
        _ => actual.to_json() == *expected,
    }
}

struct RunCommand {
    file: PathBuf,
    input: Option<String>,
    record: Option<PathBuf>,
    redact: bool,
    sample: bool,
    profiles: Option<PathBuf>,
    paths: Vec<PathBuf>,
    bindings: Vec<String>,
    stubs: Vec<String>,
}

/// `jevscript run`: explicit adapters and terminal-driven pauses (spec 11.6).
fn run_command(command: RunCommand) -> anyhow::Result<CommandOutcome> {
    let ir = compile_ir(&command.file, &command.paths)?;
    let inputs = command
        .input
        .as_deref()
        .map(serde_json::from_str)
        .transpose()
        .map_err(|error| anyhow::anyhow!("--input is not a JSON object: {error}"))?
        .unwrap_or_default();
    let bindings = adapters::build(&ir, &command.bindings, &command.stubs)?;
    if command.redact
        && let Some(path) = command.record.as_deref()
    {
        eprintln!(
            "sensitive replay companion: {}",
            jevscript_runtime::record::companion_path(path).display()
        );
    }
    let options = RunOptions {
        inputs,
        record: command.record,
        replay: None,
        redaction: if command.redact {
            Redaction::Redact
        } else {
            Redaction::Full
        },
        model: None,
        sample: command.sample.then_some(Sample::On(true)),
        profiles: command.profiles,
        paths: command.paths,
    };
    let root = source_name(&command.file);
    let runtime_file = diagnostics::runtime_file(&ir, Some(&root));
    let mut run = runtime_result(runtime_file, Run::create(ir, "main", options, bindings))?;
    run.set_event_sink(Box::new(|line| {
        print_log(&line.event);
        Ok(())
    }));
    drive_live(&mut run, &command.file)
}

/// One `log` line on stderr (spec section 5.8): level, task, source position,
/// the message and the fields as JSON. A redacted payload prints as its
/// redaction marker, exactly as the recording stores it.
fn print_log(event: &Event) {
    let Event::Log {
        level,
        message,
        fields,
        task,
        source,
    } = event
    else {
        return;
    };
    let message = match message {
        Recorded::Full(text) => text.clone(),
        redacted => serde_json::to_string(redacted).unwrap_or_default(),
    };
    let fields = if fields.is_empty() {
        String::new()
    } else {
        format!(" {}", serde_json::to_string(fields).unwrap_or_default())
    };
    eprintln!(
        "log {} {task} {}:{}: {message}{fields}",
        level.as_str(),
        source.start.line,
        source.start.column + 1
    );
}

fn drive_live(run: &mut Run, file: &Path) -> anyhow::Result<CommandOutcome> {
    let root = source_name(file);
    let runtime_file = diagnostics::runtime_file(run.ir(), Some(&root));
    loop {
        let pause = run.next().inspect_err(|error| {
            if let RunError::Runtime(runtime) = error {
                diagnostics::runtime_error(runtime_file, runtime);
            }
        })?;
        diagnostics::pause(runtime_file, &pause);
        println!("{}", serde_json::to_string(&pause)?);
        match pause {
            Pause::Done { .. } => return Ok(CommandOutcome::Success),
            Pause::Stopped { .. } | Pause::Escalate { .. } => {
                return Ok(CommandOutcome::Exit(3));
            }
            Pause::Waiting { .. } => {}
            Pause::Confirm {
                message, options, ..
            } => {
                eprintln!("{message}");
                eprintln!("options: {}", options.join(", "));
                let mut answer = String::new();
                std::io::stdin().read_line(&mut answer)?;
                let answer = answer.trim().to_string();
                run.resume(Resume::Answer { answer, text: None })?;
            }
            Pause::Budget { key, limit, .. } => {
                eprintln!(
                    "budget `{key:?}` reached {limit}; enter a new limit or leave blank to stop"
                );
                let mut answer = String::new();
                std::io::stdin().read_line(&mut answer)?;
                let answer = answer.trim();
                if answer.is_empty() {
                    return Ok(CommandOutcome::Exit(3));
                }
                let limit: f64 = answer.parse()?;
                run.resume(Resume::Extend {
                    extend: BTreeMap::from([(key, limit)]),
                })?;
            }
            Pause::Error {
                retryable: true, ..
            } => {
                eprintln!("retry? [y/N]");
                let mut answer = String::new();
                std::io::stdin().read_line(&mut answer)?;
                if matches!(answer.trim(), "y" | "yes") {
                    run.resume(Resume::Retry { retry: true })?;
                } else {
                    return Ok(CommandOutcome::Exit(1));
                }
            }
            Pause::Error { .. } => return Ok(CommandOutcome::Exit(1)),
        }
    }
}

/// `jevscript replay`: bootstrap only from the recording metadata (spec 10.3).
///
/// Replay consumes the recorded `log` events without emitting them again
/// (spec section 5.8); this command reproduces them on stderr from the
/// primary recording, so a redacted payload stays redacted. It prints a line
/// only once replay has executed and validated it, so a log that diverged,
/// or anything after it, is never shown.
fn replay(recording: &Path) -> anyhow::Result<CommandOutcome> {
    let mut run = Run::from_recording(recording).map_err(|error| {
        diagnostics::runtime_error(None, &error);
        anyhow::anyhow!("replay could not start")
    })?;
    let runtime_file =
        diagnostics::runtime_file(run.ir(), run.ir().file.as_deref()).map(str::to_string);
    let mut recorded = Vec::new();
    let mut reader = JsonlReplayer::new(recording);
    while let Some(line) = reader.next()? {
        recorded.push(line.event);
    }
    let mut printed = 0;
    let mut reproduce = |run: &Run| {
        let validated = run.replay_position().min(recorded.len());
        for (at, event) in recorded.iter().enumerate().take(validated).skip(printed) {
            if matches!(run.log().get(at), Some(Event::Log { .. })) {
                print_log(event);
            }
        }
        printed = printed.max(validated);
    };
    loop {
        let next = run.next();
        reproduce(&run);
        let pause = match next {
            Ok(pause) => pause,
            Err(RunError::WrongPayload { .. }) => {
                return Ok(match run.current_pause() {
                    Some(Pause::Escalate { .. }) => CommandOutcome::Exit(3),
                    _ => CommandOutcome::Exit(1),
                });
            }
            Err(error) => {
                if let RunError::Runtime(runtime) = &error {
                    diagnostics::runtime_error(runtime_file.as_deref(), runtime);
                }
                return Err(error.into());
            }
        };
        diagnostics::pause(runtime_file.as_deref(), &pause);
        println!("{}", serde_json::to_string(&pause)?);
        match pause {
            Pause::Done { .. } => return Ok(CommandOutcome::Success),
            Pause::Stopped { .. } => return Ok(CommandOutcome::Exit(3)),
            Pause::Error {
                retryable: false, ..
            } => return Ok(CommandOutcome::Exit(1)),
            Pause::Confirm { .. }
            | Pause::Escalate { .. }
            | Pause::Waiting { .. }
            | Pause::Budget { .. }
            | Pause::Error {
                retryable: true, ..
            } => {
                // The next pass consumes the recorded resume automatically.
            }
        }
    }
}
