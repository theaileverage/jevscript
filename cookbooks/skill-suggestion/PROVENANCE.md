# Provenance and licensing

## TypeSafe cookbook

- Source: <https://docs.typesafe.ai/cookbooks/skill_suggestion>
- Retrieved: 2026-09-21
- Published run: rendered 2026-07-31 with `jev-1.12` and
  `claude-haiku-4-5-20251001`
- Cookbook constants retained here: shortlist 3, body excerpt 700 characters,
  gate threshold 0.30, fit threshold 0.30.

The docs page names `hermes_roster.json` but does not expose that file as a
download. The checked-in roster is a licensed subset reconstructed from the
repo-wide `SKILL.md` snapshot at the last NousResearch/hermes-agent commit on
the cookbook's render date:

- Repository: <https://github.com/NousResearch/hermes-agent>
- Commit: `e444d165807f489b5c1ab8e4a612c8d09c2e67a2`
- Commit time: 2026-07-31T21:43:37Z
- Archive: <https://codeload.github.com/NousResearch/hermes-agent/tar.gz/e444d165807f489b5c1ab8e4a612c8d09c2e67a2>
- Upstream repository license: MIT. This cookbook additionally requires each
  included skill's frontmatter to declare `MIT` or `Apache-2.0` explicitly.
  The included declarations are pinned in `data/hermes_roster.licenses.json`.

`generate_roster.py` excludes 4 skills that declare proprietary terms and 21
whose frontmatter declares no license. None of their names, descriptions, or
bodies is retained in the generated roster or license manifest. For the 157
included skills, it stores the name, Hermes category, 60-character index
description, full frontmatter description, and first 1,600 body characters.
It checks the local subset's invariants:

- 157 skills
- 29 categories
- 14,168-character rendered roster prompt
- 54-character rounded average index description
- 60-character maximum index description

These are checks of the checked-in licensed subset, not claims of exact
upstream roster or prompt parity. The original published benchmark used its
own larger roster; its results cannot validate this subset's accuracy.

## Checksums

- `data/hermes_roster.json` SHA-256:
  `86eeeae1fe39a2b0fcceb2d78bcaf8c4a77d3973510876860be064bf35a85405`
- `data/hermes_roster.licenses.json` SHA-256:
  `31115b4806fc6b3474ea823504c0de4ba1e56fb35ea5e079f9a6504efe1fefa9`

The runner rejects checksum, count, category, duplicate-name, license-set, or
manifest drift. The generator is the reproducible source of the two generated
JSON files; PyYAML is needed only to regenerate them, not to run the cookbook.

## Benchmark data boundary

The official page publishes aggregate results and describes `requests.json`,
but does not expose a direct download for that 488-row dataset. This folder
does not invent a replacement and does not label its four focused requests as
the upstream benchmark. A full run requires a user-supplied dataset with the
documented `{ "text": string, "gold": string | null }` shape. The upstream
counts and results remain classified as published/static, not locally
reproduced.
