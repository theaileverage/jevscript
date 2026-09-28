# Secrets and environment variables

This page lists the variables each part of the repository reads and where the
release secrets live. It never holds a secret value, and no cell writes one.

## Runtime and CLI

The [CLI reference](../docs/cli.md#environment-variables) describes these
variables in full.

| Variable | Read by | Secret |
| --- | --- | --- |
| `TYPESAFE_API_KEY` | Every command and run that calls a model, such as `judge` and `eval`. `.env.example` is the template, and `.env` is gitignored. | Yes |
| `JEVSCRIPT_PROFILES` | The CLI, as a model profiles file layered over the bundled profiles. | No |
| `JEVSCRIPT_PATH` | The compiler, as colon-separated module roots for non-relative `use` paths. | No |
| `JEVSCRIPT_BIN` | Both SDKs, as the `jevscript` binary to start instead of the packaged one. | No |

## Packaging and release scripts

The scripts and workflows set these variables themselves. Set one by hand only
to run a single packaging step outside the scripts.

| Variable | Read by |
| --- | --- |
| `JEVSCRIPT_TARGET`, `JEVSCRIPT_PLATFORM_TAG`, `JEVSCRIPT_WHEEL_TAG` | `sdk/python/hatch_build.py`, which refuses to build a wheel without all three |
| `JEVSCRIPT_PACK_TARGETS` | `sdk/js/bin/verify-package.mjs`, the npm `prepack` check of the staged targets |
| `TARGETS`, `GITHUB_SHA`, `GITHUB_OUTPUT` | `scripts/assemble_release_bundle.sh` |
| `MACOSX_DEPLOYMENT_TARGET`, `CARGO_HOME`, `RUSTFLAGS` | `cargo`, as set by `scripts/build_release_cli.py` for the release build |
| `NPM_EXPECTED_OWNER` | `scripts/publish_npm.py`, as the npm account that must own the package |
| `NODE_AUTH_TOKEN` | `npm`, and `scripts/publish_npm.py --bootstrap`, which requires it |

## Runbook inputs

The runbook cells read these variables and stop when one they need is missing.
None of them is a secret.

| Variable | Meaning |
| --- | --- |
| `RELEASE_TAG` | The release tag, such as `v0.1.4` |
| `RELEASE_COMMIT` | The merged release commit on `main` |
| `STAGING_RUN_ID` | The ID of the `release.yml` run that built the bundle |
| `SUMS_SHA256` | The reviewed SHA-256 of that bundle's `SHA256SUMS` |
| `RUN_ID` | Any workflow run to inspect or rerun |
| `RUNME` | The `runme` binary that `scripts/check_runbooks.py` calls |

## GitHub environments and secrets

`publish.yml` runs its registry jobs in two protected environments, and each job
gets `id-token: write` so the registry can exchange a GitHub OIDC token.

- `pypi` holds no secret. PyPI trusts the workflow through its trusted publisher.
- `npm` holds the variable `NPM_EXPECTED_OWNER`. It held the secret
  `NPM_BOOTSTRAP_TOKEN` only for the first npm publication. The package release
  guide says to remove it once npm trusts the workflow.
- The `verify` job reads the staging run through the workflow's own
  `github.token`.

`env-github` lists each environment's protection rules and the names of its
secrets and variables. It never prints a value. It needs `gh auth login` with
admin access to the repository.

```bash {"cwd":"..","excludeFromRunAll":"true","interpreter":"bash","name":"env-github","tag":"network-read"}
repo=theaileverage/jevscript
gh api "repos/$repo/environments" --jq '.environments[] | "\(.name): \([.protection_rules[]?.type] | join(", "))"' | cat
echo "repository secrets: $(gh secret list --repo "$repo" --json name --jq '[.[].name] | join(", ")')"
for environment in npm pypi; do
  echo "$environment secrets: $(gh secret list --repo "$repo" --env "$environment" --json name --jq '[.[].name] | join(", ")')"
  echo "$environment variables: $(gh variable list --repo "$repo" --env "$environment" --json name --jq '[.[].name] | join(", ")')"
done
```

To set or remove a secret, use the repository's environment settings on
GitHub. Do not put a secret value in a runbook, a cell, or a command line,
where your shell history keeps it.

## Keep local secrets out of cells

When you run a cell from the repository root, Runme loads `.env.local` and
`.env` from the root into the cell's environment. A `TYPESAFE_API_KEY` in `.env`
therefore reaches every cell, and no runbook cell needs it. To keep those files
out of a cell, run it with `runme run --load-env=false NAME`. Runme also loads
`direnv` settings by default, and `--direnv=false` turns that off.

`env-local` reports which of the variables above are set in the current shell.
It prints the paths of the path variables and hides the value of every secret.

```bash {"cwd":"..","interpreter":"bash","name":"env-local","tag":"inspection"}
for name in TYPESAFE_API_KEY NODE_AUTH_TOKEN NPM_BOOTSTRAP_TOKEN; do
  if [ -n "${!name:-}" ]; then echo "$name: set (value hidden)"; else echo "$name: unset"; fi
done
for name in JEVSCRIPT_PROFILES JEVSCRIPT_PATH JEVSCRIPT_BIN NPM_EXPECTED_OWNER; do
  echo "$name: ${!name:-unset}"
done
for file in .env .env.local; do
  if [ -e "$file" ]; then echo "$file: present, and Runme loads it"; else echo "$file: absent"; fi
done
```
