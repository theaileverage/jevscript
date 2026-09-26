# Pi agent adapter

`@jevscript/adapter-pi` implements the Jevscript `agent` capability (spec section 9.1). Import `PiAdapter` into a JavaScript host, or run `jevscript-adapter-pi` as a persistent JSONL subprocess (see `crates/jevscript-cli/README.md`). The executable defaults to Herdr; pass `--backend tmux`, `cmux`, `orca`, or `zellij` to select another terminal. `--discover --models` reports local availability and supported model listings.
