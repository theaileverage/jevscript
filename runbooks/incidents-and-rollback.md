# Handle a release incident

npm and PyPI keep each name and version forever, and release tags never move.
No published version can be rolled back or replaced. Every fix goes forward,
either by rerunning the same workflow on the same bytes or by releasing a new
patch version.

Never rebuild or overwrite an existing name and version, delete or move a tag,
or unpublish from npm. Those steps have no cell.

## Find what happened

`incident-runs` lists the recent staging and publish runs. `incident-run-log`
prints the failed steps of one run.

```bash {"cwd":"..","excludeFromRunAll":"true","interpreter":"bash","name":"incident-runs","tag":"network-read"}
for workflow in release.yml publish.yml; do
  echo "== $workflow"
  gh run list --repo theaileverage/jevscript --workflow "$workflow" --limit 10 | cat
done
```

```bash {"cwd":"..","excludeFromRunAll":"true","interpreter":"bash","name":"incident-run-log","tag":"network-read"}
test -n "${RUN_ID:-}" || { echo "Set RUN_ID to the workflow run ID" >&2; exit 1; }
gh run view "$RUN_ID" --repo theaileverage/jevscript --log-failed | cat
```

Run `publish-registry-versions` in [Publication](publication.md) to read what
each registry holds now.

## A staging run failed

If a runner or network failure stopped a job and the source is sound, rerun the
failed jobs of the same run. The rerun uses the same tag and source.

```bash {"cwd":"..","excludeFromRunAll":"true","interpreter":"bash","name":"incident-rerun-failed-jobs","tag":"external-mutation"}
test -n "${RUN_ID:-}" || { echo "Set RUN_ID to the failed release.yml run ID" >&2; exit 1; }
gh run rerun "$RUN_ID" --repo theaileverage/jevscript --failed | cat
```

If the source or the workflow at the tag is wrong, the tag cannot be fixed. Fix
`main`, bump to the next patch version, and stage again with
[Release](release.md).

## A publish run failed in its verify job

The `verify` job runs before any upload, so nothing reached a registry. Check
the tag, the staging run ID, and the `SHA256SUMS` digest, then dispatch again
with `publish-dispatch` in [Publication](publication.md).

## One registry holds the version

When only one registry accepted a version, treat it as an incident. The
package release guide describes the recovery:

1. Download and verify the bundle of the tag's staging run with
   `release-bundle-download` and `release-bundle-verify` in
   [Release](release.md). If GitHub has deleted the artifact, `publish.yml`
   cannot publish it, and the fix is a new patch version.
2. Compare each registry with the bundle: run `publish-check-pypi` and
   `publish-check-npm` in [Publication](publication.md).
3. If every published file matches the bundle, run `publish-dispatch` again
   with the same tag, staging run ID, and digest. `publish.yml` uploads only the
   missing files. It accepts files already published only after it matches
   their bytes and finds `NPM_EXPECTED_OWNER` among the npm owners.
4. If a published file differs from the bundle, stop. A byte mismatch needs a
   new patch version.
5. If another npm account controls a same-byte version, stop. Resolve the
   ownership before you call the release complete.

On 2026-09-28, 0.1.3 is on npm and not on PyPI. See
[the current registry state](publication.md#read-the-current-registry-state).

## Published bytes are wrong

Release a new patch version with the fix through [Release](release.md) and
[Publication](publication.md). Then a maintainer can steer users away from the
bad version by hand:

- npm: an owner runs `npm deprecate @theaileverage/jevscript@VERSION "REASON"`.
  The version stays installable when pinned.
- PyPI: an owner yanks the release in the `jevscript` project's release
  settings on pypi.org. Pinned installs still work, and unpinned installs skip
  it.

Say which version replaces it in the GitHub Release notes of both versions.
