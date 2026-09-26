/**
 * omp (Oh My Pi), a Pi fork.
 *
 * Launch shape, effort values and busy markers follow omp 18.1.2 to 18.1.11
 * behavior: `OMP_SKIP_SETUP=1` suppresses the first-run
 * provider wizard, `--cwd` pins the directory, `--auto-approve` is autonomy,
 * effort is `--thinking`, and there is no trust gate. The busy row reads
 * `Working…` (U+2026 only) with a braille spinner and an elapsed cell. Turn
 * state and the last message come from omp's Pi-format session file under
 * `~/.omp/agent/sessions`.
 */
import { join } from 'node:path'

import type { Harness, SpawnRequest } from '../harness.ts'
import { foreignMarkers } from '../harness.ts'
import { commandModels, ompModels } from './models.ts'
import { locatePiSession, parsePiSession } from './pi-session.ts'

export const omp: Harness = {
  name: 'omp',
  title: 'omp',
  bins: ['omp'],
  efforts: ['off', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max', 'auto'],
  verified: 'omp 18.1.2-18.1.11; flags checked on 18.2.11',

  launch(request: SpawnRequest) {
    const argv = [request.bin, ...request.args, '--cwd', request.cwd]
    if (request.model) argv.push('--model', request.model)
    if (request.effort) argv.push('--thinking', request.effort)
    if (request.yolo) argv.push('--auto-approve')
    argv.push(request.prompt)
    return { argv, env: { OMP_SKIP_SETUP: '1' }, unset: foreignMarkers() }
  },

  handleFields: (request) => ({ started_at: request.startedAt }),

  screen: {
    busy: [/Working…/, /^\s*[⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏⣾⣽⣻⢿⡿⣟⣯⣷]\s+\d+[smh]/],
    idle: [/^\s*❯/],
  },

  transcript: {
    locate: (handle, ctx) => locatePiSession(join(ctx.home, '.omp', 'agent', 'sessions'), handle, ctx),
    parse: parsePiSession,
  },

  interrupt: { keys: ['Escape'], gapMs: 300 },
  models: commandModels(['models', '--json'], ompModels, { OMP_SKIP_SETUP: '1' }),
}
