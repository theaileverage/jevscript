# Get started with Jevscript

In this tutorial we write a program that asks you a question, run it, record
the run, replay the recording, and then drive the same program from a Python or
JavaScript host. None of it calls a model, so you need no API key or account.

You need one of these:

- Node 22 or later, for the npm package.
- Python 3.10 or later, for the PyPI package.

The packages run on macOS 15 or later (arm64 and x86_64), Linux with glibc 2.39
or later (arm64 and x86_64), and Windows (x86_64).

## Install the CLI

Each package contains the SDK and the `jevscript` command with its native
binary. Pick one.

With npm, in a new project directory:

```sh
npm init -y
npm install @theaileverage/jevscript
npx jevscript --version
```

With pip, in a virtual environment:

```sh
python -m venv .venv
. .venv/bin/activate
python -m pip install jevscript
jevscript --version
```

On Windows, activate the environment with `.venv\Scripts\activate`.

The last command prints the installed version. Today the npm package prints
`jevscript 0.1.3` and the PyPI package prints `jevscript 0.1.2`. Everything on
this page works with both.
The rest of this page writes `jevscript`. If you installed with npm, write
`npx jevscript` instead.

## Write a program

Save this as `hello.jev`:

```
program hello

in name: text
out greeting

needs me: person

task main:
  reply = me.ask "Greet {name}?"
  if reply.answer == "yes":
    greeting = "Hello, {name}"
  else:
    greeting = "Not today, {name}"
  log info "greeted" { answer: reply.answer }
```

`in` declares an input the host supplies. `out` declares an output the task
sets. `needs me: person` declares a capability. The program may ask a person
questions, and the host decides who that person is. `me.ask` pauses the run
until the host answers. Blocks are indented with spaces. A tab in indentation
is a compile error.

## Check it

```sh
jevscript check hello.jev
```

A correct program prints nothing and exits with code 0. To see a diagnostic,
change `reply.answer` to `rply.answer` on the `log` line and check again:

```
hello.jev:14:31: unassigned_read: `rply` is read before it is assigned (spec section 5.3)
  14 |   log info "greeted" { answer: rply.answer }
     |                                ^^^^
  cause: A variable can be read before any assignment reaches it.
  help: Initialize it before the branch or assign it on every path that reaches the read.
  docs: https://github.com/theaileverage/jevscript/blob/main/docs/error-reference.md#unassigned_read
```

The command exits with code 1. Change the name back before you go on.

## Run it and record the run

```sh
jevscript run hello.jev --input '{"name":"Ada"}' --stub me --record hello.jsonl
```

`--stub me` binds `me` to a built-in demonstration adapter, so the terminal
answers for the person. The run prints the `confirm` pause as JSON and asks:

```
Greet Ada?
options: yes, no
```

Type `yes` and press Enter. The run prints the `log` line and ends with a
`done` pause that carries the output:

```
log info main 14:3: greeted {"answer":"yes"}
{"kind":"done", ..., "outputs":{"greeting":"Hello, Ada"}, "verified":false, ...}
```

`verified` is `false` because the task declares no `verify` condition.
`hello.jsonl` now holds the whole run: the linked program, your answer, and
the log line.

## Replay the recording

```sh
jevscript replay hello.jsonl
```

The replay prints the same pause, log line, and outputs without asking you
anything. It reads only the recording, so it works without `hello.jev`.

Run the `run` command again with the same `--record` path and it fails with
`File exists`. The CLI never overwrites or appends to a recording. Choose a
new path for each run.

## Drive it from a host

A host program starts runs and answers their pauses in code. The host binds
`me` to an adapter object with `kind = "person"`. The runtime turns `me.ask`
into a `confirm` pause for the host to resume, so this program never reaches
the adapter's `call`.

In Python, save this as `host.py` and run `python host.py`:

```python
from jevscript import load


class Person:
    kind = "person"

    def call(self, verb, args, capability=None):
        print("person", verb, args)


with load("hello.jev") as program:
    run = program.task("main").start(inputs={"name": "Ada"}, bind={"me": Person()})
    for pause in run:
        if pause["kind"] == "confirm":
            print(pause["message"])
            run.resume({"answer": "yes"})
        elif pause["kind"] == "done":
            print(pause["outputs"])
```

In JavaScript, save this as `host.mjs` and run `node host.mjs`:

```js
import { load } from '@theaileverage/jevscript'

const me = { kind: 'person', call: async (verb, args) => console.log('person', verb, args) }
const program = await load('hello.jev')
const run = program.task('main').start({ inputs: { name: 'Ada' }, bind: { me } })
for await (const pause of run) {
  if (pause.kind === 'confirm') {
    console.log(pause.message)
    await run.resume({ answer: 'yes' })
  } else if (pause.kind === 'done') {
    console.log(pause.outputs)
  }
}
await program.close()
```

The Python host prints:

```
Greet Ada?
{'greeting': 'Hello, Ada'}
```

The JavaScript host prints the same question and `{ greeting: 'Hello, Ada' }`.

## Next steps

- Ask Jev a question. [`inbox_triage.jev`](../examples/inbox_triage.jev) is a
  judgment-only program. `jevscript judge inbox_triage.jev triage --state
  '{"message":"..."}'` sends it to Jev and needs `TYPESAFE_API_KEY`. Both
  packages include this file under `examples/`.
- Read [how Jevscript programs work](concepts.md) for judgments, tasks,
  machines, pauses, and recordings.
- Look up commands and flags in the [CLI reference](cli.md).
- Bind a real coding agent with an [agent adapter](../adapters/README.md).
- Read the host APIs in the [JavaScript SDK](../sdk/js/README.md) and
  [Python SDK](../sdk/python/README.md) guides.
