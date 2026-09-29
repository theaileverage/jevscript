# Check a change before you push it

[AGENTS.md](../AGENTS.md#checks) lists every check a change must pass, and the
`rust` and `sdks` jobs in [ci.yml](../.github/workflows/ci.yml) run them on each
push and pull request. The cells below run the same commands in the same order.
If this page and AGENTS.md disagree, AGENTS.md is right.

## Check your tools

CI uses the Rust toolchain pinned in `rust-toolchain.toml`, Node 22, pnpm 10,
and Python 3.12. The adapter suite's integration test needs `tmux`, and the
packaging steps need `uv`. This cell reports what this machine has. It does not
install anything.

```bash {"cwd":"..","interpreter":"bash","name":"dev-doctor","tag":"inspection"}
echo "rust pin:  $(sed -n 's/^channel = "\(.*\)"$/\1/p' rust-toolchain.toml)"
echo "installed: $(rustup toolchain list | tr '\n' ' ')"
for tool in node pnpm python3 uv; do
  if command -v "$tool" >/dev/null; then echo "$tool: $("$tool" --version)"; else echo "$tool: missing"; fi
done
if command -v tmux >/dev/null; then tmux -V; else echo "tmux: missing"; fi
```

## Format the Rust code

`dev-fmt-check` reports formatting differences. `dev-fmt-apply` rewrites the
source files.

```bash {"cwd":"..","interpreter":"bash","name":"dev-fmt-check","tag":"inspection"}
cargo fmt --check
```

```bash {"cwd":"..","excludeFromRunAll":"true","interpreter":"bash","name":"dev-fmt-apply","tag":"local-mutation"}
cargo fmt
```

## Lint, test, and build the workspace

These cells write to `target/`. The SDK suites start `target/debug/jevscript`,
so run `dev-build` before them.

```bash {"cwd":"..","excludeFromRunAll":"true","interpreter":"bash","name":"dev-clippy","tag":"local-mutation"}
cargo clippy --all-targets -- -D warnings
```

```bash {"cwd":"..","excludeFromRunAll":"true","interpreter":"bash","name":"dev-test","tag":"local-mutation"}
cargo test --workspace
```

```bash {"cwd":"..","excludeFromRunAll":"true","interpreter":"bash","name":"dev-build","tag":"local-mutation"}
cargo build
```

After `dev-build`, check every example program with the debug CLI. The cell
reads the programs and writes nothing.

```bash {"cwd":"..","excludeFromRunAll":"true","interpreter":"bash","name":"dev-check-examples","tag":"inspection"}
for program in examples/*.jev examples/lib/*.jev examples/custom-decision-model/urgent.jev; do
  echo "check $program"
  target/debug/jevscript check "$program"
done
```

## Regenerate the IR schema

`crates/jevscript-ir/tests/schema.rs` fails when the IR types and the committed
`crates/jevscript-ir/ir.schema.json` disagree. `dev-schema-check` runs that test
alone. `dev-schema-write` rewrites the committed file. The implementation lead
copies it into `spec/`. Do not edit `spec/` yourself.

```bash {"cwd":"..","excludeFromRunAll":"true","interpreter":"bash","name":"dev-schema-check","tag":"local-mutation"}
cargo test -p jevscript-ir --test schema
```

```bash {"cwd":"..","excludeFromRunAll":"true","interpreter":"bash","name":"dev-schema-write","tag":"local-mutation"}
cargo run -p jevscript-ir --example schema > crates/jevscript-ir/ir.schema.json
```

## Test the SDKs, adapters, and editors

Each cell installs packages from the lockfile and writes `node_modules/`,
`dist/`, or a package archive. Run `dev-sdk-js` before `dev-adapters`, because
the adapters take their types from the SDK's `dist/`. The Python suite needs
`pytest` in the active Python environment.

```bash {"cwd":"..","excludeFromRunAll":"true","interpreter":"bash","name":"dev-sdk-js","tag":"local-mutation"}
cd sdk/js
pnpm install --frozen-lockfile
pnpm test
pnpm run typecheck
pnpm run build
```

```bash {"cwd":"..","excludeFromRunAll":"true","interpreter":"bash","name":"dev-sdk-python","tag":"local-mutation"}
python3 -m pytest sdk/python
```

```bash {"cwd":"..","excludeFromRunAll":"true","interpreter":"bash","name":"dev-adapters","tag":"local-mutation"}
cd adapters
pnpm install --frozen-lockfile
pnpm test
pnpm run typecheck
pnpm run build
```

```bash {"cwd":"..","excludeFromRunAll":"true","interpreter":"bash","name":"dev-vscode","tag":"local-mutation"}
cd editors/vscode
pnpm install --frozen-lockfile
pnpm test
pnpm run typecheck
pnpm run package
```

`dev-zed` adds the `wasm32-wasip2` target to your rustup toolchain, then builds
the Zed extension the way Zed builds it.

```bash {"cwd":"..","excludeFromRunAll":"true","interpreter":"bash","name":"dev-zed","tag":"local-mutation"}
rustup target add wasm32-wasip2
cargo build --release --target wasm32-wasip2 --locked --manifest-path editors/zed/Cargo.toml
```
