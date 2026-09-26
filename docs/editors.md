# Editor support

`jevscript lsp` is a language server: it speaks the Language Server Protocol
over stdin and stdout, and any editor with an LSP client can use it. It gives
diagnostics (with each error's stable code, spec section and a link to the
[error reference](error-reference.md)), semantic highlighting, hover,
go-to-definition, find-references, the document outline, folding and
completion. See [`crates/jevscript-lsp`](../crates/jevscript-lsp/README.md) for
what each feature does.

Build the binary and put it on your `PATH`:

```sh
cargo install --path crates/jevscript-cli     # or: cargo build --release
jevscript lsp --help
```

The server needs no configuration. Two optional `initializationOptions` are
read:

| Option | Meaning |
| --- | --- |
| `paths` | Module search roots for `use` paths that are not relative, tried before `JEVSCRIPT_PATH` (spec section 3.9). |
| `errorReference` | The URL diagnostics link each code under, as `<url>#<code>`. |

`JEVSCRIPT_PATH` in the server's environment works as it does for the CLI.

## VS Code, Cursor, Windsurf

Use the extension in [`editors/vscode`](../editors/vscode/README.md). It
registers `.jev`, supplies comment, bracket and indentation rules, and starts
the server.

## Neovim

Neovim 0.11 and later configure servers with `vim.lsp.config`. In `init.lua`:

```lua
vim.filetype.add({ extension = { jev = 'jevscript' } })

vim.lsp.config('jevscript', {
  cmd = { 'jevscript', 'lsp' },
  filetypes = { 'jevscript' },
  root_markers = { '.git' },
  -- init_options = { paths = { 'lib' } },
})
vim.lsp.enable('jevscript')

vim.api.nvim_create_autocmd('FileType', {
  pattern = 'jevscript',
  callback = function()
    vim.bo.commentstring = '# %s'
    vim.bo.shiftwidth = 2
    vim.bo.expandtab = true -- a tab in indentation is an `indent` error (spec 2.2)
  end,
})
```

Neovim applies semantic tokens itself, so highlighting works without a
grammar. The groups are `@lsp.type.<type>.jevscript` and
`@lsp.mod.<modifier>.jevscript`; the server's own modifiers are `unitKind` and
`judgmentVerb`:

```lua
vim.api.nvim_set_hl(0, '@lsp.mod.judgmentVerb.jevscript', { link = 'Special' })
vim.api.nvim_set_hl(0, '@lsp.mod.unitKind.jevscript', { link = 'Structure' })
```

This configuration was checked with Neovim 0.12 against the shipped examples:
diagnostics, hover and semantic tokens arrive.

## Helix

In `~/.config/helix/languages.toml`:

```toml
[language-server.jevscript]
command = "jevscript"
args = ["lsp"]
# config = { paths = ["lib"] }    # sent as initializationOptions

[[language]]
name = "jevscript"
scope = "source.jevscript"
file-types = ["jev"]
roots = [".git"]
comment-token = "#"
indent = { tab-width = 2, unit = "  " }
language-servers = ["jevscript"]
```

Helix gives diagnostics, hover, go-to-definition, references, the symbol
picker and completion from this. It does not apply semantic tokens and there is
no tree-sitter grammar for Jevscript yet, so `.jev` files are not
syntax-highlighted in Helix. This configuration has not been checked against a
running Helix.

## Zed

Zed adds languages through extensions, and the repository ships one in
[`editors/zed`](../editors/zed). It registers `.jev`, starts `jevscript lsp`,
and borrows tree-sitter-python (pinned to its v0.25.0 commit) as a stand-in
grammar for Zed's structural features until Jevscript has a grammar of its
own. It follows Zed's [extension](https://zed.dev/docs/extensions/developing-extensions)
and [language extension](https://zed.dev/docs/extensions/languages) guides.

1. Install Rust with rustup, which Zed needs to build a dev extension; Zed
   adds the `wasm32-wasip2` target itself.
2. Put `jevscript` on your `PATH`, or set its path in Zed's settings (below).
3. In Zed, run *zed: install dev extension* from the command palette and pick
   the `editors/zed` directory of this repository.
4. Open a `.jev` file. Diagnostics, hover, navigation, the outline and
   completion come from the server.

Highlighting comes from the server's semantic tokens, which Zed only applies
when asked to; the stand-in grammar alone does not know Jevscript's words. In
Zed's `settings.json`:

```json
{
  "semantic_tokens": "full",
  "lsp": {
    "jevscript": {
      "binary": { "path": "/path/to/jevscript", "arguments": ["lsp"] },
      "initialization_options": { "paths": ["lib"] }
    }
  }
}
```

`lsp.jevscript` is optional: without `binary.path` the extension runs
`jevscript lsp` from `PATH`, and `initialization_options` is passed to the
server as-is.

What has been verified: the manifest follows Zed's extension guides with every
dependency pinned (`zed_extension_api` 0.7.0, the version Zed 1.21's own
extensions use, and tree-sitter-python at its v0.25.0 commit), and the
extension builds with `--locked` for `wasm32-wasip2`, which CI repeats on
every push. The server it starts is the one the rest of this page uses.
Loading the extension in a running Zed has not been verified.

## Any other client

Run `jevscript lsp` with stdio transport for files ending in `.jev`. The server
negotiates UTF-8, UTF-16 or UTF-32 positions (UTF-16 by default), syncs
documents incrementally, and publishes diagnostics after each batch of edits.
