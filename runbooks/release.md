# Stage a release

A release is three records: an immutable tag on a commit merged into `main`, a
successful `release.yml` run on that tag, and the `package-bundle` artifact
that run uploads. Publishing the bundle is a separate step in
[Publication](publication.md). The
[package release guide](../docs/package-release.md#staging-and-publication-boundary)
is the contract for every step here.

`release.yml` runs the workflows and scripts as they exist at the tag, and a
release tag never moves. A staging failure therefore costs a version number.
Get CI's `package-smoke`, `package-bundle-roundtrip`, and
`windows-package-smoke` jobs green on the release pull request first.

## Known hold on the next release

Checked on 2026-09-28: `skills/jevscript/SKILL.md` on `main` tells readers to
open `.agents/skills/jevscriptify/references/placement.md`. The packages ship
`SKILL.md` alone, so that path is missing in an installed project. Resolve it
before you stage the next release. `release-skill-paths` below shows the
repository paths the Skill names.

## Prepare the release commit

A version bump changes every version site in one pull request:

1. `Cargo.toml`: `workspace.package.version` and the `=` pin of each internal
   crate under `[workspace.dependencies]`.
2. `Cargo.lock`: `cargo build` rewrites the workspace crates' versions.
3. `sdk/js/package.json`: `version`.
4. `sdk/python/pyproject.toml`: `project.version`.
5. `sdk/python/src/jevscript/__init__.py`: `__version__`.
6. The artifact file names in `.github/workflows/ci.yml` (two smoke steps) and
   `.github/workflows/release.yml` (the `install-smoke` step).

Leave `test "$version" = 0.1.3` in `publish.yml` alone. It limits the one-time
npm bootstrap to 0.1.3.

`release-version-sites` prints each site's version and fails when one differs
from the Cargo workspace version.

```bash {"cwd":"..","interpreter":"bash","name":"release-version-sites","tag":"inspection"}
python3 - <<'PY'
import json
import re
import tomllib
from pathlib import Path

cargo = tomllib.loads(Path("Cargo.toml").read_text())
version = cargo["workspace"]["package"]["version"]
sites = [
    ("sdk/js/package.json", json.loads(Path("sdk/js/package.json").read_text())["version"]),
    ("sdk/python/pyproject.toml", tomllib.loads(Path("sdk/python/pyproject.toml").read_text())["project"]["version"]),
    ("sdk/python/src/jevscript/__init__.py",
     re.search(r'^__version__ = "([^"]+)"', Path("sdk/python/src/jevscript/__init__.py").read_text(), re.M).group(1)),
]
for name, dependency in cargo["workspace"]["dependencies"].items():
    if isinstance(dependency, dict) and "path" in dependency:
        sites.append((f"Cargo.toml {name} pin", dependency["version"].lstrip("=")))
for package in tomllib.loads(Path("Cargo.lock").read_text())["package"]:
    if package["name"].startswith("jevscript-") and "source" not in package:
        sites.append((f"Cargo.lock {package['name']}", package["version"]))
for workflow in ("ci.yml", "release.yml"):
    text = Path(".github/workflows", workflow).read_text()
    for found in sorted(set(re.findall(r"jevscript-(\d+\.\d+\.\d+)(?:\.tgz|-py3-none)", text))):
        sites.append((f".github/workflows/{workflow} artifact names", found))
print(f"Cargo.toml workspace version: {version}")
wrong = [(name, found) for name, found in sites if found != version]
for name, found in sites:
    print(f"{'ok ' if found == version else 'BAD'} {name}: {found}")
if wrong:
    raise SystemExit(f"{len(wrong)} version sites differ from {version}")
PY
```

The packages ship `skills/jevscript/SKILL.md` without the rest of the
repository. Read each line this cell prints and confirm the path it names is
either marked as a source-checkout path or shipped with the Skill.

```bash {"cwd":"..","interpreter":"bash","name":"release-skill-paths","tag":"inspection"}
grep -nE '(\.agents|\.claude|docs|examples|spec|crates|scripts|sdk)/' skills/jevscript/SKILL.md || echo "SKILL.md names no repository paths"
```

## Tag the merged commit

Merge the release pull request into protected `main` first. Confirm the `v*`
tag protection that prevents moving and deleting tags is still in place.

`release-fetch` updates your `origin/*` branches and tags. It changes nothing
on GitHub.

```bash {"cwd":"..","excludeFromRunAll":"true","interpreter":"bash","name":"release-fetch","tag":"network-read"}
git fetch origin main --tags
```

Check out the merged commit, then create the annotated tag locally. The cell
refuses a tag that does not match the Cargo workspace version or a commit that
is not on `origin/main`.

```bash {"cwd":"..","excludeFromRunAll":"true","interpreter":"bash","name":"release-tag-create","tag":"local-mutation"}
test -n "${RELEASE_TAG:-}" || { echo "Set RELEASE_TAG, for example v0.1.4" >&2; exit 1; }
test -n "${RELEASE_COMMIT:-}" || { echo "Set RELEASE_COMMIT to the merged release commit" >&2; exit 1; }
version=$(python3 -c 'import tomllib; print(tomllib.load(open("Cargo.toml", "rb"))["workspace"]["package"]["version"])')
test "$RELEASE_TAG" = "v$version" || { echo "RELEASE_TAG must be v$version" >&2; exit 1; }
test "$(git rev-parse HEAD)" = "$(git rev-parse "$RELEASE_COMMIT^{commit}")" || { echo "check out $RELEASE_COMMIT first" >&2; exit 1; }
git merge-base --is-ancestor "$RELEASE_COMMIT" origin/main || { echo "$RELEASE_COMMIT is not on origin/main" >&2; exit 1; }
git tag -a "$RELEASE_TAG" -m "Jevscript $RELEASE_TAG" "$RELEASE_COMMIT"
```

`release-tag-check` runs the same check as the `source-ref` job of
`release.yml`: the tag names the Cargo version, points at the checkout, and is
merged into `origin/main`.

```bash {"cwd":"..","excludeFromRunAll":"true","interpreter":"bash","name":"release-tag-check","tag":"inspection"}
test -n "${RELEASE_TAG:-}" || { echo "Set RELEASE_TAG, for example v0.1.4" >&2; exit 1; }
python3 scripts/check_release_ref.py --commit "$(git rev-parse HEAD)" --tag "$RELEASE_TAG" --ref-type tag
```

Pushing the tag is permanent. The tag cannot move or be deleted afterwards.

```bash {"cwd":"..","excludeFromRunAll":"true","interpreter":"bash","name":"release-tag-push","tag":"external-mutation"}
test -n "${RELEASE_TAG:-}" || { echo "Set RELEASE_TAG, for example v0.1.4" >&2; exit 1; }
git push origin "refs/tags/$RELEASE_TAG"
```

## Stage the packages

`release-stage-dispatch` starts `release.yml` on the tag. The run builds all
five targets, assembles the npm tarball, installs both package types on every
target with Python 3.10, 3.12, and 3.14, and uploads `package-bundle`.

```bash {"cwd":"..","excludeFromRunAll":"true","interpreter":"bash","name":"release-stage-dispatch","tag":"external-mutation"}
test -n "${RELEASE_TAG:-}" || { echo "Set RELEASE_TAG, for example v0.1.4" >&2; exit 1; }
gh workflow run release.yml --repo theaileverage/jevscript --ref "$RELEASE_TAG" | cat
```

`release-stage-runs` lists the staging runs for the tag. Take the run ID from
its output. Every job must pass, and a skipped job is not a pass. In the
`npm-bundle` job, find `Artifact package-bundle has been successfully uploaded`
in the upload step's log.

```bash {"cwd":"..","excludeFromRunAll":"true","interpreter":"bash","name":"release-stage-runs","tag":"network-read"}
test -n "${RELEASE_TAG:-}" || { echo "Set RELEASE_TAG, for example v0.1.4" >&2; exit 1; }
gh run list --repo theaileverage/jevscript --workflow release.yml --branch "$RELEASE_TAG" --limit 5 | cat
```

## Review the bundle

`release-bundle-download` saves the run's `package-bundle` to
`.release-tmp/staging-<run ID>`. GitHub deletes workflow artifacts after the
repository's retention period, and `publish.yml` can publish only a bundle that
still exists.

```bash {"cwd":"..","excludeFromRunAll":"true","interpreter":"bash","name":"release-bundle-download","tag":"local-mutation"}
test -n "${STAGING_RUN_ID:-}" || { echo "Set STAGING_RUN_ID to the release.yml run ID" >&2; exit 1; }
bundle=".release-tmp/staging-$STAGING_RUN_ID"
test ! -e "$bundle" || { echo "$bundle exists. Verify it with release-bundle-verify, or move it aside first." >&2; exit 1; }
gh run download "$STAGING_RUN_ID" --repo theaileverage/jevscript --name package-bundle --dir "$bundle" | cat
```

With the tag checked out, `release-bundle-verify` checks the bundle with
`scripts/verify_release_bundle.py`: exactly one tarball and five wheels, every
digest in `SHA256SUMS`, one version, the tag's commit, and the same CLI bytes
per target in npm and PyPI. It then lists the archive contents and prints the
SHA-256 of `SHA256SUMS` itself. Record that digest. `publish.yml` needs it as
`sums_sha256`.

```bash {"cwd":"..","excludeFromRunAll":"true","interpreter":"bash","name":"release-bundle-verify","tag":"inspection"}
test -n "${RELEASE_TAG:-}" || { echo "Set RELEASE_TAG, for example v0.1.4" >&2; exit 1; }
test -n "${STAGING_RUN_ID:-}" || { echo "Set STAGING_RUN_ID to the release.yml run ID" >&2; exit 1; }
bundle=".release-tmp/staging-$STAGING_RUN_ID"
python3 scripts/verify_release_bundle.py "$bundle" --commit "$(git rev-parse "$RELEASE_TAG^{commit}")"
for archive in "$bundle"/theaileverage-jevscript-*.tgz; do tar -tzf "$archive"; done
for wheel in "$bundle"/wheelhouse/*.whl; do python3 -m zipfile -l "$wheel"; done
echo "SHA-256 of SHA256SUMS: $(shasum -a 256 "$bundle/SHA256SUMS" | cut -d ' ' -f 1)"
```

## Create the GitHub Release by hand

Neither workflow creates a GitHub Release. A maintainer creates one for the tag
in the GitHub web interface and attaches the npm tarball, the five wheels, and
`SHA256SUMS` from the reviewed bundle, with no other files. Then continue with
[Publication](publication.md).
