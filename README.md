# Jevscript

https://jevscript.sh

Jevscript is a small language for agent control loops. A program asks typed
questions, and [Jev](https://docs.typesafe.ai), TypeSafe's System One model,
answers them with probabilities. The program owns the loop, the thresholds,
the budgets, and the policy. The host binds terminal agents, people, text
models, and tools as capabilities at run time, and every run can be recorded
and replayed with zero model calls.

```
program inbox_triage

in message: text

judgment triage(message):
  urgent  = message feels "needs a response within the hour"
  owner   = message pick:
    code      "asks for a code change or reports a bug"
    customer  "a customer asking for help or a reply"
    schedule  "asks to find or move a meeting time"
    other
  risky   = message feels "acting on it could send, pay, delete or commit something"
  effort  = message rate:
    trivial  "answerable in one line"
    hour     "an hour of focused work"
    project  "a multi-day project"
```

That is four typed answers from one Jev request, with no output parsing.
Compiling and checking a program is local. Running a judgment calls Jev and
needs a `TYPESAFE_API_KEY`.

## Install

Each package contains its SDK and the `jevscript` command with a native
binary. Installation needs no Rust toolchain, install script, or download.

| Package | Install | Published version | Needs |
| --- | --- | --- | --- |
| npm [`@theaileverage/jevscript`](https://www.npmjs.com/package/@theaileverage/jevscript) | `npm install @theaileverage/jevscript` | 0.1.3 | Node 22+ |
| PyPI [`jevscript`](https://pypi.org/project/jevscript/) | `python -m pip install jevscript` | 0.1.2 | Python 3.10+ |

Both packages run on macOS 15 (arm64 and x86_64), glibc Linux (arm64 and
x86_64), and Windows (x86_64). To check the install, run
`npx jevscript --version` in an npm project or `jevscript --version` in the
Python environment.

## Start as a developer

[Get started](docs/getting-started.md) takes you from install to a recorded,
replayed run and a small Python or JavaScript host. It needs no API key.

Then read [how Jevscript programs work](docs/concepts.md) and the host API for
[JavaScript](sdk/js/README.md) or [Python](sdk/python/README.md).

## Start as a coding agent

To write Jevscript in a project, install the bundled Skill from the project
root:

```sh
jevscript setup --agent codex --agent claude-code
```

The Skill goes to `.agents/skills/jevscript`, and Claude Code gets a link at
`.claude/skills/jevscript`. Setup runs offline and never runs during package
installation. The [CLI reference](docs/cli.md#setup-options) covers
`--global`, `--copy`, and `--project`.

These commands check and run a program without a model or an API key:

- `jevscript check file.jev` prints each diagnostic as
  `file:line:col: code: message` on stderr, with a link to the
  [error reference](docs/error-reference.md). It exits with code 1 on any
  error.
- `jevscript compile file.jev` prints the linked IR as JSON on stdout.
- `jevscript run file.jev --stub NAME --record new.jsonl` runs `main` with a
  demonstration adapter for capability `NAME`.
- `jevscript replay new.jsonl` replays a recording with no model or adapter
  calls.

To change this repository, read [AGENTS.md](AGENTS.md) first. It has the
crate map, the conventions, and every check a change must pass. The
[language specification](spec/jevscript-language-specification.md) decides
the language, and only the implementation lead edits `spec/`.

## Documentation

| Page | Read it to |
| --- | --- |
| [Get started](docs/getting-started.md) | Write, run, record, and replay a first program. |
| [How Jevscript programs work](docs/concepts.md) | Understand judgments, tasks, machines, pauses, and recordings. |
| [CLI reference](docs/cli.md) | Look up commands, flags, recordings, and environment variables. |
| [JavaScript SDK](sdk/js/README.md) and [Python SDK](sdk/python/README.md) | Drive runs from a host program. |
| [Agent adapters](adapters/README.md) | Bind Claude Code, Codex, and other terminal agents. |
| [Editor support](docs/editors.md) | Set up VS Code, Cursor, Windsurf, Neovim, Helix, or Zed. |
| [Error reference](docs/error-reference.md) | Fix a diagnostic by its code. |
| [Language specification](spec/jevscript-language-specification.md) | Read the rules the compiler and runtime follow. |
| [Conformance evidence](docs/conformance.md) | See which tests prove each acceptance item of spec section 15. |
| [Package release guide](docs/package-release.md) | Build, verify, and publish the npm and PyPI packages. |

The [examples](examples) directory has a coding harness (`fix_issue.jev`), a
judgment-only program (`inbox_triage.jev`), and a review machine
(`review_loop.jev`). The [Chief of Staff example](examples/chief-of-staff/README.md)
is a larger Python host application.

## Releases

npm and PyPI are where the packages are published. Each release also has an
immutable Git tag, such as `v0.1.3`. The release workflow builds and checks
the packages and uploads them as a GitHub Actions artifact. The publish
workflow sends that artifact to the registries. Neither workflow creates a
GitHub Release. A maintainer creates one from the verified artifact.

The [v0.1.3 GitHub Release](https://github.com/theaileverage/jevscript/releases/tag/v0.1.3)
attaches the npm tarball, the five platform wheels, and `SHA256SUMS`. PyPI
serves `jevscript` 0.1.2, so download the 0.1.3 wheels from that release
page, not from PyPI. The
[package release guide](docs/package-release.md#where-a-release-appears)
describes both workflows.

## Build from source

The Rust toolchain is pinned in `rust-toolchain.toml`.

```sh
cargo build --workspace
cargo test --workspace
target/debug/jevscript --help
```

The SDKs start `jevscript serve` and speak JSON-RPC over stdio. Their tests
use `target/debug/jevscript`, and a host uses a source build when
`JEVSCRIPT_BIN` points at it. [AGENTS.md](AGENTS.md#checks) lists the SDK,
adapter, and editor test commands. The agent
adapters in `adapters/` are not published. Build them with `pnpm install &&
pnpm run build` in that directory.

```
spec/        the language specification
crates/      syntax, IR, compiler, runtime, CLI, language server, conformance checker
sdk/         the JavaScript and Python host SDKs
adapters/    agent adapters for terminal coding agents
editors/     the VS Code and Zed extensions
skills/      the coding-agent Skill that `jevscript setup` installs
examples/    example programs and the Chief of Staff host
cookbooks/   runnable cookbook translations
docs/        guides and references
```

## License

Apache-2.0. See [LICENSE](LICENSE).
