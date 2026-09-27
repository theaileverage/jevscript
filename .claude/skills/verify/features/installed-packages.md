# Installed packages: CLI, SDKs, setup, integrity

`scripts/smoke_packages.py --npm <tgz> --wheel <whl>` audits both archives
(member allowlist, license, wheel tag, forbidden bytes, LF-only Skill and
examples, external supervisor branding in tracked source) and then acts as a
user would.

## Sub-features

- Offline `npm install` and a fresh venv `pip install` of the built artifacts.
- The installed `jevscript --version` and `check`, from both packages, with no
  repository binary on PATH.
- `jevscript setup --agent codex --agent claude-code` into scratch project and
  home directories, and the installed Skill bytes.
- `load` of `inbox_triage.jev` through each SDK, and a no-model task run.
- A damaged installed binary is refused (`integrity check`) by the command and
  the SDK.

## How to get to it (user POV)

`npm install jevscript` or `pip install jevscript`, then `jevscript ...` or
`import { load } from 'jevscript'` / `import jevscript`.

## Driving it with package-smoke.sh

Steps `stage-npm`, `stage-wheel`, `wheel`, `npm-build`, `npm-pack`, `smoke`.
Pass means `summary.txt` shows exit 0 for every step and the script prints
`PASS <target> <version>`.

## Gotchas

- The tarball and wheel carry only the host binary here; the release tarball
  carries all five, built on their own runners.
- Staging writes gitignored trees under `sdk/`; run cleanup afterwards.
