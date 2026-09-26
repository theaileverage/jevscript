# Jevscript CLI adapters

`jevscript run` never invents a binding. Each declared capability must be
selected explicitly with either `--bind NAME=COMMAND` or the visibly logged
demonstration-only `--stub NAME`.

A `--bind` command stays open for the run and speaks one JSON object per line.
The CLI sends:

```json
{"operation":"call","capability":"tree","verb":"diff","args":{"positional":[],"named":{}}}
{"operation":"observe","capability":"claude","handle":{"$jev":"handle","capability":"claude","id":"dev-1"}}
```

The subprocess answers with exactly one of:

```json
{"result":{"files":["src/lib.rs"]}}
{"observation":{"status":"exited","last_message":"done","tail":"","exit_code":0}}
{"error":{"message":"temporarily unavailable","retryable":true}}
```

Stdout is protocol-only; adapter diagnostics belong on stderr. Malformed JSON,
a missing result field, or unexpected process exit becomes `adapter_error`.
The CLI closes and reaps adapter processes when the run ends.

`run --redact --record run.jsonl` also writes the sensitive owner-only replay
companion `run.jsonl.replay.jsonl` and prints its path. Keep both files for
replay; `jevscript replay run.jsonl` needs no source, profile file, adapter, API
key, or model call. Recording destinations must be new: neither the primary nor
its companion is appended to or overwritten.
