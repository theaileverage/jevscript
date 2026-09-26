# Jevscript for VS Code

Language support for Jevscript `.jev` files in VS Code and its forks (Cursor,
Windsurf, VSCodium). Everything comes from `jevscript lsp`, the language server
built into the `jevscript` binary, which runs the real lexer, parser and
compiler:

- diagnostics from the parser, checker and linker, across `use` imports, each
  with its stable code, spec section and a link to its entry in the error
  reference; warnings such as `uncapped_field` show as warnings;
- semantic highlighting: unit kinds, judgment verbs (`feels`, `pick`, `rate`),
  labels, capabilities and their verbs, strings with their `{expr}` holes,
  numbers, comments;
- hover with unit signatures and keyword, builtin and verb documentation from
  the spec;
- go to definition and find references for units, defs, capabilities, labels,
  machine states and `use`-imported names such as `harness.watch`;
- the outline, folding, and completion of keywords, names in scope, capability
  verbs, judgment results and declared labels.

The extension itself registers `.jev`, supplies the comment, bracket and
indentation rules, and starts the server.

## Install

The extension is not on a marketplace. Build the `.vsix` and install it:

```sh
cargo build --release                       # the server: target/release/jevscript
cd editors/vscode
pnpm install
pnpm run package                            # dist/jevscript.vsix
code --install-extension dist/jevscript.vsix     # or: cursor / windsurf --install-extension
```

The manifest is Open VSX compatible, so the same file installs in Cursor and
Windsurf, and could be published there unchanged.

The extension runs `jevscript lsp`. Put `jevscript` on your `PATH`, or point
`jevscript.server.path` at the binary; a relative path resolves against the
first workspace folder, so `target/debug/jevscript` works inside this
repository.

## Settings

| Setting | Default | Meaning |
| --- | --- | --- |
| `jevscript.server.path` | `jevscript` | The binary to run as `<path> lsp`. |
| `jevscript.modulePaths` | `[]` | Search roots for `use` paths that are not relative, tried before `JEVSCRIPT_PATH` (spec section 3.9). |
| `jevscript.errorReference` | the published `docs/error-reference.md` | Where each diagnostic's code links, as `<url>#<code>`. |

`Jevscript: Restart Language Server` restarts it; changing a setting does too.

## Highlighting

Tokens use the standard semantic token types, so every theme colours them.
Two modifiers carry what the spec distinguishes beyond those: `unitKind` on
`judgment`, `task`, `def` and `machine`, and `judgmentVerb` on `feels`,
`pick`, `rate`, `among` and `each`. Target them in `settings.json`:

```json
"editor.semanticTokenColorCustomizations": {
  "rules": {
    "keyword.judgmentVerb:jevscript": { "foreground": "#c678dd", "bold": true },
    "keyword.unitKind:jevscript": { "foreground": "#e5c07b" }
  }
}
```

Labels and machine states are `enumMember`, events `event`, capabilities
`interface`, module aliases `namespace`, capability verbs `method`, units,
prelude defs and builtins `function` (the last two `defaultLibrary`).

## Development

```sh
pnpm test          # manifest checks, and a session with target/debug/jevscript
pnpm run typecheck
pnpm run build     # dist/extension.js
```

Run `cargo build` first: the session test starts the real server.
