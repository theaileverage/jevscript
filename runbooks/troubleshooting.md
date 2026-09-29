# Troubleshooting

Each section starts with the message you see, then gives the cause and the fix.

## `CERTIFICATE_VERIFY_FAILED` from a release script

`scripts/check_pypi_wheels.py` and `scripts/publish_npm.py` read the registries
with Python's `urllib`. The python.org installer for macOS ships without a CA
bundle, so `urllib` rejects every HTTPS certificate until you run
`Install Certificates.command` from that Python's `/Applications/Python 3.x`
folder. For one command, set `SSL_CERT_FILE=/etc/ssl/cert.pem` to use the
system's CA bundle instead. `trouble-python-tls` shows where Python looks for
certificates and tries one HTTPS request.

```bash {"cwd":"..","excludeFromRunAll":"true","interpreter":"bash","name":"trouble-python-tls","tag":"network-read"}
python3 -c 'import ssl; print(ssl.get_default_verify_paths())'
python3 -c 'import urllib.request; urllib.request.urlopen("https://pypi.org/simple/", timeout=15); print("python HTTPS ok")'
```

## `runner is <host>, not <target>`

`scripts/build_release_cli.py` builds only for the machine it runs on. The
other targets are built by their CI runners in `release.yml`, and by
`package-smoke` and `windows-package-smoke` in `ci.yml`.

## `release CLI embeds N private path or credential markers`

The release binary contains `/Users/`, `/home/runner/`, `C:\Users\`, or
`TYPESAFE_API_KEY=`. `build_release_cli.py` builds with its own `CARGO_HOME`
under `.release-tmp/cargo-home` and remaps that path and the home directory for
rustc. C code that build scripts compile is outside rustc's remap. Under MSVC it
keeps absolute `__FILE__` paths, which is why the registry must not sit in the
user profile. See
[the release binary notes](../.claude/skills/verify/features/release-binary.md).
`trouble-binary-markers` lists every match in the last release build with its
offset and context. It withholds any credential value.

```bash {"cwd":"..","excludeFromRunAll":"true","interpreter":"bash","name":"trouble-binary-markers","tag":"inspection"}
python3 -c 'import sys; sys.path.insert(0, "scripts"); from build_release_cli import marker_matches; print("\n".join(marker_matches(open(sys.argv[1], "rb").read())) or "no markers")' target/release/jevscript
```

## `darwin-<arch> binary declares macOS <version>, wheel advertises (15, 0)`

The Mach-O minimum OS must equal the macOS 15.0 wheel floor.
`build_release_cli.py` sets `MACOSX_DEPLOYMENT_TARGET=15.0` for its own build.
A binary built another way does not get it. Rebuild with `package-build-cli`
and check it with `package-macos-binary` in [Packaging](packaging.md).

## `Rust, JS and Python release versions must agree` or `versions disagree`

A version site was missed in a bump. Run `release-version-sites` in
[Release](release.md) to find it.

## `unexpected CLI version`

`target/release/jevscript --version` does not print the Cargo workspace version.
The binary is stale or the bump is incomplete. Run `release-version-sites`, then
`package-build-cli`.

## `GLIBC_2.xx not found` on Linux

The Linux wheels are tagged `manylinux_2_39` because the Ubuntu 24.04 runners
build them. They need glibc 2.39 or later. Do not claim older glibc support,
and do not move the Linux builds to a newer runner image without changing the
wheel tag.

## An SDK test cannot start `jevscript`

The SDK suites start `target/debug/jevscript`. Run `dev-build` in
[Development](development.md), or set `JEVSCRIPT_BIN` to another build.

## The adapter integration test fails before it starts

The adapter suite drives a real `tmux` server. Install `tmux`, then run
`dev-adapters` again. `dev-doctor` reports whether `tmux` is on `PATH`.

## `jevscript setup` refuses a destination

Setup refuses destinations it does not manage and managed Skills whose bytes
changed. Resolve the collision yourself, then run setup again. The
[setup options](../docs/cli.md#setup-options) describe `--copy`, `--global`,
and `--project`.

## `replay_diverged`

The replay no longer matches the recorded program, profile, or event sequence.
See [`replay_diverged` in the error reference](../docs/error-reference.md#replay_diverged).

## A package tarball has an unexpected name

`npm pack` names the scoped package's tarball
`theaileverage-jevscript-<version>.tgz`. The npm registry serves the same bytes
as `jevscript-<version>.tgz`.

## The staging run uploaded an empty `package-bundle`

`actions/upload-artifact` skips a search root whose name starts with a dot when
`include-hidden-files` is false. That is how `v0.1.1` uploaded nothing. Keep
the bundle directory at `.release-tmp/package-bundle/`, whose last path
component is not hidden.

## A runbook cell stops with `Set <NAME>`

The cell needs that input. Set it on the same command line, for example
`STAGING_RUN_ID=123 runme run publish-check-pypi`.
