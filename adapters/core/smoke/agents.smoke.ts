/**
 * Opt-in live smoke: a trivial real turn for every installed CLI on tmux and,
 * only with JEVSCRIPT_HERDR_LAB=1, Herdr. Never part of CI.
 */
import { mkdtemp, rm } from 'node:fs/promises'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'

import { AgentAdapter, createBackend, discoverHarness, HARNESSES } from '../src/index.ts'

const LIVE = process.env['JEVSCRIPT_LIVE_SMOKE'] === '1'
const only = new Set((process.env['JEVSCRIPT_SMOKE_AGENTS'] ?? '').split(',').filter(Boolean))

describe.skipIf(!LIVE)('live agent CLIs', () => {
  for (const backendName of ['tmux', 'herdr'] as const) {
    for (const harness of HARNESSES) {
      it(`${harness.name} on ${backendName} spawns and observes a trivial turn`, async (test) => {
        if (only.size && !only.has(harness.name)) test.skip()
        // Herdr panes run only when JEVSCRIPT_HERDR_LAB=1 opts in.
        if (backendName === 'herdr' && process.env['JEVSCRIPT_HERDR_LAB'] !== '1') test.skip()
        const agent = await discoverHarness(harness)
        const backend = createBackend(backendName, { session: `jevscript-smoke-${process.pid}` })
        const probe = await backend.probe()
        if (!agent.installed || !probe.ready) test.skip()

        const cwd = await mkdtemp(join(process.cwd(), '.smoke-'))
        let handle: Awaited<ReturnType<AgentAdapter['spawn']>> | undefined
        try {
          const adapter = new AgentAdapter(harness, {
            backend,
            cwd,
            trust: 'dialog',
            idleSeconds: 12,
            turnStartSeconds: { spawn: 45 },
          })
          handle = await adapter.spawn({ named: { prompt: 'Reply with exactly JEVSCRIPT_SMOKE_OK. Do not use tools.' } })
          const observed = await adapter.observe(handle)
          expect(['running', 'waiting', 'exited']).toContain(observed.status)
          const deadline = Date.now() + 120_000
          let settled = observed
          // Live CLIs can show a login gate only after their first render.
          while (settled.status === 'running' && Date.now() < deadline) {
            if (settled.dialog === 'auth') throw new Error(`${harness.name} needs authentication: ${settled.tail.slice(-300)}`)
            await new Promise((resolve) => setTimeout(resolve, 1_500))
            settled = await adapter.observe(handle)
          }
          if (settled.status === 'running') {
            process.stderr.write(`${harness.name} on ${backendName} stayed running: ${JSON.stringify({
              busy: settled.busy, last_message: settled.last_message,
              tail: settled.tail.slice(-1200),
            })}\n`)
          }
          expect(['waiting', 'exited']).toContain(settled.status)
          expect(`${settled.last_message}\n${settled.tail}`).toContain('JEVSCRIPT_SMOKE_OK')
        } finally {
          if (handle) await new AgentAdapter(harness, { backend }).stop(handle)
          await rm(cwd, { recursive: true, force: true })
        }
      }, 300_000)
    }
  }
})
