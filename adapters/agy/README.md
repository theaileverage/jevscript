# Antigravity agent adapter

`@jevscript/adapter-agy` implements the Jevscript `agent` capability (spec section 9.1). Import `AntigravityAdapter` into a JavaScript host, or run `jevscript-adapter-agy` as a persistent JSONL subprocess (see `crates/jevscript-cli/README.md`). The executable defaults to Herdr; pass `--backend tmux`, `cmux`, `orca`, or `zellij` to select another terminal. `--discover --models` reports local availability and supported model listings.
