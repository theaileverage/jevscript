# Jevscript for Zed

A Zed dev extension that registers `.jev` files and starts `jevscript lsp`.
Install it with *zed: install dev extension*, pointing at this directory; see
[docs/editors.md](../../docs/editors.md#zed) for the settings and what it
provides.

It uses tree-sitter-python as a stand-in grammar until Jevscript has its own;
highlighting comes from the language server's semantic tokens
(`"semantic_tokens": "full"` in Zed's settings).

Build it the way Zed does, to check it compiles:

```sh
rustup target add wasm32-wasip2
cargo build --release --target wasm32-wasip2 --locked
```

What has been verified: the manifest follows Zed's extension guides with every
dependency pinned (`zed_extension_api` 0.7.0, tree-sitter-python at its
v0.25.0 commit), and the extension builds with `--locked` for
`wasm32-wasip2`, which CI repeats on every push. Loading it in a running Zed
has not been verified.
