/** Runtime connection/configuration provided to section 11.2 companion hosts. */
export const TS_CONTEXT = `import { readFileSync } from 'node:fs'
import { Socket } from 'node:net'
import { createInterface } from 'node:readline'
const config = JSON.parse(readFileSync(new URL('./config.json', import.meta.url), 'utf8'))
// Adapter effects execute in the toolbox process; these are their SDK declarations.
const bindings = Object.fromEntries(config.bindings.map(item => [item.name, {
  kind: item.kind, ...(item.manifest ? { manifest: item.manifest } : {}),
  call() { throw new Error('The toolbox runtime owns adapter dispatch.') },
}]))
const connection = new Socket({ fd: 3, readable: true, writable: true })
const answers = new Socket({ fd: 4, readable: true, writable: true }).unref()
const lines = createInterface({ input: answers })
const iterator = lines[Symbol.asyncIterator]()
export const runtime = { ...config, bindings, transport: { input: connection, output: connection } }
export async function answerPause(pause) {
  answers.write(JSON.stringify(pause) + '\\n')
  const line = await iterator.next()
  if (line.done) throw new Error('The toolbox pause connection closed.')
  const answer = JSON.parse(line.value)
  if (answer.abort) throw new Error('The run was aborted.')
  return answer
}

`
export const PY_CONTEXT = `import json
import os
from pathlib import Path
from types import SimpleNamespace

config = json.loads((Path(__file__).parent / '.toolbox/config.json').read_text())
class BoundAdapter:
    # Effects are dispatched by the toolbox process, never by generated code.
    def __init__(self, binding):
        self.kind = binding['kind']
        if 'manifest' in binding:
            self.manifest = binding['manifest']
    def call(self, verb, args):
        raise RuntimeError('The toolbox runtime owns adapter dispatch.')

runtime = SimpleNamespace(**{**config, 'bindings': {
    binding['name']: BoundAdapter(binding) for binding in config['bindings']
}, 'transport': (os.fdopen(os.dup(3), 'r'), os.fdopen(os.dup(3), 'w', buffering=1))})

def answer_pause(pause):
    os.write(4, (json.dumps(pause) + '\\n').encode())
    with os.fdopen(os.dup(4), 'r') as stream:
        answer = json.loads(stream.readline())
    if answer.get('abort'):
        raise RuntimeError('The run was aborted.')
    return answer
`
