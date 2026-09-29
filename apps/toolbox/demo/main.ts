/**
 * `pnpm demo`: Jev and annotation fixtures, with real signed-in CLI Chat.
 *
 * Unlike `pnpm start` it does not load a
 * `.env` file, and before anything starts it removes every provider variable
 * from its runtime environment. Chat's CLI environment keeps sign-in and excludes
 * provider API keys and fixture endpoints.
 *
 * State lives in its own home (`~/.jevs-toolbox-demo` unless
 * `JEVS_TOOLBOX_HOME` says otherwise), apart from a real toolbox home and from
 * any checkout, so it survives a restart. `--reset` clears it first.
 */
import { existsSync } from 'node:fs'
import { mkdir, rm } from 'node:fs/promises'
import { homedir } from 'node:os'
import { join } from 'node:path'

import { startToolbox } from '../server/app.ts'
import { AnthropicModel } from '../server/claude.ts'
import { APP_ROOT } from '../server/env.ts'
import { serveBuilt } from '../server/static.ts'
import { startFixtures } from './services.ts'

/** Every variable through which a real provider credential or endpoint could reach the toolbox or the runtime. */
const PROVIDER_VARIABLES = ['TYPESAFE_API_KEY', 'JEVSCRIPT_PROFILES', 'ANTHROPIC_API_KEY', 'ANTHROPIC_AUTH_TOKEN', 'ANTHROPIC_BASE_URL', 'ANTHROPIC_PROFILE']

const home = process.env['JEVS_TOOLBOX_HOME'] ?? join(homedir(), '.jevs-toolbox-demo')
const port = Number(process.env['JEVS_TOOLBOX_PORT'] ?? 5188)
const dist = join(APP_ROOT, 'dist')

if (!existsSync(join(dist, 'index.html'))) {
  console.error('The page is not built. Run `pnpm demo`, which builds it first, or `pnpm build`.')
  process.exit(1)
}
if (process.argv.includes('--reset')) {
  // Only what the toolbox itself writes, so a home pointed somewhere unexpected loses nothing else.
  for (const entry of ['toolbox.sqlite', 'toolbox.sqlite-wal', 'toolbox.sqlite-shm', 'recordings', 'work', 'profiles.json']) {
    await rm(join(home, entry), { recursive: true, force: true })
  }
}

await mkdir(home, { recursive: true })
for (const name of PROVIDER_VARIABLES) delete process.env[name]
const fixtures = await startFixtures(home)
Object.assign(process.env, fixtures.env)

const modelName = 'claude-opus-5-5'
const toolbox = await startToolbox({
  env: process.env,
  home,
  port,
  model: new AnthropicModel(modelName, fixtures.env['ANTHROPIC_API_KEY']),
  modelName,
  services: 'demo',
  serveStatic: serveBuilt(dist),
})

console.log(
  [
    `jevs toolbox demo: ${toolbox.url}`,
    `  state:      ${toolbox.database} (ideas, pins, run index, resend history)`,
    `  recordings: ${join(home, 'recordings')} (JSONL, one per run)`,
    `  Jev:        fixture endpoint ${fixtures.jev.endpoint}`,
    `  Annotator:  Claude fixture endpoint ${fixtures.claude.baseUrl}`,
    '  Chat:       choose Claude Code or Codex; uses your signed-in local CLI for real replies.',
    '  No .env or provider API key is read. CLI sign-in is used for Chat. Ctrl-C stops the demo and its fixtures; `pnpm demo -- --reset` starts it clean.',
  ].join('\n'),
)

const stop = async () => {
  await toolbox.close()
  await fixtures.close()
  process.exit(0)
}
process.on('SIGINT', () => void stop())
process.on('SIGTERM', () => void stop())
