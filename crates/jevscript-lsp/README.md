# `jevscript-lsp`: the language server

`jevscript lsp` speaks the Language Server Protocol over stdio. It does not
have a parser of its own: every buffer goes through the `jevscript-syntax`
lexer and parser, and every buffer that parses is linked, checked and lowered
by `jevscript-compiler` exactly as `jevscript check` does it. What this crate
adds is what an editor needs and a compiler does not. Wiring it into editors is
in [`docs/editors.md`](../../docs/editors.md).

## Features

| Request | What it gives | Module |
| --- | --- | --- |
| `publishDiagnostics` | Lexer, parser, checker and linker diagnostics, `use` imports included. Imports are linked against their open buffers, saved or not, and every open importer is republished when a library it reaches is opened, edited or closed. Each carries its stable code, `codeDescription` linking to `docs/error-reference.md#<code>`, the spec section in its message and `data.specSection`, and related locations: a duplicate's first declaration, or the place inside a library where the library's own error is (shown on the importing `use` line). Warnings are warnings. | `diagnostics` |
| `semanticTokens/full` | Keywords (`unitKind` and `judgmentVerb` modifiers), contextual words only in their positions (spec 2.5), units, prelude defs and builtins, capabilities and verbs, module aliases, parameters, variables, inputs, fields, labels, states, events, types, strings split around `{expr}` holes, numbers, comments. | `semantic` |
| `hover` | A unit's header, doc comment, and results or states; the prelude def's source; a capability's kind and verbs; keyword, builtin and verb docs summarising the spec with the section they come from. | `navigate`, `docs` |
| `definition`, `references` | Units, defs, tasks and machines, qualified names through `use` (`harness.watch`, `a.b.unit`), judgment results through the variable holding the call (`j.next`), labels (`j.next is stuck`), parameters named in a call, capabilities, `with` mappings, machine states, shape fields, variables. References to an exported unit search every open buffer and every `.jev` file in the workspace folders. | `navigate`, `world` |
| `documentSymbol` | Imports, inputs with their fields, outputs, capabilities with declared tool verbs, units; a judgment's results and labels; a machine's states and events. | `outline` |
| `foldingRange` | Every indented block, multi-line text, comment runs, `use` runs. Computed from tokens, so it works on a broken buffer. | `outline` |
| `completion` | After `x.`: a module's units, a capability's verbs, a judgment call's results, a shape's fields, a handle's verbs, the fields of a `choice`, `level`, observation or machine result. After `is`: the declared labels. After `->` on an `on` line: the machine's states. After `needs x:`: the kinds. Otherwise: names in scope, units, the prelude, builtins, keywords. | `completion` |

Text sync is incremental. Positions are negotiated as UTF-8, UTF-16 or UTF-32.
Two `initializationOptions` are read: `paths` (module search roots, tried
before `JEVSCRIPT_PATH`) and `errorReference` (the base URL of the error
reference).

## Broken buffers

The parser stops at the first error by design. `recover` re-parses with the
top-level declaration that holds the error blanked out — every character
replaced by spaces of the same byte width, so every other span stays exact —
until the rest parses. One half-typed line costs the features of its own
declaration, not the file's, and every broken declaration gets its own syntax
error. While a buffer has syntax errors, only those are published: running the
whole-program checks on a program with a declaration missing would report
errors that are not there. Completion reads the scope from the buffer with the
line being typed replaced by `_ = none`, so `j.` completes while it is still a
syntax error.

## Responsiveness

The server is one synchronous loop, like the rest of the workspace. Edits apply
as they arrive; diagnostics are computed once the message queue is empty, so a
burst of keystrokes compiles once. `tests/session.rs` holds a 5,000-line
program to a budget while it is typed into and broken; in a debug build a
keystroke-to-diagnostics round trip on it takes about 120 ms.

## Why `lsp-server`

The two maintained Rust frameworks are `tower-lsp` (and its fork
`tower-lsp-server`) and `lsp-server`. `tower-lsp` is built on `tokio` and
`async` handlers. The workspace keeps `tokio` to one place, inside the HTTP Jev
client, because the interpreter and everything above it is synchronous and
single-threaded (spec section 10.5). `lsp-server` is the protocol scaffold
rust-analyzer runs on: a blocking connection over crossbeam channels, stdio
reader and writer threads, and nothing else. It fits the rest of the codebase,
adds three small dependencies, and is maintained with rust-analyzer. Types come
from `lsp-types`, the crate both frameworks use.

## Limitations

- References to an exported unit search the open buffers and at most 2,000
  `.jev` files under the workspace folders, in path order, skipping `target`,
  `node_modules` and hidden directories. A reference in a file past that bound
  is not found; open the file to include it.
- Prelude defs have hover but no definition location: the prelude is compiled
  into the binary, not a file.

## Follow-ups

Out of scope for this crate, noted so they are not lost:

- a TextMate grammar and a tree-sitter grammar (the second would replace the
  Zed extension's tree-sitter-python stand-in, and give Helix highlighting,
  which does not apply semantic tokens);
- a formatter;
- publishing the VS Code extension to the Visual Studio Marketplace and Open
  VSX;
- rename, code actions and signature help;
- a row for `lsp` in the command table of spec section 11.6.
