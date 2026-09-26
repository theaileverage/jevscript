# Muse agent adapter

`@jevscript/adapter-muse` implements the Jevscript `agent` capability (spec section 9.1). Import `MuseAdapter` into a JavaScript host, or run `jevscript-adapter-muse` as a persistent JSONL subprocess (see `crates/jevscript-cli/README.md`). The executable defaults to Herdr; pass `--backend tmux`, `cmux`, `orca`, or `zellij` to select another terminal. `--discover --models` reports local availability and supported model listings.
