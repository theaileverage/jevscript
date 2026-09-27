# Release CLI build and embedded-path scan

`scripts/build_release_cli.py --target <host>` builds `jevscript-cli` in release
mode, checks `--version` against the Cargo workspace version, checks the macOS
minimum OS on darwin, and refuses a binary holding `/Users/`, `/home/runner/`,
`C:\Users\` or `TYPESAFE_API_KEY=`. It then copies the binary to
`.release-tmp/binaries/<target>/`.

## Sub-features

- Neutral build paths: `CARGO_HOME` is `.release-tmp/cargo-home`, remapped to
  `/cargo` for rustc, and the home directory is remapped to `/build`. C code
  compiled by build scripts (aws-lc-sys) is not covered by rustc's remap; with
  MSVC it keeps absolute `__FILE__` paths, which is why the registry must sit
  outside the user profile.
- Scan report: each match prints its offset and up to 160 bytes of context,
  cut at NUL and at any credential marker; at most 20 are shown.

## How to get to it (user POV)

A release operator runs it, locally or as the first step of each CI package
job. Its output ends with `validated <target> jevscript <version> (<n> bytes)`.

## Driving it with package-smoke.sh

The `build` step. Evidence: `<evidence>/build.log`. To see what the scan would
report, inspect the built binary directly:

```sh
python3 -c 'import sys; sys.path.insert(0, "scripts"); from build_release_cli import marker_matches; print("\n".join(marker_matches(open(sys.argv[1], "rb").read())) or "no markers")' target/release/jevscript
```

## Gotchas

- The first run with an empty `.release-tmp/cargo-home` downloads the registry.
- `--target` must equal the host; there is no cross build.
- `~/.cargo/config.toml` does not apply to this build, because `CARGO_HOME`
  moves.
