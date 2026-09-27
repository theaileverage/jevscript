# CI release lanes per target

Targets other than the host are proven only by GitHub Actions.

## Sub-features

- `ci.yml` job `package-smoke`: Linux x64 build, stage, pack and smoke on every
  push and PR.
- `ci.yml` job `package-bundle-roundtrip`: downloads the representative
  Linux `package-bundle` that `package-smoke` uploaded through
  `.github/actions/upload-package-bundle`, the action `release.yml` uses, and
  re-verifies it against the uploaded `SHA256SUMS` digest.
- `ci.yml` job `windows-package-smoke`: the same on `windows-2022`, so the
  MSVC build and its path scan run before a tag exists.
- `release.yml` (manual, on a `v*` tag): all five targets build, stage and
  wheel; one npm bundle; install smoke on each target with Python 3.10, 3.12
  and 3.14.

## How to get to it (user POV)

Open the PR's checks, or the staging run for a tag.

## Driving it with gh

```sh
gh run list --workflow ci.yml --branch <branch> --limit 1
gh run view <run-id> --log --job <job-id> | grep -E 'validated|embeds|smoke passed|Error'
```

## Gotchas

- `release.yml` runs the workflow and scripts as they exist at the tag, and
  release tags cannot move, so a failure there costs a version. Get the
  matching PR lane green first.
- A skipped job is not a pass.
- `actions/upload-artifact` skips a search path whose own name starts with a
  dot unless `include-hidden-files` is true, and only warns by default, so
  v0.1.1's `.release-bundle/` upload succeeded with nothing in it. The
  `package-bundle-roundtrip` job and `if-no-files-found: error` exist for
  that; read the upload step for `Artifact package-bundle has been
  successfully uploaded`, not just the job's colour.
