/**
 * The fixture services a browser pass runs against: the TypeSafe stand-in and
 * the Anthropic stand-in from `harness.ts`, with Claude scripted to draft the
 * inbox triage program and to answer or edit at a pin. Run it directly to
 * serve them for a manual pass:
 *
 *   node test/fixtures.ts <dir>
 *
 * It prints the environment the toolbox needs and keeps running.
 */
import { readFile } from 'node:fs/promises'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'

import { REPO_ROOT } from '../server/env.ts'
import { fakeClaude, type FakeClaude, fakeJev, type FakeJev } from './harness.ts'

/** The annotator's worked example: `stuck` goes to `waiting_on_me` and stops sending. */
export const STUCK_TO_WAITING = {
  find: 'on stuck "the agent is looping or unsure how to proceed" -> nudging:\n      dev.send "Stop. In three lines, what is blocking you?"\n',
  replace: 'on stuck "the agent is looping or unsure how to proceed" -> waiting_on_me\n',
}

export interface Fixtures {
  jev: FakeJev
  claude: FakeClaude
  /** The variables that point a toolbox at these fixtures. */
  env: Record<string, string>
  close(): Promise<void>
}

export async function startFixtures(dir: string): Promise<Fixtures> {
  const triage = await readFile(join(REPO_ROOT, 'apps/toolbox/fixtures/inbox_triage.jev'), 'utf8')
  const jev = await fakeJev(['finished', 'approved', 'week'])
  const claude = await fakeClaude()
  claude.fallback = (request) => {
    const asked = JSON.stringify(request.messages.at(-1)?.content ?? '')
    if (!request.output_config?.format) return `Here is a program that sorts each message into a lane.\n\n\`\`\`jev\n${triage}\`\`\``
    if (asked.includes('ask me instead')) return JSON.stringify({ kind: 'edit', text: 'Send stuck agents to you instead of nudging them.', edits: [STUCK_TO_WAITING] })
    return JSON.stringify({ kind: 'answer', text: 'Only the guard tree.tests_pass can move working to reviewing.', edits: [] })
  }
  return {
    jev,
    claude,
    env: {
      TYPESAFE_API_KEY: 'fixture-key',
      JEVSCRIPT_PROFILES: await jev.profiles(dir),
      ANTHROPIC_API_KEY: 'fixture-key',
      ANTHROPIC_BASE_URL: claude.baseUrl,
    },
    async close() {
      await Promise.all([jev.close(), claude.close()])
    },
  }
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const dir = process.argv[2]
  if (!dir) throw new Error('usage: node test/fixtures.ts <dir>')
  const fixtures = await startFixtures(dir)
  for (const [key, value] of Object.entries(fixtures.env)) console.log(`export ${key}=${value}`)
  const stop = () => void fixtures.close().then(() => process.exit(0))
  process.on('SIGINT', stop)
  process.on('SIGTERM', stop)
}
