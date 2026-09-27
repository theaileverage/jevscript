# jevscript (npm)

The JavaScript SDK spawns `jevscript serve` and implements the section 11.2
host surface over JSON-RPC stdio.

Install with `npm install jevscript` in a Node 22+ project. The package includes
the release-matched Rust CLI; no Cargo or install-time download is needed. Run
`npx jevscript --version` or `npx jevscript check node_modules/jevscript/examples/inbox_triage.jev`.
For a user-level command, use `npm install -g jevscript` and ensure npm's global
bin directory is on PATH. See the [package release guide](../../docs/package-release.md)
for supported platforms, OS floors, and package smoke requirements.
An unsupported platform reports a clear error. Offline installation works from
the complete saved `.tgz` with `npm install --offline ./jevscript-0.1.2.tgz`.
`load({ bin })` and `JEVSCRIPT_BIN` are explicit host overrides; the usual SDK
path uses the binary in this package after checking its digest.

Run `npx jevscript setup --agent codex` or `--agent claude-code` from a
project to install the bundled coding-agent Skill offline. Repeat `--agent`
for both, add `--global` for user scope, or `--copy` for separate directories
instead of links. Setup runs only when invoked; npm install has no postinstall.

```ts
import { load } from 'jevscript'

const program = await load('examples/fix_issue.jev', { paths: ['./lib'] })
const run = program.task('main').start({
  inputs: { issue },
  bind: { claude, tree, me: person },
  profiles: './profiles.json',
})
for await (const pause of run) {
  if (pause.kind === 'confirm') await run.resume({ answer: 'yes' })
}
await program.close()
```

Adapter `call` and `observe` callbacks receive the bound capability identity as
their last argument. Throw an error with `retryable = true` to surface a
retryable `adapter_error` pause. Events are delivered as the runtime appends
them, including capability calls made before the next pause returns.

A program's `log` lines (spec section 5.8) reach the host as typed
`LogEvent`s: `{ level, message, fields, task, source, run_id, seq }`. Pass
`onLog` to route them as they are written, or iterate `run.logs()`:

```ts
const run = program.task('main').start({
  inputs: { issue },
  bind: { claude, tree, me: person },
  onLog: (line) => console[line.level](line.message, line.fields),
})
const answers = await program.judgment('triage').run(state, { onLog: console.log })
```

A replay checks the recorded lines without emitting them again, so `onLog` sees
only lines written live. `logEvent(event)` reads a line off any recording event.

With `redact: true`, retain the sensitive private companion
`<recording>.replay.jsonl` beside the redacted primary file; both are required
for replay. `record` and `replay` are mutually exclusive, and recording paths
must be new; the runtime never appends to or overwrites either artifact.

## Subprocess agent adapters

Use the same agent executable as `jevscript run --bind` with
`subprocessAgent` (spec sections 9.1 and 11.6):

```ts
import { load, subprocessAgent } from 'jevscript'

const agent = subprocessAgent('jevscript-adapter-codex', ['--backend', 'tmux'])
try {
  const program = await load('program.jev')
  try {
    const run = program.task('main').start({ bind: { dev: agent } })
    for await (const pause of run) { /* handle pauses */ }
  } finally { await program.close() }
} finally { await agent.close() }
```

The helper sends one JSONL request at a time, forwards call and observation
results, and preserves the adapter's `retryable` error flag. Pass an argv array;
no shell interprets it. Closing the helper reaps its child process and leaves
reattachable agent panes alone.
