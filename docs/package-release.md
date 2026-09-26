# Jevscript SDK and CLI package release

This guide describes the **staged** 0.1.0 release. Nothing in this repository
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
wheel floors must be checked against the final binary's Mach-O minimum OS
before upload. Node >=22 and Python >=3.10 are required. The only locally
tested release artifact so far is **macOS arm64**, as recorded below; the
other targets require passing the staged workflow on their own runners.

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
python3 scripts/smoke_packages.py --npm .release-tmp/jevscript-0.1.0.tgz --wheel .release-tmp/wheelhouse/jevscript-0.1.0-py3-none-macosx_11_0_arm64.whl
```

The smoke opens each archive and checks an allowlist, license, wheel tag,
example, Skill digest, manifest, and forbidden content, including a
case-insensitive scan for external supervisor branding in tracked source and
both archives. It installs each package offline
with no repository binary on PATH; runs the real command for `--version` and
`check`; runs setup from both installed CLIs in scratch project and home
directories without `npx` or `skills` on PATH; loads `inbox_triage.jev` through
each SDK; executes a no-model task;
then damages each installed binary and requires the command and SDK to reject
it. `build_release_cli.py` remaps local build paths and rejects personal path
or credential markers in the binary. For Linux and Windows, use the matching
matrix target and wheel tag from `scripts/stage_release.py`.

## Staging and publication boundary

`.github/workflows/ci.yml` runs a Linux x64 package smoke. The manual
`.github/workflows/release.yml` builds all five platform wheels and binaries,
assembles the npm tarball, then installs both artifact types on each target
with Python 3.10, 3.12, and 3.14. Node 22 is used for the npm checks.
It uploads **workflow artifacts only**: a `package-bundle` holding the npm
tarball, the five wheels and their `SHA256SUMS`, which
`scripts/verify_release_bundle.py` has checked for one version, one source
commit, and identical CLI bytes per target across npm and PyPI.

`.github/workflows/publish.yml` uploads that bundle and rebuilds nothing.
Before a release:

1. Tag the release commit `v0.1.0` and run `release.yml` on that tag. Every
   target job must pass.
2. Download its `package-bundle`, read the archive contents and `SHA256SUMS`,
   and record the SHA-256 of `SHA256SUMS` itself.
3. Configure trusted publishers for this repository's `publish.yml`: PyPI with
   environment `pypi`, npm with environment `npm`. Protect both GitHub
   environments with required reviewers. No registry token is stored.
4. Run `publish.yml` on the same tag with the staging run ID and the recorded
   digest. It requires that run to be a successful `release.yml` run of the
   tag's commit and rechecks every bundle digest. Before uploading, it compares
   any wheels already on PyPI with the verified bytes. After the upload, it
   requires every verified wheel on PyPI to have the same SHA-256 digest, then
   publishes the tarball to npm with provenance. A rerun skips wheels already
   present only when their bytes match the verified bundle.

Registry names were unclaimed by public lookup on 2026-09-26, but that does
not reserve them. npm/PyPI versions and the packaged CLI version must match
exactly. A broken publication needs a new patch version; never rebuild or
overwrite an existing name/version pair. If only one registry accepts a
version, treat it as an incident: publish the same verified bytes to the
other registry rather than rebuilding.
