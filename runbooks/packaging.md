# Build and smoke-test the host packages

A local build proves the packaging path for this machine's target only. Its npm
tarball carries one binary, and the release tarball carries five. Only
`release.yml` builds the five-target set, on one runner per target. Nothing on
this page uploads anything.

[Local artifact proof](../docs/package-release.md#local-artifact-proof) is the
contract. The [verify Skill](../.claude/skills/verify/SKILL.md) scripts run it,
and these cells call those scripts rather than copying their steps.

## Check the machine

`package-doctor` prints the host target, the toolchain, the Node, pnpm, uv, and
Python versions, and the three package versions. It fails when the versions
disagree, when Node is older than 22, or when one of those tools is missing.

```bash {"cwd":"..","excludeFromRunAll":"true","interpreter":"bash","name":"package-doctor","tag":"inspection"}
.claude/skills/verify/scripts/doctor.sh
```

## Build, install, and smoke-test both packages

`package-host-smoke` builds the release CLI, stages it into both SDK trees,
builds the wheel and the npm tarball, assembles and verifies a one-target
bundle, then runs `scripts/smoke_packages.py`. The smoke installs both archives
offline with no repository binary on `PATH`, runs the installed CLI and both
SDKs, and requires a damaged binary to be rejected.

The cell writes `.release-tmp/`, the gitignored staged trees under `sdk/`, and
an evidence directory, which defaults to `$TMPDIR/jevscript-verify/<epoch>`.
The first run downloads the crates registry into `.release-tmp/cargo-home`,
which takes several minutes. The cell prints `PASS <target> <version>` when
every step passes.

```bash {"cwd":"..","excludeFromRunAll":"true","interpreter":"bash","name":"package-host-smoke","tag":"local-mutation"}
.claude/skills/verify/scripts/package-smoke.sh
```

To rebuild only the release CLI and its embedded-path scan, run
`package-build-cli`. It copies the binary to
`.release-tmp/binaries/<target>/`.

```bash {"cwd":"..","excludeFromRunAll":"true","interpreter":"bash","name":"package-build-cli","tag":"local-mutation"}
target=$(python3 -c 'import sys; sys.path.insert(0, "scripts"); from build_release_cli import host_target; print(host_target())')
python3 scripts/build_release_cli.py --target "$target"
```

## Inspect a macOS binary

The package release guide asks for `otool -l` and `otool -L` on both macOS
binaries before upload. On a Mac, after `package-build-cli`, this cell checks
the Mach-O minimum OS against the wheel floor and lists the linked libraries.

```bash {"cwd":"..","excludeFromRunAll":"true","interpreter":"bash","name":"package-macos-binary","tag":"inspection"}
target=$(python3 -c 'import sys; sys.path.insert(0, "scripts"); from build_release_cli import host_target; print(host_target())')
binary=".release-tmp/binaries/$target/jevscript"
python3 scripts/check_macos_binary.py --target "$target" "$binary"
otool -L "$binary"
```

## Remove the staged trees

`package-cleanup` deletes the gitignored staged trees: `.release-tmp/binaries`,
`.release-tmp/wheelhouse`, the tarballs in `.release-tmp`,
`.release-tmp/package-bundle`, and the native, example, and Skill trees under
`sdk/js` and `sdk/python/src/jevscript`. It keeps `.release-tmp/cargo-home` and
every evidence directory.

```bash {"cwd":"..","excludeFromRunAll":"true","interpreter":"bash","name":"package-cleanup","tag":"local-mutation"}
.claude/skills/verify/scripts/cleanup.sh
```
