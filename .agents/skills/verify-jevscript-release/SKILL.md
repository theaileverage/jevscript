---
name: verify-jevscript-release
description: Verify Jevscript release artifacts through the shipped npm and Python package boundaries.
---

# Verify Jevscript release artifacts

## Launch

Run from this repository's isolated worktree. Build the pinned host CLI with `python3 scripts/build_release_cli.py --target darwin-arm64`, stage npm and wheel assets with `scripts/stage_release.py`, build `sdk/js`, then pack and build the wheel as shown in `docs/package-release.md`. On other hosts, use the matching target and wheel tag from `scripts/stage_release.py`.

## Doctor

Confirm the worktree path and branch, clean disposable `.release-tmp` ownership, pinned Rust toolchain, Node >=22, Python >=3.10, and matching host target. Inspect package versions in `Cargo.toml`, `sdk/js/package.json`, and `sdk/python/pyproject.toml`. Confirm `native/manifest.json` records the current commit and all target digests before packing. Do not use a previously staged binary after a source or version change.

## Drive

Run `python3 scripts/smoke_packages.py --npm <fresh tarball> --wheel <fresh host wheel>`. The smoke installs both exact artifacts into disposable directories, invokes their shipped CLIs and SDKs, exercises offline setup and a no-model task, and checks that a damaged binary is rejected. Run `python3 -m unittest discover -s scripts -p 'test_*integration.py' -v` for release command integrations. For a full five-target staging bundle, run `scripts/verify_release_bundle.py` with the expected source commit and reviewed `SHA256SUMS` digest.

## Evidence

Keep the command log, artifact names and SHA-256 digests, current source commit, host OS/architecture, and exit statuses. Label a local package smoke as local E2E and a fake registry test as fixture integration. A single-host pass does not establish the five-target release matrix, registry identity, protected environments, or a live registry install.

## Cleanup

Remove only disposable paths created by this run under `.release-tmp`; preserve the artifact and command log used as evidence. Never delete a shared cache, other worktree, registry object, tag, or publication run.
