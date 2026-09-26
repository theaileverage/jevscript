# `agent` adapter: Claude Code in a tmux pane

Real. All five verbs of spec section 9.1 are implemented and tested: a unit
suite runs them against a fake tmux, a fixture transcript and a fake clock, and
an integration test runs them against a real tmux server with a shell script
standing in for `claude`. It has been smoke-tested against Claude Code 2.1 and
tmux 3.7.

## What an `agent` adapter is

An adapter is the only way a Jevscript program reaches the world (spec section
9). It lives in the host process, not in the runtime: the runtime never links
against tmux, a browser or an agent CLI, and instead asks the host over
JSON-RPC (`capability.call`, `capability.observe`, spec section 11.5).

The `agent` kind has exactly five verbs (spec section 9.1):


| Verb      | Written as                                | Returns                                       |
| --------- | ----------------------------------------- | --------------------------------------------- |
| `spawn`   | `claude.spawn in tree, prompt issue.body` | a handle                                      |
| `observe` | `dev.observe`                             | an observation record                         |
| `send`    | `dev.send "text"`                         | nothing                                       |
| `wait`    | `dev.wait idle, minutes 5`                | an observation; pauses the run with `waiting` |
| `stop`    | `dev.stop`                                | nothing                                       |
|           |                                           |                                               |


## Binding it

```ts
import { ClaudeCodeAdapter } from '@jevscript/adapter-claude-code'

const run = program.task('main').start({
  bind: { dev: new ClaudeCodeAdapter({ capability: 'dev', session: 'jevscript' }) },
})
```

Options, all optional:


| Option                              | Default                                | What it is                                                                                                                |
| ----------------------------------- | -------------------------------------- | ------------------------------------------------------------------------------------------------------------------------- |
| `capability`                        | `claude`                               | The name the program bound the adapter under. The SDK does not tell an adapter its name, and every handle must carry one. |
| `session`                           | `jevscript`                            | The tmux session panes are created in. Created on demand.                                                                 |
| `bin`                               | `claude`                               | The Claude Code binary.                                                                                                   |
| `args`                              | `[]`                                   | Extra arguments for every spawn, placed before the prompt.                                                                |
| `idleSeconds`                       | `20`                                   | How long the screen must stay unchanged before `wait idle` returns.                                                       |
| `pollMs`                            | `1000`                                 | How often `wait` looks.                                                                                                   |
| `tmux`                              | `tmux`                                 | The tmux binary.                                                                                                          |
| `configDir`                         | `$CLAUDE_CONFIG_DIR`, then `~/.claude` | Where Claude Code keeps transcripts.                                                                                      |
| `exec`, `readFile`, `clock`, `uuid` | the real ones                          | Injection points for tests.                                                                                               |


## The verbs

**`spawn`** takes `prompt` (required) and `in` (a handle from another adapter;
its `cwd`, `path` or `dir` field is the working directory, else the host's
`process.cwd()`). Adapter-specific named arguments pass through unchecked, as
spec section 9.1 allows: `session` (the tmux session for this pane), `model`
(`--model`), `permissionMode` (`--permission-mode`) and `args` (a list of extra
flags). The pane is created first and `remain-on-exit` set on it, then Claude
Code is started into it with `respawn-pane` and a fresh `--session-id`, so a
process that exits at once still leaves a dead pane whose exit status can be
read. The prompt is Claude Code's initial argument. The handle is:

```json
{
  "capability": "dev",
  "id": "<claude session uuid>",
  "session": "jevscript",
  "pane": "%7",
  "cwd": "/path/to/worktree",
  "session_id": "<claude session uuid>"
}
```

Everything needed to reattach after a host restart is in it.

**`observe`** returns at least the record below (spec section 9.1) plus
`session_id`, `turns` (assistant messages so far) and `usage`
(`{ tokens, usd }`; `usd` is `null` because Claude Code 2.x does not write cost
to the transcript). Spec section 7.1 says the runtime sums adapter usage for
information only.

```json
{
  "status": "running | waiting | exited",
  "last_message": "the agent's most recent complete message to the user",
  "tail": "the last screen, bounded to roughly 4k tokens",
  "exit_code": null
}
```

- `tail` is `tmux capture-pane -p -J`, trailing whitespace trimmed, cut to the
last 16k characters.
- `last_message` comes from Claude Code's own transcript,
`<configDir>/projects/<cwd with every non-alphanumeric character as` -`>/<session id>.jsonl`,
never from the screen, so a redraw does not change the answer. It is the text
blocks of the most recent assistant message that is complete (has a
`stop_reason`) and said anything; a message that only called tools is not a
message to the user. Subagent (`isSidechain`) lines are ignored.
- `status` is `exited` when the pane is dead or gone, `waiting` when the
transcript's last turn is a complete assistant message with no tool use
pending *and* the screen shows an empty input box, else `running`. Before the
first turn (including while the workspace-trust dialog is up) that means
`running`; `wait idle` still returns once the screen settles.
- `exit_code` is tmux's `pane_dead_status` when the pane is dead, else `null`.

**`send`** types one line with `send-keys -l` and submits with Enter. Text with
newlines goes through a tmux buffer and `paste-buffer -p`, so Claude Code takes
it as one bracketed paste rather than submitting at the first newline; Claude
Code then wraps it in its own `<pasted_content>` tags, which is how it treats
anything pasted.

**`wait idle, minutes n`** polls `observe` until the status is no longer
`running`, or the screen has not changed for `idleSeconds`, or `n` minutes have
passed, and returns the last observation with `waited` set to `status`, `idle`
or `timeout`. It blocks the host's adapter for that long by design (spec
section 9.6); the runtime pauses the run with `waiting` meanwhile and the host
may `inject`. Only the `idle` condition exists.

**`stop`** sends `C-c` twice with a short gap (one ends the turn, two end
Claude Code), then kills the pane if it is still there. A second `stop` finds
nothing and does nothing.

## Two rules that matter more than they look

- **Return structured records, never opaque blobs** (spec section 9.5), so that
`shape` and `trail` can work on them.
- **`tail` and `last_message` are agent-written text and can contain anything.**
A program should `shape` them before judging them, and this adapter never
interprets them: the only thing it reads off the screen is whether the input
box is empty, and that is one ingredient of `status`, not a message. The
`trail` records the runtime keeps are built from the runtime's own calls
precisely so that nothing an agent prints can steer them (spec section 7.5).

## Errors

Every failure is an `AdapterError` with `retryable`, which is what spec section
12 says an `adapter_error` carries: a tmux failure is retryable, a missing
binary or a pane that no longer exists is not. Note that the SDK currently
forwards only the message across the runtime boundary; carrying `retryable`
through `capability.call` is the SDK's to add.

## Developing

```sh
cd sdk/js && pnpm install && pnpm run build   # the adapter's types come from the SDK's dist
cd adapters/claude-code && pnpm install && pnpm test && pnpm run typecheck
```

The integration test is skipped when `tmux` is not on the PATH. Nothing here
ever starts the real `claude`; the fixture `test/fixtures/agent.sh` stands in
for it.