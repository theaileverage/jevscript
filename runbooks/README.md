# Jevscript runbooks

These runbooks hold the repository's operational procedures as named
[Runme](https://runme.dev) cells. Each cell runs one step from the repository
root. The source documents stay the authority: [AGENTS.md](../AGENTS.md#checks)
for the checks, [the package release guide](../docs/package-release.md) for the
release contract, and the workflows in [.github/workflows](../.github/workflows).
When a runbook disagrees with its source, fix the runbook.

| Runbook | Use it to | Source |
| --- | --- | --- |
| [Development](development.md) | Format, lint, build, and test the workspace, SDKs, adapters, and editors. | [AGENTS.md](../AGENTS.md#checks), [ci.yml](../.github/workflows/ci.yml) |
| [Verification](verification.md) | Run the conformance, release-script, and custom-model suites on their own. | [Conformance evidence](../docs/conformance.md) |
| [Packaging](packaging.md) | Build the host's npm tarball and wheel, install them offline, and smoke-test them. | [Local artifact proof](../docs/package-release.md#local-artifact-proof), [verify Skill](../.claude/skills/verify/SKILL.md) |
| [Release](release.md) | Bump the version, tag the merged commit, stage `release.yml`, and review the bundle. | [Staging and publication boundary](../docs/package-release.md#staging-and-publication-boundary) |
| [Publication](publication.md) | Read the registries, dispatch `publish.yml`, and verify the published bytes. | [publish.yml](../.github/workflows/publish.yml) |
| [Incidents and rollback](incidents-and-rollback.md) | Recover from a split registry, a failed run, or bad published bytes. | [Package release guide](../docs/package-release.md) |
| [Troubleshooting](troubleshooting.md) | Match an error message to its cause and fix. | The scripts in [scripts](../scripts) |
| [Secrets and environment](secrets-and-environment.md) | Find which variables and secrets each step reads, and check which are set. | [CLI environment variables](../docs/cli.md#environment-variables) |

## Install Runme

The runbooks are tested with Runme 3.17.5. Install it with `brew install runme`,
or download the archive for your platform from the
[Runme v3.17.5 release](https://github.com/runmedev/runme/releases/tag/v3.17.5)
and check it against that release's `checksums.txt`. CI installs the same
version in the `runbooks` job of [ci.yml](../.github/workflows/ci.yml).

The cells are written for macOS and Linux maintainers. Windows builds run only
in CI.

## Run a cell

Run every command from the repository root. Do not use `--chdir` instead:
with it, Runme reads only the root `README.md`.

- `runme list` lists every named cell and its file.
- `runme run --dry-run NAME` prints the script a cell would run, without running it.
- `runme run NAME` runs one cell.
- `runme run --all` runs every cell that Run All includes. That is the read-only
  `inspection` set described below.
- `runme run --all --filename runbooks/release.md` limits Run All to one runbook.

A cell that needs input reads it from an environment variable. Set the variable
on the same command line, for example
`RELEASE_TAG=v0.1.4 runme run release-tag-check`. A cell stops with a message
naming the variable when it is missing.

Do not pass `--allow-unnamed`. It turns the unnamed examples in `README.md` and
`docs/` into cells, and `runme run --all --allow-unnamed` would run them.

## Effect tags

Every cell has a `tag` that names its strongest effect.

| Tag | What the cell does | In Run All |
| --- | --- | --- |
| `inspection` | Reads files in this checkout and uses no network. It writes nothing except Python's gitignored `__pycache__`. | Yes, unless it needs a build or a downloaded bundle first |
| `network-read` | Reads a registry, GitHub, or a download. Some cells need `gh auth login`. It changes nothing that other people see. | No |
| `local-mutation` | Writes in this checkout or on this machine: `target/`, `node_modules/`, `.release-tmp/`, the staged package trees under `sdk/`, a tracked file, a local tag, or the rustup toolchain. | No |
| `external-mutation` | Changes what other people see: pushes a tag, dispatches or reruns a workflow. | No |

Runme never runs a cell marked `excludeFromRunAll` from `--all`, even when you
also pass `--tag`. Run a mutating cell by its name.

These rules hold for every runbook:

- An `external-mutation` cell has no default input. It stops unless you set each
  variable it names.
- No cell publishes to a registry from your machine, writes a secret, deletes or
  moves a tag, or deprecates or yanks a version. The runbooks describe those
  steps in prose for a maintainer to do by hand.
- The only deletions are the staged package trees, through the verify Skill's
  cleanup script, and paths under `.release-tmp/`.
- The first `cargo` command in a checkout can make rustup download the
  toolchain pinned in `rust-toolchain.toml`.
- Runme loads `.env` and `.env.local` from the root into every cell by default. Read
  [Secrets and environment](secrets-and-environment.md#keep-local-secrets-out-of-cells)
  before you run a cell with a local `.env`.

## Change a runbook

Every fenced block in `runbooks/` must be a named `bash` cell. Runme runs
unnamed blocks too when you pass `--filename` with `--all`, so write an example
command in inline code, not in a fenced block. Give each cell `"cwd":".."`,
`"interpreter":"bash"`, and a `tag`. Give every cell that is not an
`inspection` cell `"excludeFromRunAll":"true"`. Link to a source document or
script instead of copying a long procedure.

Pipe `gh` output through `cat` in a cell. Runme runs each cell in a
pseudo-terminal, and `gh` writing to a terminal waits for a reply that a
non-interactive run never sends.

`scripts/check_runbooks.py` enforces these rules through the `runme` CLI. It
dry-runs every cell, checks that each one runs in the repository root under
bash, and fails when a Markdown file outside `runbooks/` has a named cell. CI
runs it on every push and pull request. Set `RUNME` to use a `runme` binary
that is not on `PATH`.

```bash {"cwd":"..","interpreter":"bash","name":"runbooks-check","tag":"inspection"}
python3 scripts/check_runbooks.py
```
