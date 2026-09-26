import { createInterface } from 'node:readline'

for await (const line of createInterface({ input: process.stdin })) {
  const request = JSON.parse(line)
  if (request.operation === 'observe') {
    process.stdout.write(`${JSON.stringify({ observation: { status: 'waiting', last_message: 'finished', tail: 'finished', exit_code: null, id_seen: request.handle.id } })}\n`)
  } else if (request.verb === 'spawn') {
    process.stdout.write(`${JSON.stringify({ result: { id: 'fake-agent-1', capability: request.capability } })}\n`)
  } else if (request.verb === 'fail') {
    process.stdout.write(`${JSON.stringify({ error: { message: 'try later', retryable: true } })}\n`)
  } else if (request.verb === 'stop') {
    process.stdout.write('{"result":null}\n')
  } else {
    process.stdout.write(`${JSON.stringify({ result: { verb: request.verb, capability: request.capability, args: request.args } })}\n`)
  }
}
