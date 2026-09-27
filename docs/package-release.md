# Jevscript SDK and CLI package release

This guide describes the **staged** 0.1.2 release. Nothing in this repository
uploads to npm or PyPI automatically. The spec's command is `jevscript`, and
both host SDKs invoke the release-matched `jevscript serve` binary (spec §11.5–11.6).

## Artifacts and target claims

`npm` package `jevscript` contains its ESM SDK, a `jevscript` command shim and
all five target binaries. PyPI project `jevscript` has one wheel per target,
each with its Python SDK, command entry point and native binary. Binary SHA-256
digests live in the package manifest and are checked before execution.
`THIRD_PARTY_NOTICES.txt` beside the binaries carries the license texts of
every Rust crate linked into the CLI. Neither
installer needs Rust, a postinstall script, or a binary download. The packages
include `inbox_triage.jev` for compilation and `package_smoke.jev` for an
offline, no-model task. They also include the same `skills/jevscript/SKILL.md`
bytes, embedded in the native CLI for `jevscript setup`.

After installing either package, run `jevscript setup --agent codex` or
`jevscript setup --agent claude-code` in the target project. Repeat `--agent`
to install for both. The default canonical location is
`<project>/.agents/skills/jevscript`; Claude Code receives a link under
`<project>/.claude/skills/jevscript`. `--global` installs under the user's home
with links in `~/.codex/skills/jevscript` and/or
`~/.claude/skills/jevscript`. `--copy` replaces links with independent copies.
`--project <dir>` selects an existing project directory. Setup is explicit,
offline, and refuses unmanaged or changed destinations. Repeating the same
setup reports already installed paths; resolve any collision yourself before
rerunning. Package installation never invokes setup automatically.

Target staging matrix: macOS arm64/x86_64, glibc Linux arm64/x86_64, and Windows
x86_64. Linux wheels currently target `manylinux_2_39`, matching the configured
Ubuntu 24.04 build runners; do not advertise older glibc compatibility. macOS
wheel floors are checked against the final binary's Mach-O minimum OS by
`scripts/build_release_cli.py` before staging. Node >=22 and Python >=3.10 are required. The only locally
tested release artifact so far is **macOS arm64**, as recorded below; the
other targets require passing the staged workflow on their own runners.
That Mach-O check reads the main executable header. Before upload, inspect
`otool -L` and `otool -l` on both final macOS binaries for linked-library
requirements, then run the installed packages on macOS 10.15 x86_64 and
macOS 11 arm64 or raise the wheel tags to the oldest versions actually tested.

## Local artifact proof

Use the pinned Rust toolchain and release version. The following commands
write generated artifacts only under ignored package staging paths:

```sh
python3 scripts/build_release_cli.py --target darwin-arm64
python3 scripts/stage_release.py npm --binary-root .release-tmp/binaries --targets darwin-arm64
python3 scripts/stage_release.py wheel --binary-root .release-tmp/binaries --targets darwin-arm64
cd sdk/js && pnpm install --frozen-lockfile && pnpm run build
JEVSCRIPT_PACK_TARGETS=darwin-arm64 npm pack --pack-destination ../../.release-tmp
cd ../python
JEVSCRIPT_TARGET=darwin-arm64 JEVSCRIPT_PLATFORM_TAG=macosx_11_0_arm64 JEVSCRIPT_WHEEL_TAG=py3-none-macosx_11_0_arm64 uv build --wheel --out-dir ../../.release-tmp/wheelhouse
cd ../..
python3 scripts/smoke_packages.py --npm .release-tmp/jevscript-0.1.2.tgz --wheel .release-tmp/wheelhouse/jevscript-0.1.2-py3-none-macosx_11_0_arm64.whl
```

The smoke opens each archive and checks an allowlist, license, wheel tag,
example, Skill digest, manifest, LF-only Skill and examples, and forbidden
content, including a case-insensitive scan for external supervisor branding
in tracked source and both archives. It installs each package offline
with no repository binary on PATH; runs the real command for `--version` and
`check`; runs setup from both installed CLIs in scratch project and home
directories without `npx` or `skills` on PATH; loads `inbox_triage.jev` through
each SDK; executes a no-model task;
then damages each installed binary and requires the command and SDK to reject
it. `build_release_cli.py` builds with its own `CARGO_HOME` under
`.release-tmp` (the first run downloads the registry), remaps local build
paths and rejects personal path or credential markers in the binary. For
Linux and Windows, use the matching matrix target and wheel tag from
`scripts/stage_release.py`.

## Staging and publication boundary

`.github/workflows/ci.yml` runs Linux x64 and Windows x64 package smokes on
every push and pull request, so the MSVC build and its embedded-path scan
pass before a release tag, which never moves, exists. The Linux lane also
uploads a representative one-target `package-bundle` through the same
`.github/actions/upload-package-bundle` action the release uses, and a
separate job downloads it and verifies its digests against the uploaded
`SHA256SUMS`, so the artifact roundtrip the install matrix and publication
depend on is proven before tagging. `.gitattributes` checks
every text file out with LF on every platform, so the Windows wheel's Skill
bytes and digest match the Linux-built npm tarball. The manual
`.github/workflows/release.yml` builds all five platform wheels and binaries,
assembles the npm tarball, then installs both artifact types on each target
with Python 3.10, 3.12, and 3.14. Node 22 is used for the npm checks.
It uploads **workflow artifacts only**: a `package-bundle` holding the npm
tarball, the five wheels and their `SHA256SUMS`, which
`scripts/verify_release_bundle.py` has checked for one version, one source
commit, and identical CLI bytes per target across npm and PyPI. The upload
fails the job when it finds no files, and the log prints the SHA-256 of
`SHA256SUMS` for review. v0.1.0 and v0.1.1 were tagged but never published;
v0.1.1's staging run uploaded no bundle because its hidden `.release-bundle/`
directory was skipped by the upload action, so 0.1.2 is the first release.

`.github/workflows/publish.yml` uploads that bundle and rebuilds nothing.
Before a release:

1. Merge the reviewed release commit into protected `main`. Protect `v*`
   against retargeting and deletion, then tag that merged commit `v0.1.2`.
   Run `release.yml` on the tag. Its source-ref job requires the tag's commit
   to be in `main` history; every target job must pass.
2. Download its `package-bundle`, read the archive contents and `SHA256SUMS`,
   and record the SHA-256 of `SHA256SUMS` itself.
3. Create protected GitHub environments `npm` and `pypi` with required
   reviewers. Register a PyPI pending trusted publisher for project `jevscript`,
   GitHub owner `theaileverage`, repository `jevscript`, workflow `publish.yml`,
   and environment `pypi`. A pending publisher does not reserve the name.
   Confirm the npm account can create `jevscript` and has 2FA enabled. Set
   `NPM_EXPECTED_OWNER` in the protected `npm` environment to that account's
   exact npm username. npm requires an existing package before a trusted
   publisher can be configured.
4. For the first npm publication only, give the `npm` environment an
   owner-controlled, short-lived `NPM_BOOTSTRAP_TOKEN` with publish permission.
   Run `publish.yml` on the same tag with the staging run ID, the reviewed
   digest, and `npm_bootstrap: true`. Its `verify` job binds the successful
   staging run to the tag's commit and rechecks every bundle digest. The
   protected bootstrap job checks that the credential's `npm whoami` matches
   `NPM_EXPECTED_OWNER`. It publishes the exact reviewed npm tarball with
   provenance, then requires that account to appear in `npm owner ls` as a
   writer. If `0.1.2` already exists, it accepts the version only when both
   the registry SHA-512 integrity and owner match; a same-byte package under
   another account fails. The PyPI job runs after bootstrap; it compares any
   existing wheels with the reviewed bytes, uploads missing wheels, and then
   requires exactly the five reviewed filenames and SHA-256 digests, with no
   sdist. The final npm job checks the same tarball and owner. Neither package
   is rebuilt.
5. Once npm holds `jevscript@0.1.2`, configure its trusted publisher for
   `theaileverage/jevscript`, workflow filename `publish.yml`, environment
   `npm`, and permission for `npm publish`. Remove `NPM_BOOTSTRAP_TOKEN` from
   the GitHub environment. For later releases, dispatch `publish.yml` with
   `npm_bootstrap: false`; its normal job publishes through OIDC and accepts an
   already published version only after the same exact-integrity check.

Registry names were unclaimed by public lookup on 2026-09-26, but that does
not reserve them. npm/PyPI versions and the packaged CLI version must match
exactly. A broken publication needs a new patch version; never rebuild or
overwrite an existing name/version pair. If only one registry accepts a
version, treat it as an incident. Read PyPI's version JSON and npm's version
metadata, compare their wheel SHA-256 and tarball SHA-512 integrity with the
reviewed bundle, and rerun `publish.yml` with the same tag, staging run ID,
and digest. The workflow skips already published bytes only after matching
them and checking the expected npm writer. If a different account controls a
same-byte npm version, stop and resolve ownership before considering release
complete. A byte mismatch requires a new patch version; never rebuild or
overwrite an existing name/version pair.
