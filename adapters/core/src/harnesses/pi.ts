/**
 * Pi.
 *
 * Launch shape, effort values and the trust dialog follow Pi 0.80 to 0.82
 * behavior: the prompt is one positional argument (more
 * would queue as separate messages), effort is `--thinking`, Pi has no
 * permission system, and a first run in an untrusted folder asks
 * `Trust project folder?` with trust preselected, answered with Enter. Turn
 * state and the last message come from Pi's session file
 * (`$PI_CODING_AGENT_DIR/sessions`, default `~/.pi/agent/sessions`).
 */
import { join } from 'node:path'

import type { Harness, HarnessContext, SpawnRequest } from '../harness.ts'
import { foreignMarkers } from '../harness.ts'
import { commandModels, tableModels } from './models.ts'
import { locatePiSession, parsePiSession } from './pi-session.ts'

export function piAgentDir(ctx: Pick<HarnessContext, 'env' | 'home'>): string {
  return ctx.env['PI_CODING_AGENT_DIR'] ?? join(ctx.home, '.pi', 'agent')
}

export const pi: Harness = {
  name: 'pi',
  title: 'Pi',
  bins: ['pi'],
  efforts: ['off', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max'],
  verified: 'Pi 0.80-0.82; flags checked on 0.87.1',

  launch(request: SpawnRequest) {
    const argv = [request.bin, ...request.args]
    if (request.model) argv.push('--model', request.model)
    if (request.effort) argv.push('--thinking', request.effort)
    argv.push(request.prompt)
    return { argv, unset: foreignMarkers() }
  },

  handleFields: (request) => ({ started_at: request.startedAt }),

  screen: {
    busy: [/Working\.\.\./],
    idle: (screen) => /^\s*─{3,}/m.test(screen),
    dialogs: [{ kind: 'trust', all: [/Trust project folder\?/], answer: ['Enter'] }],
  },

  transcript: {
    locate: (handle, ctx) => locatePiSession(join(piAgentDir(ctx), 'sessions'), handle, ctx),
    parse: parsePiSession,
  },

  interrupt: { keys: ['Escape'], gapMs: 300 },
  models: commandModels(['--list-models'], tableModels),
}
