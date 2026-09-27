---
name: verify
description: Drive Jevscript's installed CLI and npm and Python SDK packages through the local release smoke when changing package or release behavior.
---

# Verify Jevscript packages

## Launch

Use the pinned Rust toolchain on a matching host. From the repository root, build and stage the binary with `python3 scripts/build_release_cli.py --target darwin-arm64` on macOS arm64, or the matching target from `scripts/stage_release.py` on another host. The command must print `validated <target> jevscript 0.1.2`. This is a short-lived CLI; no server remains running.

## Doctor

Run `target/release/jevscript --version`, `python3 scripts/build_release_cli.py --help`, and the host's OS and architecture check (`sw_vers -productVersion && uname -m` on macOS). For a macOS 15 release proof, require version 15.x and the architecture matching the staged target.

## Drive

For the full installed-package path, use the exact staging and smoke commands in `docs/package-release.md` under “Local artifact proof”, substituting the host's target and tag. The smoke installs the packed npm tarball and wheel offline in temporary directories, then invokes the installed `jevscript` commands and both SDKs. For a quick CLI check, run `target/release/jevscript check examples/inbox_triage.jev`; the installed-package smoke is the release path.

## Evidence

Capture the build output, wheel filename and tag, `otool -l` and `otool -L` for each final Mac binary, and the installed-package smoke output with exit status. Label a fake-`otool` integration run **fixture integration**, a local installed-package run **local E2E**, and a release workflow run **CI installed-package E2E**. Only the latter on both macOS 15 architectures proves both target runtimes. The bundle verifier in `scripts/verify_release_bundle.py` establishes exact npm/wheel binary parity and source commit; retain its output too. Keep logs under ignored `.release-tmp/verification/` when a durable local record is needed.

## Cleanup

The CLI and smoke start no persistent service. The smoke removes its own temporary install directories. Keep evidence logs; remove only scratch artifacts created for this verification when they are no longer needed. Do not remove another lane's worktree or a shared build cache.

## Feature map

See `features/README.md` for the user paths and their observable results.
