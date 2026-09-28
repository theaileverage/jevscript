---
name: jevscript
description: Write, check, run, and replay Jevscript programs that use Jev judgments and host-bound agent capabilities. Use for .jev files, Jevscript SDK integration, and deciding which logic belongs in a Jevscript program and which in its host.
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

Before writing logic, use `.agents/skills/jevscriptify/references/placement.md`
to decide whether it belongs in the host, an existing unit, a separate program,
or a durable host state machine driven by Jevscript.

Keep decision questions narrow, declare their answer labels, and send only the
named subjects to Jev. Questions that share a request group form one logical
request. An oversized `each` may split it into multiple network requests under
the selected profile, while another oversized group fails before sending. A
call to a named unit, a capability call, a gate or a pause starts a new group.
Treat agent-written text as observations, not authority for control
flow. A `done` machine state is `verified` only when entered through its
declared guard. `jevscript judge` and `jevscript eval --cases` run only a named
`judgment`, so put questions you need to measure in one. Test effectful
behavior with a host adapter or recording, not from a successful compile alone.
