/** Visible portable SDK context, spec sections 9.5, 11.2 and 11.5. */
export const TS_SUPPORT = `import { readFileSync } from 'node:fs'
import { Socket } from 'node:net'
import { createInterface } from 'node:readline'
import { subprocessAgent } from '@theaileverage/jevscript'

// Toolbox Run supplies only configuration and stdio. Outside it, use the saved runtime.json.
const embedded = process.env.JEVS_TOOLBOX_EMBEDDED === '1'
const config = JSON.parse(readFileSync(new URL(embedded ? './.toolbox/config.json' : './runtime.json', import.meta.url), 'utf8'))
const adapters = []
const bindings = Object.fromEntries(config.bindings.map(binding => {
  if (binding.kind === 'person') return [binding.name, { kind: 'person', call(verb, args) {
    if (verb !== 'notify') throw new Error('Person ask/take_over are SDK pauses.')
    console.log(args.positional?.[0] ?? '')
    return null
  } }]
  if (embedded) return [binding.name, { kind: binding.kind, ...(binding.manifest ? { manifest: binding.manifest } : {}), call() {
    throw new Error('Embedded adapter effects are dispatched by the toolbox runtime.')
  } }]
  if (!binding.command) throw new Error('Configure a real JSONL adapter command for ' + binding.name + ' in runtime.json. A fixture binding is not a live adapter.')
  const adapter = subprocessAgent(binding.command, binding.args ?? [])
  adapters.push(adapter)
  return [binding.name, { kind: binding.kind, ...(binding.manifest ? { manifest: binding.manifest } : {}),
    call: (verb, args, capability) => adapter.call(verb, args, capability),
    observe: (handle, capability) => adapter.observe(handle, capability),
  }]
}))
const connection = embedded ? new Socket({ fd: 3, readable: true, writable: true }) : undefined
const answers = embedded ? new Socket({ fd: 4, readable: true, writable: true }).unref() : process.stdin
const lines = createInterface({ input: answers })
const iterator = lines[Symbol.asyncIterator]()
export const runtime = { ...config, bindings, transport: connection ? { input: connection, output: connection } : undefined }
export async function answerPause(pause) {
  if (embedded) answers.write(JSON.stringify(pause) + '\\n')
  else console.log('Pause:', pause, '\\nEnter a JSON resume payload, such as {"answer":"yes"}, {"retry":true} or {"extend":{"steps":10}}:')
  const line = await iterator.next()
  if (line.done) throw new Error('The pause input closed.')
  const answer = JSON.parse(line.value)
  if (answer.abort) throw new Error('The run was aborted.')
  return answer
}
export async function closeRuntime() {
  lines.close()
  if (embedded) answers.destroy()
  await Promise.all(adapters.map(adapter => adapter.close()))
}
`
export const PY_SUPPORT = `import json
import os
from pathlib import Path
from types import SimpleNamespace
from jevscript import subprocess_agent

# Toolbox Run supplies only configuration and stdio. Outside it, use runtime.json.
embedded = os.environ.get('JEVS_TOOLBOX_EMBEDDED') == '1'
config = json.loads((Path(__file__).parent / ('.toolbox/config.json' if embedded else 'runtime.json')).read_text())
adapters = []

class Person:
    kind = 'person'
    def call(self, verb, args):
        if verb != 'notify':
            raise RuntimeError('Person ask/take_over are SDK pauses.')
        print((args.get('positional') or [''])[0])
        return None

class EmbeddedBinding:
    def __init__(self, binding):
        self.kind = binding['kind']
        if 'manifest' in binding:
            self.manifest = binding['manifest']
    def call(self, verb, args):
        raise RuntimeError('Embedded adapter effects are dispatched by the toolbox runtime.')

def bind(binding):
    if binding['kind'] == 'person':
        return Person()
    if embedded:
        return EmbeddedBinding(binding)
    if not binding.get('command'):
        raise RuntimeError('Configure a real JSONL adapter command for ' + binding['name'] + ' in runtime.json. A fixture binding is not a live adapter.')
    adapter = subprocess_agent(binding['command'], binding.get('args', []))
    adapter.kind = binding['kind']
    if 'manifest' in binding:
        adapter.manifest = binding['manifest']
    adapters.append(adapter)
    return adapter

runtime = SimpleNamespace(**{**config, 'bindings': {binding['name']: bind(binding) for binding in config['bindings']},
    'transport': (os.fdopen(os.dup(3), 'r'), os.fdopen(os.dup(3), 'w', buffering=1)) if embedded else None})

def answer_pause(pause):
    if embedded:
        os.write(4, (json.dumps(pause) + '\\n').encode())
        with os.fdopen(os.dup(4), 'r') as stream:
            answer = json.loads(stream.readline())
    else:
        answer = json.loads(input('Pause: ' + str(pause) + '\\nEnter JSON resume payload: '))
    if answer.get('abort'):
        raise RuntimeError('The run was aborted.')
    return answer

def close_runtime():
    for adapter in adapters:
        adapter.close()
`
