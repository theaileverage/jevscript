---
name: verify
description: Prove the Jevscript release packages work the way a user installs them — build the release `jevscript` CLI for this host, stage it into the npm tarball and the PyPI wheel, install both offline and drive the installed CLI and SDKs. Use after touching scripts/build_release_cli.py, scripts/stage_release.py, scripts/smoke_packages.py, sdk packaging, version sites or the release workflows.
---

# Verify the Jevscript release packages

The user-facing surface here is what `npm install jevscript` and
`pip install jevscript` put on a machine: a `jevscript` command, the JS and
Python SDKs that spawn `jevscript serve`, and the bundled examples and Skill.
Everything below drives that surface through the repo's own release scripts,
on the **host target only**. Other targets cannot be built or run here; their
proof is the matching CI lane (see `features/ci-release-lanes.md`).

Nothing here uploads to a registry or creates a tag. Never run
`.github/workflows/publish.yml` or `scripts/publish_npm.py` to verify.

## Launch

There is no server. Launch means building the release CLI once:

```sh
python3 scripts/build_release_cli.py --target "$(python3 -c 'import sys; sys.path.insert(0, "scripts"); from build_release_cli import host_target; print(host_target())')"
```

Ready when it prints `validated <target> jevscript <version> (<n> bytes)`.
It builds with `CARGO_HOME=.release-tmp/cargo-home`, so the first run fetches
the whole registry (several minutes); later runs reuse it.

## Doctor

Read-only; run it first and whenever a step fails for a reason that looks
environmental:

```sh
.claude/skills/verify/scripts/doctor.sh
```

It prints the host target, toolchain, Node/pnpm/uv/Python versions and the
Rust, npm and PyPI version strings, and exits non-zero if the three versions
disagree or Node is older than 22.

## Drive

The whole package path for this host, into an evidence directory:

```sh
.claude/skills/verify/scripts/package-smoke.sh [evidence-dir]
```

Steps, each logged to `<evidence-dir>/<step>.log`: release build and
embedded-path scan, npm and wheel staging, `uv build --wheel`, `pnpm build`
plus `npm pack`, `scripts/bundle.sh`, which runs the assemble-and-verify
step of `.github/actions/upload-package-bundle` exactly as CI does on a
one-target bundle in `.release-tmp/package-bundle` (the upload itself runs
only on GitHub; see `features/ci-release-lanes.md`), then
`scripts/smoke_packages.py`, which installs both
artifacts offline in scratch directories with no repository binary on PATH,
runs the installed CLI (`--version`, `check`, `setup`), loads and runs a task
through each SDK, and requires a damaged binary to be rejected. The feature
files say what each step proves.

## Evidence

The evidence directory defaults to
`${TMPDIR:-/tmp}/jevscript-verify/<epoch>` and is printed at the start and the
end. It holds every step log, `summary.txt` (step, exit code) and
`artifacts.sha256` for the built tarball and wheel. Proof standards:

- Only the installed packages count. A passing `cargo test` or SDK unit run
  says nothing about what the archives contain.
- A step that did not run is not a pass. Report which target you proved and
  label it local E2E; every other target needs its CI lane's log.
- A scan failure prints each match's offset and up to 160 bytes of context;
  quote that context, never the whole binary. A credential marker's value is
  withheld by design.

## Cleanup

```sh
.claude/skills/verify/scripts/cleanup.sh
```

Removes the staged trees (`.release-tmp/binaries`, `.release-tmp/wheelhouse`,
`.release-tmp/*.tgz`, `sdk/js/native`, `sdk/js/examples`, `sdk/js/skills`,
`sdk/python/src/jevscript/_bin`, `.../examples`, `.../skills`). It keeps
`.release-tmp/cargo-home` (a download cache) and never touches the evidence
directory. Every path it removes is gitignored.
