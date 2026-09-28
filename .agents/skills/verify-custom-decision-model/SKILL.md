---
name: verify-custom-decision-model
description: Verify the Laya and Kev Jevscript example through the CLI and, when available, a public checkpoint.
---

# Verify the custom decision model example

## Launch

Work from an isolated Jevscript checkout. `cargo test -p jevscript-cli --test custom_decision_model_cli` builds the CLI and starts a disposable HTTP server inside the test. For a live provider check, start Laya or Kev in a separate directory using `examples/custom-decision-model/README.md` and record the exact checkpoint revision. Do not start both servers unless the run needs both.

## Doctor

Confirm `pwd -P` and `git status --short --branch` identify the intended checkout. Confirm the pinned Rust toolchain can build `jevscript`, `examples/custom-decision-model/urgent.jev` passes `jevscript check`, and `profiles.json` names the endpoint actually started. For a live check, send the provider's documented `/v1/systemone` curl smoke before running Jevscript. Confirm the configured bearer token matches that local server without printing its value.

## Drive

Run `cargo test -p jevscript-cli --test custom_decision_model_cli`. It invokes the built CLI, selects both example profiles, sends requests to loopback HTTP, and checks the decoded Noul, Choice, and Score results. To check an installed CLI, run `cargo install --path crates/jevscript-cli --root <run-owned-dir> --locked --debug`, then set `JEVSCRIPT_BIN=<run-owned-dir>/bin/jevscript` for that same integration test. For a live check, run the README's `jevscript judge` command for the started provider. Record the exit code and typed JSON answer.

## Evidence

Keep the checkout commit, command and exit code, server kind, endpoint, selected model ID, and redacted response. Label the automated server run **fixture integration**. Label a completed run through downloaded Laya or Kev weights **local E2E** and state the exact checkpoint revision. A fixture pass proves Jevscript's HTTP selection and answer parsing, not provider inference or decision quality.

## Cleanup

The integration test owns and closes its loopback server. For a live check, stop only the provider process started for this run and remove only its run-owned temporary files. Preserve the command and response evidence; do not remove downloaded weights or another process's cache.
