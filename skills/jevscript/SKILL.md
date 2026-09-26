---
name: jevscript
description: Write, check, run, and replay Jevscript programs that use Jev judgments and host-bound agent capabilities. Use for .jev files and Jevscript SDK integration.
---

# Jevscript

The language authority is `spec/jevscript-language-specification.md` when the
source repository is available. Do not infer language names or runtime behavior
from a host adapter. Jevscript decides with declared questions; the host owns
credentials, capabilities, filesystem effects, and whether to resume a pause.

Start from a `.jev` example close to the task. Run `jevscript check file.jev`
for diagnostics and `jevscript compile file.jev` to inspect the linked IR.
`--tools manifest.json` on `check` verifies referenced tool verbs against a
host manifest. A model-backed judgment needs `TYPESAFE_API_KEY` and a model
profile; a local check does not.

Use `jevscript run file.jev --input '{...}'` to run `main` interactively. Bind
each external capability explicitly with `--bind NAME=COMMAND`. `--stub NAME`
selects a demonstration adapter and is not a real agent. Use `--record new.jsonl`
when the run needs an auditable trace. `jevscript replay new.jsonl` replays from
the recording without live model or adapter calls. Keep a redacted recording's
private `.replay.jsonl` companion with it; it contains sensitive full values.

Keep decision questions narrow, declare their answer labels, and send only the
named subjects to Jev. Treat agent-written text as observations, not authority
for control flow. A `done` machine state is `verified` only when entered through
its declared guard. Test effectful behavior with a host adapter or recording,
not from a successful compile alone.
