# Section 6.4a citation check

At HEAD `5bcb251e06bc5a7deaeea4ae73f7ea0f7c0a1b15`, this is a static documentation and spec check, separate from the live CLI scenario. It makes no claim about model quality.

- `spec/jevscript-language-specification.md`, section 6.4a: the runtime list must be nonempty and no longer than the model profile's `max_criteria_per_question`; an overlong list pauses with `pick_too_many`.
- `examples/chief-of-staff/src/cos/state/threads.py`, `Threads.candidates`: the host sorts same-scope threads and returns at most `CANDIDATE_LIMIT` entries, currently eight.
- `examples/chief-of-staff/jev/threads.jev`: `route_message` uses `pick among` over that supplied list. Its comment now attributes the profile cap and overlong-list error to section 6.4a, and candidate scoping and capping to this host.

The focused post-edit CLI test command was:

```sh
JEVSCRIPT_BIN="$PWD/target/debug/jevscript" uv run --project examples/chief-of-staff --frozen --offline --extra dev pytest examples/chief-of-staff/tests/test_bearings_cli.py::test_status_route_uses_cli_headline_and_normal_runs_replay -q
```

Result: `1 passed in 4.11s`. The earlier run of the same focused test passed in 4.11s before the comment edit. The separate live CLI commands, outbox, recording and episode snapshots, and six successful replay outputs are retained in `route-cli.log` and `route-evidence.json` in this directory.
