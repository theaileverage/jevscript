//! The compiler's negative suite: one minimal program per whole-program code
//! in spec section 12 and per link code in section 3.9. Each test names the
//! rule and asserts the code and the offending line.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use jevscript_compiler::{Resolver, analyze_file, compile_source};
use jevscript_syntax::{Diagnostic, ErrorCode};

/// Wraps statements in `program t` with the given declarations, so line 4 is
/// the first statement of `task main`.
fn in_main(decls: &str, body: &str) -> String {
    let indented: Vec<String> = body.lines().map(|l| format!("  {l}")).collect();
    format!("program t\n{decls}\ntask main:\n{}\n", indented.join("\n"))
}

fn first_error(source: &str) -> Diagnostic {
    let diagnostics = compile_source(source).expect_err("the program is rejected");
    diagnostics
        .into_iter()
        .find(Diagnostic::is_error)
        .expect("at least one error")
}

fn assert_error(source: &str, code: ErrorCode, line: u32) {
    let diagnostic = first_error(source);
    assert_eq!(
        diagnostic.code, code,
        "wrong code for {source:?}: {diagnostic}"
    );
    assert_eq!(
        diagnostic.span.start.line, line,
        "wrong line for {source:?}: {diagnostic}"
    );
}

/* -------------------------------------------------------- section 12 codes */

#[test]
fn judgment_side_effect_for_a_capability_reached_from_a_question() {
    // Spec 3.5: a judgment cannot call capabilities. The grammar leaves only
    // an interpolation hole as a way in.
    let source = "program t\n\nneeds tree: tool\n\njudgment j(x):\n  a = x feels \"differs from {tree.diff}\"\n";
    assert_error(source, ErrorCode::JudgmentSideEffect, 6);
}

#[test]
fn verify_must_sit_directly_in_the_task_body() {
    // Spec 7.4: `verify` goes on an `until` directly in the task body; nested
    // in another statement's block, in a def or in a machine action it is a
    // `syntax` error, so that its condition is in task scope at the end.
    let nested = in_main(
        "",
        "ok = true\nif ok:\n  until verify(ok), max 1:\n    break",
    );
    assert_error(&nested, ErrorCode::Syntax, 6);
    let in_loop = in_main(
        "",
        "ok = true\nloop max 2:\n  until verify(ok), max 1:\n    break",
    );
    assert_error(&in_loop, ErrorCode::Syntax, 6);
    let in_def = "program t\n\ndef d(x):\n  until verify(x), max 1:\n    break\n  return x\n";
    assert_error(in_def, ErrorCode::Syntax, 4);
    let in_machine = "program t\n\nneeds tree: tool\n\nmachine m(x):\n  goal \"g\"\n  observe:\n    n x\n  state a:\n    on go \"go\" -> b:\n      until verify(x), max 1:\n        break\n  state b done\n";
    assert_error(in_machine, ErrorCode::Syntax, 11);
    // Directly in the body, after other statements, is the allowed place.
    let direct = in_main("", "ok = true\nuntil verify(ok), max 1:\n  break");
    compile_source(&direct).expect("a top-level verify compiles");
}

#[test]
fn def_side_effect_for_a_capability_call() {
    // Spec 8: a def may not call capabilities.
    let source = "program t\n\nneeds tree: tool\n\ndef d(x):\n  tree.create x\n  return 1\n";
    assert_error(source, ErrorCode::DefSideEffect, 6);
}

#[test]
fn def_side_effect_for_a_gate_a_pause_and_a_handle_verb() {
    // Spec 8: no gates, no pauses, and a verb on a handle acts.
    let gate = "program t\n\ndef d(x):\n  gate risk x, confidence x:\n    proceed -> return 1\n  return 2\n";
    assert_error(gate, ErrorCode::DefSideEffect, 4);
    let stop = "program t\n\ndef d(x):\n  stop \"no\"\n";
    assert_error(stop, ErrorCode::DefSideEffect, 4);
    let escalate = "program t\n\ndef d(x):\n  escalate \"no\"\n";
    assert_error(escalate, ErrorCode::DefSideEffect, 4);
    let handle = "program t\n\ndef d(dev):\n  dev.send \"hi\"\n  return 1\n";
    assert_error(handle, ErrorCode::DefSideEffect, 4);
    let task = "program t\n\ntask act(x):\n  return x\n\ndef d(x):\n  return act(x)\n";
    assert_error(task, ErrorCode::DefSideEffect, 7);
}

#[test]
fn verb_unknown_for_a_verb_the_kind_does_not_have() {
    // Spec 9.1 to 9.3: the verbs of `agent`, `person` and `llm` are fixed.
    assert_error(
        &in_main("\nneeds claude: agent\n", "claude.fly \"x\""),
        ErrorCode::VerbUnknown,
        6,
    );
    assert_error(
        &in_main("\nneeds me: person\n", "me.write \"x\""),
        ErrorCode::VerbUnknown,
        6,
    );
    assert_error(
        &in_main("\nneeds writer: llm\n", "writer.ask \"x\""),
        ErrorCode::VerbUnknown,
        6,
    );
}

#[test]
fn verb_unknown_for_a_typed_tool_and_never_for_an_open_one() {
    // Spec 9.4: a signature block closes the tool's verbs; no block leaves
    // them open.
    let typed = "program t\n\nneeds tree: tool:\n  create(branch) -> handle\n\ntask main:\n  tree.open_pr\n";
    assert_error(typed, ErrorCode::VerbUnknown, 7);
    let open = in_main("\nneeds tree: tool\n", "tree.anything 1, 2, 3");
    assert!(compile_source(&open).is_ok());
}

#[test]
fn verb_arity_for_a_typed_tool_call() {
    // Spec 9.4: the positional count must match the signature.
    let source =
        "program t\n\nneeds tree: tool:\n  create(branch) -> handle\n\ntask main:\n  tree.create\n";
    assert_error(source, ErrorCode::VerbArity, 7);
    let source = "program t\n\nneeds tree: tool:\n  create(branch) -> handle\n\ntask main:\n  tree.create 1, 2\n";
    assert_error(source, ErrorCode::VerbArity, 7);
    let source = "program t\n\nneeds tree: tool:\n  create(branch) -> handle\n\ntask main:\n  tree.create \"x\"\n";
    assert!(compile_source(source).is_ok());
}

#[test]
fn unassigned_read_before_assignment() {
    // Spec 5.3: reading a variable before assignment is a compile error.
    assert_error(&in_main("", "x = y\ny = 1"), ErrorCode::UnassignedRead, 4);
}

#[test]
fn unassigned_read_after_an_if_that_assigns_in_one_branch_only() {
    // Spec 5.3: a variable assigned in every branch is assigned after the
    // `if`; one assigned in only some branches is not.
    let some = in_main("", "c = true\nif c:\n  x = 1\ny = x");
    assert_error(&some, ErrorCode::UnassignedRead, 7);
    let all = in_main("", "c = true\nif c:\n  x = 1\nelse:\n  x = 2\ny = x");
    assert!(compile_source(&all).is_ok(), "{all}");
    let loop_body = in_main("", "loop max 2:\n  x = 1\ny = x");
    assert_error(&loop_body, ErrorCode::UnassignedRead, 6);
}

#[test]
fn loop_and_comprehension_variables_are_readable_inside_their_bodies() {
    // Spec 5.5 and 8.2.
    let source = in_main(
        "",
        "xs = [1, 2]\nfor x in xs:\n  y = x\nzs = [x + 1 for x in xs if x > 0]",
    );
    assert!(compile_source(&source).is_ok(), "{source}");
    let escaped = in_main("", "xs = [1, 2]\nzs = [x for x in xs]\ny = x");
    assert_error(&escaped, ErrorCode::UnassignedRead, 6);
}

#[test]
fn inputs_params_capabilities_units_prelude_and_builtins_are_always_readable() {
    // Spec 3.2, 3.4, 3.9, 5.7 and 7.
    let source = "program t\n\nin issue: { title }\nneeds tree: tool\n\ndef d(x):\n  return x\n\ntask helper(p):\n  return p\n\ntask main:\n  a = issue.title\n  b = tree.diff\n  c = d(1)\n  e = helper(2)\n  f = stuck(trail 3)\n  g = std.repeats([])\n  h = len([])\n";
    assert!(compile_source(source).is_ok());
}

#[test]
fn main_has_params() {
    // Spec 7: `main` reads the program's `in` declarations instead.
    assert_error(
        "program t\n\ntask main(x):\n  y = x\n",
        ErrorCode::MainHasParams,
        3,
    );
}

#[test]
fn machine_unknown_state() {
    // Spec 7.8: every `->` target must be declared.
    let source =
        "program t\n\nmachine m(x):\n  state a:\n    on go \"go\" -> nowhere\n  state b done\n";
    assert_error(source, ErrorCode::MachineUnknownState, 5);
}

#[test]
fn machine_no_done() {
    // Spec 7.8: at least one state must be `done`.
    let source = "program t\n\nmachine m(x):\n  state a:\n    on go \"go\" -> a\n";
    assert_error(source, ErrorCode::MachineNoDone, 3);
}

#[test]
fn event_no_description_for_an_empty_text() {
    // Spec 7.8: the description is what Jev reads. The parser rejects a
    // missing one; an empty one gets this far.
    let source = "program t\n\nmachine m(x):\n  state a:\n    on go \"\" -> b\n  state b done\n";
    assert_error(source, ErrorCode::EventNoDescription, 5);
}

#[test]
fn machine_gate_inside_an_action_block() {
    // Spec 7.8: action blocks may not `gate`.
    let source = "program t\n\nmachine m(x):\n  state a:\n    on go \"go\" -> b:\n      gate risk 0, confidence 1:\n        proceed -> y = 1\n  state b done\n";
    assert_error(source, ErrorCode::MachineGate, 6);
}

#[test]
fn return_anywhere_in_a_machine_action_is_syntax() {
    // Spec 7.8: machine results have a fixed shape, so `return` belongs only
    // to defs and tasks, including when it is nested in an action's block.
    let direct = "program t\n\nmachine m(x):\n  state a:\n    on go \"go\" -> b:\n      return x\n  state b done\n";
    assert_error(direct, ErrorCode::Syntax, 6);
    let nested = "program t\n\nmachine m(x):\n  state a:\n    on go \"go\" -> b:\n      if x:\n        return x\n  state b done\n";
    assert_error(nested, ErrorCode::Syntax, 7);
}

#[test]
fn pick_among_each() {
    // Spec 6.4a: `each` cannot be combined with `pick among`.
    let source = in_main("", "xs = [\"a\"]\nc = each xs pick among \"which?\"");
    assert_error(&source, ErrorCode::PickAmongEach, 5);
}

#[test]
fn gate_args_for_a_gate_missing_risk_or_confidence() {
    // Spec 7.6: `risk` and `confidence` are required.
    let source = in_main("", "c = 0.5\ngate confidence c:\n  proceed -> x = 1");
    assert_error(&source, ErrorCode::GateArgs, 5);
    let source = in_main("", "c = 0.5\ngate risk c:\n  proceed -> x = 1");
    assert_error(&source, ErrorCode::GateArgs, 5);
}

#[test]
fn duplicate_name_for_units_states_and_events() {
    // Spec 12: two units, two states of one machine, or two events of one
    // state with the same name.
    let units = "program t\n\ndef f(x):\n  return x\n\ndef f(y):\n  return y\n";
    assert_error(units, ErrorCode::DuplicateName, 6);
    let states = "program t\n\nmachine m(x):\n  state a:\n    on go \"go\" -> b\n  state a:\n    on go \"go\" -> b\n  state b done\n";
    assert_error(states, ErrorCode::DuplicateName, 6);
    let events = "program t\n\nmachine m(x):\n  state a:\n    on go \"go\" -> b\n    on go \"again\" -> b\n  state b done\n";
    assert_error(events, ErrorCode::DuplicateName, 6);
}

#[test]
fn assign_immutable_for_an_input_or_a_capability() {
    // Spec 3.2: inputs are immutable; 3.4: capability names cannot be
    // reassigned.
    let input = "program t\n\nin issue: text\n\ntask main:\n  issue = \"x\"\n";
    assert_error(input, ErrorCode::AssignImmutable, 6);
    let capability = "program t\n\nneeds tree: tool\n\ntask main:\n  tree = 1\n";
    assert_error(capability, ErrorCode::AssignImmutable, 6);
    let shaped_capability =
        "program t\n\nneeds tree: tool\n\ntask main:\n  tree = shape:\n    value 1\n";
    assert_error(shaped_capability, ErrorCode::AssignImmutable, 6);
    let shaped = "program t\n\nin issue: text\n\ntask main:\n  issue = shape:\n    a  1\n";
    assert_error(shaped, ErrorCode::AssignImmutable, 6);
}

#[test]
fn loop_control_outside_a_loop() {
    // Spec 5.5: `continue` and `break` behave as in Python, so only inside a
    // loop.
    assert_error(&in_main("", "break"), ErrorCode::LoopControlOutside, 4);
    assert_error(
        &in_main("", "x = 1\ncontinue"),
        ErrorCode::LoopControlOutside,
        5,
    );
    assert!(compile_source(&in_main("", "loop max 2:\n  break")).is_ok());
}

/* ---------------------------------------------------------- section 3.9 codes */

static NEXT_DIR: AtomicUsize = AtomicUsize::new(0);

/// A fresh directory for one test's files.
fn scratch() -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("check_errors")
        .join(NEXT_DIR.fetch_add(1, Ordering::SeqCst).to_string());
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

fn write(dir: &Path, name: &str, source: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, source).expect("write");
    path
}

fn first_file_error(path: &Path) -> Diagnostic {
    let compilation = analyze_file(path, &Resolver::default());
    assert!(
        !compilation.is_ok(),
        "{} should be rejected",
        path.display()
    );
    compilation
        .diagnostics
        .into_iter()
        .find(Diagnostic::is_error)
        .expect("at least one error")
}

fn assert_file_error(path: &Path, code: ErrorCode, line: u32) {
    let diagnostic = first_file_error(path);
    assert_eq!(diagnostic.code, code, "{diagnostic}");
    assert_eq!(diagnostic.span.start.line, line, "{diagnostic}");
}

#[test]
fn use_not_found() {
    // Spec 3.9: a path that resolves to nothing.
    let dir = scratch();
    let root = write(
        &dir,
        "root.jev",
        "program t\n\nuse \"./missing.jev\" as m\n\ntask main:\n  x = 1\n",
    );
    assert_file_error(&root, ErrorCode::UseNotFound, 3);
}

#[test]
fn use_cycle_between_two_files() {
    // Spec 3.9: modules that import each other.
    let dir = scratch();
    let root = write(
        &dir,
        "root.jev",
        "program t\n\nuse \"./a.jev\" as a\n\ntask main:\n  x = 1\n",
    );
    write(
        &dir,
        "a.jev",
        "program a\n\nuse \"./b.jev\" as b\n\ndef fa(x):\n  return x\n",
    );
    write(
        &dir,
        "b.jev",
        "program b\n\nuse \"./a.jev\" as a\n\ndef fb(x):\n  return x\n",
    );
    let diagnostic = first_file_error(&root);
    assert_eq!(diagnostic.code, ErrorCode::UseCycle, "{diagnostic}");
    // The cycle is reported at the `use` in `b.jev`, against that file (12).
    assert_eq!(diagnostic.span.start.line, 3, "{diagnostic}");
    assert!(
        diagnostic
            .file
            .as_deref()
            .is_some_and(|f| f.ends_with("b.jev")),
        "{diagnostic:?}"
    );
}

#[test]
fn use_has_inputs() {
    // Spec 3.9: a file with `in` or `out` is a program, not a library.
    let dir = scratch();
    let root = write(
        &dir,
        "root.jev",
        "program t\n\nuse \"./lib.jev\" as lib\n\ntask main:\n  x = 1\n",
    );
    write(
        &dir,
        "lib.jev",
        "program lib\n\nin thing: text\n\ndef f(x):\n  return x\n",
    );
    assert_file_error(&root, ErrorCode::UseHasInputs, 3);
}

#[test]
fn use_needs_unmapped_when_a_need_is_missing_from_with() {
    // Spec 3.9: every `needs` in the library must be mapped.
    let dir = scratch();
    let root = write(
        &dir,
        "root.jev",
        "program t\n\nuse \"./lib.jev\" as lib\n\nneeds tree: tool\n\ntask main:\n  x = 1\n",
    );
    write(
        &dir,
        "lib.jev",
        "program lib\n\nneeds tree: tool\n\ndef f(x):\n  return x\n",
    );
    assert_file_error(&root, ErrorCode::UseNeedsUnmapped, 3);
}

#[test]
fn use_needs_unmapped_when_the_kinds_differ() {
    // Spec 3.9: kinds must match across the `with` mapping.
    let dir = scratch();
    let root = write(
        &dir,
        "root.jev",
        "program t\n\nuse \"./lib.jev\" as lib with tree: claude\n\nneeds claude: agent\n\ntask main:\n  x = 1\n",
    );
    write(
        &dir,
        "lib.jev",
        "program lib\n\nneeds tree: tool\n\ndef f(x):\n  return x\n",
    );
    assert_file_error(&root, ErrorCode::UseNeedsUnmapped, 3);
}

#[test]
fn use_private_for_an_underscore_name_through_an_alias() {
    // Spec 3.9: a `_name` is private to its file.
    let dir = scratch();
    let root = write(
        &dir,
        "root.jev",
        "program t\n\nuse \"./lib.jev\" as lib\n\ntask main:\n  x = lib._hidden(1)\n",
    );
    write(
        &dir,
        "lib.jev",
        "program lib\n\ndef _hidden(x):\n  return x\n\ndef shown(x):\n  return _hidden(x)\n",
    );
    assert_file_error(&root, ErrorCode::UsePrivate, 6);
    // The library's own use of its private def is fine, and its alias works.
    let ok = write(
        &dir,
        "ok.jev",
        "program t\n\nuse \"./lib.jev\" as lib\n\ntask main:\n  x = lib.shown(1)\n",
    );
    let compilation = analyze_file(&ok, &Resolver::default());
    assert!(compilation.is_ok(), "{:#?}", compilation.diagnostics);
    let ir = compilation.ir.unwrap();
    let defs: Vec<&str> = ir.defs.iter().map(|d| d.name.as_str()).collect();
    assert!(defs.contains(&"lib._hidden") && defs.contains(&"lib.shown"));
}

#[test]
fn a_library_capability_is_renamed_to_the_importers() {
    // Spec 3.9: `with dev: claude` binds the library's `dev` to the
    // importer's `claude`, and the flat program only mentions `claude`.
    let dir = scratch();
    let root = write(
        &dir,
        "root.jev",
        "program t\n\nuse \"./lib.jev\" as lib with dev: claude\n\nneeds claude: agent\n\ntask main:\n  h = lib.start(\"go\")\n",
    );
    write(
        &dir,
        "lib.jev",
        "program lib\n\nneeds dev: agent\n\ntask start(prompt):\n  h = dev.spawn prompt prompt\n  return h\n",
    );
    let compilation = analyze_file(&root, &Resolver::default());
    let ir = compilation
        .ir
        .unwrap_or_else(|| panic!("compiles: {:#?}", compilation.diagnostics));
    let start = ir.task("lib.start").expect("linked task");
    let mut names = Vec::new();
    jevscript_compiler::walk_expr_in_stmts(&start.body, &mut |expr| {
        if let jevscript_ir::Expr::Name { name, .. } = expr {
            names.push(name.clone());
        }
    });
    assert!(names.contains(&"claude".to_string()), "{names:?}");
    assert!(!names.contains(&"dev".to_string()), "{names:?}");
    assert_eq!(ir.modules[1].mapping[0].inner, "dev");
    assert_eq!(ir.modules[1].mapping[0].outer, "claude");
}

#[test]
fn a_nested_use_qualifies_transitively() {
    // Spec 3.9: `a.b.unit`.
    let dir = scratch();
    let root = write(
        &dir,
        "root.jev",
        "program t\n\nuse \"./a.jev\" as a\n\ntask main:\n  x = a.fa(1)\n",
    );
    write(
        &dir,
        "a.jev",
        "program a\n\nuse \"./b.jev\" as b\n\ndef fa(x):\n  return b.fb(x)\n",
    );
    write(&dir, "b.jev", "program b\n\ndef fb(x):\n  return x\n");
    let ir = analyze_file(&root, &Resolver::default())
        .ir
        .expect("compiles");
    let defs: Vec<&str> = ir.defs.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(
        defs,
        vec![
            "a.fa",
            "a.b.fb",
            "std.focus_impl",
            "std.stuck",
            "std.repeats"
        ]
    );
    let aliases: Vec<&str> = ir.modules.iter().map(|m| m.alias.as_str()).collect();
    assert_eq!(aliases, vec!["", "a", "a.b", "std"]);
}

#[test]
fn a_search_root_resolves_a_bare_path() {
    // Spec 3.9: a path that does not start with `./` is looked up in the
    // host's search roots.
    let dir = scratch();
    let root = write(
        &dir,
        "root.jev",
        "program t\n\nuse \"shared/lib.jev\" as lib\n\ntask main:\n  x = lib.f(1)\n",
    );
    let roots = scratch();
    std::fs::create_dir_all(roots.join("shared")).unwrap();
    write(
        &roots.join("shared"),
        "lib.jev",
        "program lib\n\ndef f(x):\n  return x\n",
    );
    assert!(!analyze_file(&root, &Resolver::default()).is_ok());
    let resolver = Resolver {
        roots: vec![roots],
        ..Resolver::default()
    };
    assert!(analyze_file(&root, &resolver).is_ok());
}

#[test]
fn an_overlaid_source_is_linked_instead_of_the_file_on_disk() {
    // An editor's unsaved buffer: the resolver's overlay replaces the file's
    // contents, and names a module that does not exist on disk yet.
    let dir = scratch();
    let root = write(
        &dir,
        "root.jev",
        "program t\n\nuse \"./lib.jev\" as lib\nuse \"./new.jev\" as fresh\n\ntask main:\n  x = lib.f(1)\n  y = fresh.g(2)\n",
    );
    let lib = write(&dir, "lib.jev", "program lib\n\ndef f(x):\n  return x\n");
    let mut resolver = Resolver::default();
    resolver.overlay.insert(
        std::fs::canonicalize(&dir).unwrap().join("new.jev"),
        "program fresh\n\ndef g(y):\n  return y\n".to_string(),
    );
    assert!(analyze_file(&root, &resolver).is_ok());
    resolver.overlay.insert(
        std::fs::canonicalize(&lib).unwrap(),
        "program lib\n\ndef f(x):\n  return z\n".to_string(),
    );
    let compilation = analyze_file(&root, &resolver);
    let error = compilation
        .diagnostics
        .iter()
        .find(|d| d.code == ErrorCode::UnassignedRead)
        .expect("the overlaid library's error");
    assert!(
        error
            .file
            .as_deref()
            .is_some_and(|f| f.ends_with("lib.jev"))
    );
    assert_eq!(error.span.start.line, 4);
}

#[test]
fn a_parse_error_inside_a_library_names_its_file() {
    // Spec 3.9: "the callee's own diagnostics with their file names".
    let dir = scratch();
    let root = write(
        &dir,
        "root.jev",
        "program t\n\nuse \"./lib.jev\" as lib\n\ntask main:\n  x = 1\n",
    );
    write(&dir, "lib.jev", "program lib\n\ndef f(x):\n  return (\n");
    let diagnostic = first_file_error(&root);
    assert_eq!(diagnostic.code, ErrorCode::Syntax);
    assert!(
        diagnostic
            .file
            .as_deref()
            .is_some_and(|f| f.ends_with("lib.jev")),
        "{diagnostic:?}"
    );
    assert!(!diagnostic.message.contains("lib.jev"), "{diagnostic}");
}

#[test]
fn a_nested_library_link_error_names_the_importing_library() {
    // Spec 3.9 and 12: `a.jev` imports `b.jev` without mapping one of `b`'s
    // needs, so the error is `a.jev`'s, not the root's.
    let dir = scratch();
    let root = write(
        &dir,
        "root.jev",
        "program t\n\nuse \"./a.jev\" as a with tree\n\nneeds tree: tool\n\ntask main:\n  x = 1\n",
    );
    write(
        &dir,
        "a.jev",
        "program a\n\nuse \"./b.jev\" as b\n\nneeds tree: tool\n\ndef fa(x):\n  return x\n",
    );
    write(
        &dir,
        "b.jev",
        "program b\n\nneeds tree: tool\n\ndef fb(x):\n  return x\n",
    );
    let diagnostic = first_file_error(&root);
    assert_eq!(diagnostic.code, ErrorCode::UseNeedsUnmapped, "{diagnostic}");
    assert_eq!(diagnostic.span.start.line, 3, "{diagnostic}");
    assert!(
        diagnostic
            .file
            .as_deref()
            .is_some_and(|f| f.ends_with("a.jev")),
        "{diagnostic:?}"
    );
}

#[test]
fn an_unreadable_root_names_the_path_that_was_asked_for() {
    // Spec 12: every diagnostic names its file, including the one that could
    // not be read.
    let missing = scratch().join("missing.jev");
    let compilation = analyze_file(&missing, &Resolver::default());
    assert!(!compilation.is_ok());
    let diagnostic = &compilation.diagnostics[0];
    assert_eq!(diagnostic.code, ErrorCode::UseNotFound);
    assert!(
        diagnostic
            .file
            .as_deref()
            .is_some_and(|f| f.ends_with("missing.jev")),
        "{diagnostic:?}"
    );
}

#[test]
fn a_root_diagnostic_names_the_root_file() {
    // Spec 12: every diagnostic names the file it was found in.
    let dir = scratch();
    let root = write(&dir, "root.jev", "program t\n\ntask main:\n  x = y\n");
    let diagnostic = first_file_error(&root);
    assert_eq!(diagnostic.code, ErrorCode::UnassignedRead);
    assert!(
        diagnostic
            .file
            .as_deref()
            .is_some_and(|f| f.ends_with("root.jev")),
        "{diagnostic:?}"
    );
    let parse_error = write(&dir, "broken.jev", "program t\n\ntask main:\n  x = (\n");
    let diagnostic = first_file_error(&parse_error);
    assert!(
        diagnostic
            .file
            .as_deref()
            .is_some_and(|f| f.ends_with("broken.jev")),
        "{diagnostic:?}"
    );
    // A program compiled from a string has no file to name.
    let from_source = first_error("program t\n\ntask main:\n  x = y\n");
    assert_eq!(from_source.file, None);
}
