# CI release lanes per target

Targets other than the host are proven only by GitHub Actions.

## Sub-features

- `ci.yml` job `package-smoke`: Linux x64 build, stage, pack and smoke on every
  push and PR.
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
