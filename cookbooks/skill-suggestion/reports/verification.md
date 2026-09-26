# Verification report

Updated: 2026-09-26

## Result

The 157-skill licensed subset compiles and the full execution path contains
exactly two compiler request groups. Four deterministic cases ran through the
actual Python SDK and Rust runtime against a localhost scripted TypeSafe-shaped
endpoint. A freshly recorded Apple Notes run replayed with the endpoint shut
down and produced identical outputs. Nine cookbook unit tests passed.

A separate 4-case smoke benchmark made real TypeSafe and OpenRouter calls on
2026-09-21 with the former 182-skill roster. Its evidence is recorded in
`live-smoke-2026-09-21.md` and the adjacent JSONL file. It does not measure
the licensed subset and is not the upstream 488-row benchmark.

The deterministic policy is 39 lines of `.jev`. For context, the official
page's stage 3, stage 4, and suggestion-wrapper Python blocks total 172 lines,
including cache/timing/demo plumbing. This repository keeps the 302-line host
runner, 179-line benchmark harness, and 140-line OpenRouter adapter in Python
because they own process, dataset, and measured-agent concerns rather than the
decision policy.

## Evidence matrix

| Evidence | Classification | Result |
| --- | --- | --- |
| TypeSafe published benchmark | upstream published/static | 488 rows: 315 covered, 173 uncovered, 171 distinct covered skills |
| Baseline | upstream published/static | wrong load 16.8%; needless load 9.8% |
| TypeSafe Python suggestion | upstream published/static | wrong load 7.3%; needless load 4.0% |
| Oracle ceiling | upstream published/static | wrong load 2.5%; needless load 1.2% |
| Changed covered rows | upstream published/static | 37 fixed, 7 broken, of 315 |
| Licensed roster reconstruction | local static/provenance | 157 skills, 29 categories, 14,168 prompt characters, average 54, max 60 |
| Four focused cases | fixture/scripted | Apple Notes and pptx-author selected; Mastodon and prose-only abstained |
| Request groups | fixture/scripted recording | full path 2 groups with 4 questions each; early gate 1 group |
| Replay | local recorded/replay | identical Apple Notes outputs, zero endpoint calls during replay |
| Live Jev | historical live local smoke, former roster | 4 cases, 7 calls: both covered skills selected and both uncovered cases abstained |
| DeepSeek V4.1 Flash agent | historical live local smoke, former roster | 12 turns: every arm scored 0% wrong first load and 0% needless load on 2 covered plus 2 uncovered rows |
| Live 488-row agent benchmark | live | unavailable: source dataset is not linked for download and run would cost up to 976 Jev plus 1,464 agent calls |

## Commands run

```sh
cargo build --workspace
target/debug/jevscript check cookbooks/skill-suggestion/skill_suggestion.jev
target/debug/jevscript compile cookbooks/skill-suggestion/skill_suggestion.jev
python3 cookbooks/skill-suggestion/runner.py verify
python3 -m unittest discover cookbooks/skill-suggestion -p 'test_*.py' -v
python3 -m pytest sdk/python
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --workspace
(cd sdk/js && pnpm test && pnpm run typecheck && pnpm run build)
(cd adapters && pnpm install --frozen-lockfile && pnpm test && pnpm run typecheck)
```

The 2026-09-21 provider command and its results remain in the dated live report.
The checks above describe the current licensed subset; no live provider call
was made for this cleanup.

## Publishable-tree license scan

`git archive HEAD` was scanned as the source tree that could be published. Its
428 text files contained zero 80-character body fragments or full long
descriptions from the 25 excluded source entries. The archived roster and
license manifest each contain exactly 157 names, all declared MIT or
Apache-2.0; none of the 25 excluded names is present in either artifact.
The comparison uses the pre-filter roster at local commit `cd47557`:

```sh
python3 - <<'PY'
import io, json, subprocess, tarfile
prefix = 'cookbooks/skill-suggestion/data/'
old = json.loads(subprocess.check_output(['git', 'show', 'cd47557:' + prefix + 'hermes_roster.json']))
old_licenses = json.loads(subprocess.check_output(['git', 'show', 'cd47557:' + prefix + 'hermes_roster.licenses.json']))
excluded = [row for row in old if old_licenses[row['name']] not in {'MIT', 'Apache-2.0'}]
tar = tarfile.open(fileobj=io.BytesIO(subprocess.check_output(['git', 'archive', '--format=tar', 'HEAD'])))
texts = {}
for member in tar:
    if member.isfile():
        try:
            texts[member.name] = tar.extractfile(member).read().decode('utf-8')
        except UnicodeDecodeError:
            pass
roster = json.loads(texts[prefix + 'hermes_roster.json'])
licenses = json.loads(texts[prefix + 'hermes_roster.licenses.json'])
assert len(roster) == len(licenses) == 157
assert {row['name'] for row in roster} == set(licenses)
assert set(licenses.values()) <= {'MIT', 'Apache-2.0'}
assert not {row['name'] for row in excluded} & set(licenses)
hits = []
for path, text in texts.items():
    for row in excluded:
        body = row['body']
        chunks = (body[i:i + 80] for i in range(0, max(len(body) - 79, 1), 40))
        fragments = [chunk for chunk in chunks if len(chunk) == 80]
        if len(row['description_full']) >= 60:
            fragments.append(row['description_full'])
        if any(chunk in text or json.dumps(chunk, ensure_ascii=False)[1:-1] in text for chunk in fragments):
            hits.append((path, row['name']))
assert not hits, hits
print(f'{len(texts)} text files, {len(excluded)} excluded entries, zero copied fragments')
PY
```

## Deterministic case results

| Case | Gate | Shortlist | Suggestion | Requests |
| --- | ---: | --- | --- | ---: |
| Apple Notes | 0.85 | apple-notes, google-workspace, concept-diagrams | apple-notes | 2 |
| PowerPoint authoring | 0.8733 | pptx-author, claude-design, chroma | pptx-author | 2 |
| Mastodon no exact skill | 0.8333 | xurl, google-workspace, openhands | none (fit gate) | 2 |
| Prose-only monad explanation | 0.05 | empty | none (wide gate) | 1 |

## Request-state proof

- Request 1 state keys: `request`, `wide_roster`.
- Every wide record has only `name`, `description`, `label`.
- Request 2 state keys: `request`, `detailed`.
- Every detailed record has only `name`, `detail`.
- Compiled main-task request-group ids: `[0, 1]`.

## Limitations

The scripted client is deterministic fixture evidence, not provider-quality
evidence. Upstream benchmark figures remain attributed to TypeSafe's Python
run. The direct TypeSafe `hermes_roster.json` file was named but not exposed;
the checked-in artifact is a reproducibly filtered subset of the dated Hermes
commit and is rejected unless its local count, license, prompt-length, and
checksum invariants match. There is no current live-provider accuracy result.
