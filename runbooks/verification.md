# Run one verification suite on its own

`dev-test` in [Development](development.md) runs every Rust suite at once. Use
these cells when you work on one area and want its suite alone, or when you
change the release scripts, whose suite runs only in CI's `package-smoke` job.
Each cell compiles or runs tests, so each writes to `target/` or to Python
caches.

## Check the spec section 15 acceptance suites

[Conformance evidence](../docs/conformance.md) maps each acceptance item of spec
section 15 to the tests that prove it. The conformance crate checks recordings
for batching, exact state, pauses, and machine menus.

```bash {"cwd":"..","excludeFromRunAll":"true","interpreter":"bash","name":"verify-conformance","tag":"local-mutation"}
cargo test -p jevscript-conformance
```

## Check the release scripts

These tests drive `scripts/` against fake `npm`, `otool`, and registry servers
on loopback. They make no registry call. A pass is fixture integration evidence,
not proof that a registry accepts anything.

```bash {"cwd":"..","excludeFromRunAll":"true","interpreter":"bash","name":"verify-release-scripts","tag":"local-mutation"}
python3 -m unittest discover -s scripts -p 'test_*integration.py' -v
```

## Check the custom decision model profiles

This test runs the built CLI against a disposable HTTP server for each profile
in `examples/custom-decision-model/profiles.json`. For a check against a real
local model, follow
[the custom decision model example](../examples/custom-decision-model/README.md)
and the
[verify-custom-decision-model Skill](../.agents/skills/verify-custom-decision-model/SKILL.md).

```bash {"cwd":"..","excludeFromRunAll":"true","interpreter":"bash","name":"verify-custom-model","tag":"local-mutation"}
cargo test -p jevscript-cli --test custom_decision_model_cli
```

## Check the installed packages and the runbooks

- To install the host's npm tarball and wheel offline and drive their CLI and
  SDKs, run `package-host-smoke` in [Packaging](packaging.md).
- To check the runbooks themselves, run `runbooks-check` in the
  [runbooks index](README.md#change-a-runbook).
