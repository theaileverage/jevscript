/**
 * The command line every adapter executable shares.
 *
 * `jevscript-adapter-<cli> [options]` serves the JSONL subprocess protocol
 * (spec section 11.6) on stdin and stdout; `--discover` prints what the
 * machine has instead. `jevscript-agent <cli> [options]` is the same for any
 * harness in the catalog, and `jevscript-agent discover` lists them all.
 */
import { parseArgs } from 'node:util'

import { AgentAdapter, type AgentAdapterOptions } from './agent.ts'
import { backendName, BACKEND_NAMES, discoverBackends } from './backends/index.ts'
import { discover, discoverHarness } from './discover.ts'
import { AdapterError } from './errors.ts'
import { type Harness, TRUST_MODES, type TrustMode } from './harness.ts'
import { HARNESSES, harness as harnessNamed } from './harnesses/index.ts'
import { serveJsonl } from './jsonl.ts'

const OPTIONS = {
  capability: { type: 'string' },
  backend: { type: 'string' },
  session: { type: 'string' },
  'state-dir': { type: 'string' },
  bin: { type: 'string' },
  arg: { type: 'string', multiple: true },
  model: { type: 'string' },
  effort: { type: 'string' },
  yolo: { type: 'boolean' },
  trust: { type: 'string' },
  cwd: { type: 'string' },
  'idle-seconds': { type: 'string' },
  'poll-ms': { type: 'string' },
  discover: { type: 'boolean' },
  models: { type: 'boolean' },
  help: { type: 'boolean', short: 'h' },
} as const

function usage(name: string): string {
  return `usage: ${name} [options]

Serves the Jevscript subprocess adapter protocol (one JSON request per line on
stdin, one reply per line on stdout), for example:

  jevscript run fix_issue.jev --bind claude='${name} --backend tmux'

options:
  --capability NAME      capability name when a request does not carry one
  --backend NAME         terminal backend: ${BACKEND_NAMES.join(', ')} (default herdr,
                         or $JEVSCRIPT_AGENT_BACKEND)
  --session NAME         backend session (tmux/zellij session, herdr session)
  --state-dir DIR        where launch scripts and exit files go
  --bin PATH             the agent CLI binary
  --arg FLAG             an extra flag for every spawn (repeatable)
  --model ID             default model
  --effort LEVEL         default effort
  --yolo                 approve the agent's tool use without asking
  --trust MODE           folder-trust handling: ${TRUST_MODES.join(', ')} (default off)
  --cwd DIR              working directory when a spawn names none
  --idle-seconds N       how long an unchanged screen counts as idle (default 20)
  --poll-ms N            how often wait looks (default 1000)
  --discover [--models]  print what this machine has as JSON and exit
`
}

/** Build adapter options from parsed flags. */
export function optionsFrom(values: Record<string, unknown>): AgentAdapterOptions {
  const options: AgentAdapterOptions = {}
  const text = (key: string) => (typeof values[key] === 'string' ? (values[key] as string) : undefined)
  const capability = text('capability')
  if (capability) options.capability = capability
  const backend = text('backend')
  if (backend) options.backend = backendName(backend)
  const session = text('session')
  if (session) options.session = session
  const stateDir = text('state-dir')
  if (stateDir) options.backendOptions = { stateDir }
  const bin = text('bin')
  if (bin) options.bin = bin
  if (Array.isArray(values['arg'])) options.args = (values['arg'] as unknown[]).map(String)
  const model = text('model')
  if (model) options.model = model
  const effort = text('effort')
  if (effort) options.effort = effort
  if (values['yolo'] === true) options.yolo = true
  const trust = text('trust')
  if (trust) {
    if (!(TRUST_MODES as readonly string[]).includes(trust)) {
      throw new AdapterError(`--trust must be one of ${TRUST_MODES.join(', ')}`, false)
    }
    options.trust = trust as TrustMode
  }
  const cwd = text('cwd')
  if (cwd) options.cwd = cwd
  const idle = text('idle-seconds')
  if (idle) options.idleSeconds = Number(idle)
  const poll = text('poll-ms')
  if (poll) options.pollMs = Number(poll)
  return options
}

/**
 * Run one adapter executable. `make` builds the adapter, so a package can
 * construct its own class (Claude Code's keeps its original options).
 */
export async function runAdapterCli(
  harness: Harness,
  argv: string[] = process.argv.slice(2),
  make: (options: AgentAdapterOptions) => AgentAdapter = (options) => new AgentAdapter(harness, options),
  name = `jevscript-adapter-${harness.name}`,
): Promise<number> {
  let parsed
  try {
    parsed = parseArgs({ args: argv, options: OPTIONS, allowPositionals: false, strict: true })
  } catch (error) {
    process.stderr.write(`${name}: ${error instanceof Error ? error.message : String(error)}\n${usage(name)}`)
    return 2
  }
  const values = parsed.values as Record<string, unknown>
  if (values['help']) {
    process.stdout.write(usage(name))
    return 0
  }
  if (values['discover']) {
    const [agent, backends] = await Promise.all([
      discoverHarness(harness, { models: values['models'] === true }),
      discoverBackends(),
    ])
    process.stdout.write(`${JSON.stringify({ agent, backends }, null, 2)}\n`)
    return 0
  }
  let adapter: AgentAdapter
  try {
    adapter = make(optionsFrom(values))
  } catch (error) {
    process.stderr.write(`${name}: ${error instanceof Error ? error.message : String(error)}\n`)
    return 2
  }
  await serveJsonl(adapter)
  return 0
}

/** `jevscript-agent`: any harness by name, or `discover` for all of them. */
export async function runAgentCli(argv: string[] = process.argv.slice(2)): Promise<number> {
  const [command, ...rest] = argv
  if (command === 'discover') {
    const models = rest.includes('--models')
    const [agents, backends] = await Promise.all([discover({ models }), discoverBackends()])
    process.stdout.write(`${JSON.stringify({ agents, backends }, null, 2)}\n`)
    return 0
  }
  const harness = command ? harnessNamed(command) : undefined
  if (!harness) {
    process.stderr.write(
      `usage: jevscript-agent <${HARNESSES.map((h) => h.name).join('|')}> [options]\n       jevscript-agent discover [--models]\n`,
    )
    return command === '--help' || command === '-h' ? 0 : 2
  }
  return runAdapterCli(harness, rest, undefined, `jevscript-agent ${harness.name}`)
}
