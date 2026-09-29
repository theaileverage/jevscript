# Publish a staged release

`publish.yml` uploads the exact bytes of a reviewed `package-bundle` and
rebuilds nothing. Its `verify` job binds the staging run to the tag's commit and
rechecks every digest. Then its `pypi` job uploads the wheels and its `npm` job
publishes the tarball, in that order. Stage and review the bundle with
[Release](release.md) first.

## Know the two registry identities

npm and PyPI are separate projects with separate trust settings. Each can hold
a different latest version, and each registry is the record of what it
publishes.

| | npm | PyPI |
| --- | --- | --- |
| Name | `@theaileverage/jevscript` | `jevscript` |
| Artifact | One tarball with all five target binaries | Five wheels, one per target, and no sdist |
| Uploaded by | `publish.yml` job `npm`, through npm trusted publishing | `publish.yml` job `pypi`, through PyPI trusted publishing |
| Environment | `npm`, with variable `NPM_EXPECTED_OWNER` | `pypi` |
| Byte check | Tarball SHA-512 against `dist.integrity` | Wheel SHA-256 against the PyPI file digests |

The unscoped npm name `jevscript` is a separate, pending request. Nothing here
publishes to it.

## Read the current registry state

Checked on 2026-09-28, the registries hold different versions:

- npm `@theaileverage/jevscript` holds 0.1.3 only. Its tarball records commit
  `1410d7f`, the `v0.1.3` tag.
- PyPI `jevscript` holds 0.1.2 only, five wheels from commit `16a42b5`, the
  `v0.1.2` tag. PyPI has no 0.1.3.
- The [v0.1.3 GitHub Release](https://github.com/theaileverage/jevscript/releases/tag/v0.1.3)
  attaches the 0.1.3 tarball, five 0.1.3 wheels, and `SHA256SUMS` from staging
  run 36332522495. Those wheels are not on PyPI.
- `publish.yml` has run only for `v0.1.2`.

`publish-registry-versions` reads the current state from both registries.

```bash {"cwd":"..","excludeFromRunAll":"true","interpreter":"bash","name":"publish-registry-versions","tag":"network-read"}
echo "npm @theaileverage/jevscript latest: $(npm view @theaileverage/jevscript version)"
echo "npm versions: $(npm view @theaileverage/jevscript versions --json | tr -d ' \n')"
curl -fsS https://pypi.org/pypi/jevscript/json | python3 -c 'import json, sys; data = json.load(sys.stdin); print("PyPI jevscript latest:", data["info"]["version"]); print("PyPI versions:", ", ".join(sorted(data["releases"])))'
```

## Confirm the publishing settings

Confirm these settings in the GitHub, npm, and PyPI web interfaces before you
dispatch. `env-github` in [Secrets and environment](secrets-and-environment.md)
reads the GitHub side.

1. The `npm` and `pypi` GitHub environments require a reviewer.
2. The PyPI project `jevscript` trusts owner `theaileverage`, repository
   `jevscript`, workflow `publish.yml`, and environment `pypi`.
3. The npm package `@theaileverage/jevscript` trusts repository
   `theaileverage/jevscript`, workflow `publish.yml`, and environment `npm`.
4. `NPM_EXPECTED_OWNER` in the `npm` environment names the npm account that
   must appear in `npm owner ls @theaileverage/jevscript`.
5. The `npm` environment no longer holds `NPM_BOOTSTRAP_TOKEN`.

## Compare the bundle with the registries

Both cells read the bundle that `release-bundle-download` saved and change
nothing. Before publication they tell you what each registry already holds.
After publication they must pass.

`publish-check-pypi` fails when PyPI holds a file that is not in the bundle or a
wheel whose SHA-256 differs. Missing wheels are allowed. Add `--require-all`
after publication, as `publish-check-pypi-complete` does.

```bash {"cwd":"..","excludeFromRunAll":"true","interpreter":"bash","name":"publish-check-pypi","tag":"network-read"}
test -n "${STAGING_RUN_ID:-}" || { echo "Set STAGING_RUN_ID to the release.yml run ID" >&2; exit 1; }
python3 scripts/check_pypi_wheels.py ".release-tmp/staging-$STAGING_RUN_ID"
```

```bash {"cwd":"..","excludeFromRunAll":"true","interpreter":"bash","name":"publish-check-pypi-complete","tag":"network-read"}
test -n "${STAGING_RUN_ID:-}" || { echo "Set STAGING_RUN_ID to the release.yml run ID" >&2; exit 1; }
python3 scripts/check_pypi_wheels.py ".release-tmp/staging-$STAGING_RUN_ID" --require-all
```

`publish-check-npm` compares the tarball's SHA-512 with the `dist.integrity`
npm serves for that version, then lists the package's npm owners. It fails when
npm does not hold the version or holds different bytes. It reads the registry
with `npm view` and does not call `scripts/publish_npm.py`, which publishes when
the version is missing.

```bash {"cwd":"..","excludeFromRunAll":"true","interpreter":"bash","name":"publish-check-npm","tag":"network-read"}
test -n "${STAGING_RUN_ID:-}" || { echo "Set STAGING_RUN_ID to the release.yml run ID" >&2; exit 1; }
tarballs=(".release-tmp/staging-$STAGING_RUN_ID"/theaileverage-jevscript-*.tgz)
tarball=${tarballs[0]}
version=$(tar -xOzf "$tarball" package/package.json | python3 -c 'import json, sys; print(json.load(sys.stdin)["version"])')
bundle=$(python3 -c 'import base64, hashlib, sys; print("sha512-" + base64.b64encode(hashlib.sha512(open(sys.argv[1], "rb").read()).digest()).decode())' "$tarball")
registry=$(npm view "@theaileverage/jevscript@$version" dist.integrity)
echo "bundle:   $bundle"
echo "registry: ${registry:-missing}"
test "$registry" = "$bundle" || { echo "npm does not hold the reviewed $version tarball" >&2; exit 1; }
npm owner ls @theaileverage/jevscript
```

## Dispatch the publish workflow

`publish-dispatch` starts `publish.yml` on the release tag. It takes the tag,
the staging run ID, and the SHA-256 of `SHA256SUMS` that `release-bundle-verify`
printed. It always passes `npm_bootstrap=false`. Each registry job then waits
for a reviewer in its protected environment.

```bash {"cwd":"..","excludeFromRunAll":"true","interpreter":"bash","name":"publish-dispatch","tag":"external-mutation"}
test -n "${RELEASE_TAG:-}" || { echo "Set RELEASE_TAG, for example v0.1.4" >&2; exit 1; }
test -n "${STAGING_RUN_ID:-}" || { echo "Set STAGING_RUN_ID to the release.yml run ID" >&2; exit 1; }
test -n "${SUMS_SHA256:-}" || { echo "Set SUMS_SHA256 to the reviewed SHA-256 of SHA256SUMS" >&2; exit 1; }
gh workflow run publish.yml --repo theaileverage/jevscript --ref "$RELEASE_TAG" \
  -f staging_run_id="$STAGING_RUN_ID" -f sums_sha256="$SUMS_SHA256" -f npm_bootstrap=false | cat
```

The npm bootstrap path (`npm_bootstrap: true` with a short-lived
`NPM_BOOTSTRAP_TOKEN`) was for the first npm publication. `publish.yml` accepts
it only for 0.1.3. No cell runs it.

`publish-runs` lists the publish runs for the tag.

```bash {"cwd":"..","excludeFromRunAll":"true","interpreter":"bash","name":"publish-runs","tag":"network-read"}
test -n "${RELEASE_TAG:-}" || { echo "Set RELEASE_TAG, for example v0.1.4" >&2; exit 1; }
gh run list --repo theaileverage/jevscript --workflow publish.yml --branch "$RELEASE_TAG" --limit 5 | cat
```

## Verify the published release

1. Run `publish-check-pypi-complete` and `publish-check-npm`. Both must pass.
2. Run `publish-install-smoke`. It installs the published version from each
   registry into `.release-tmp/registry-smoke-<version>` and prints each
   installed CLI's version.
3. Update the published versions in the install table of the root
   [README](../README.md#install).

```bash {"cwd":"..","excludeFromRunAll":"true","interpreter":"bash","name":"publish-install-smoke","tag":"local-mutation"}
test -n "${RELEASE_TAG:-}" || { echo "Set RELEASE_TAG, for example v0.1.4" >&2; exit 1; }
version=${RELEASE_TAG#v}
scratch=".release-tmp/registry-smoke-$version"
mkdir -p "$scratch/npm"
(cd "$scratch/npm" && npm init -y >/dev/null && npm install "@theaileverage/jevscript@$version" && npx --no-install jevscript --version)
python3 -m venv "$scratch/venv"
"$scratch/venv/bin/python" -m pip install "jevscript==$version"
"$scratch/venv/bin/jevscript" --version
```

If one registry accepted the version and the other did not, follow
[Incidents and rollback](incidents-and-rollback.md#one-registry-holds-the-version).
