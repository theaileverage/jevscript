/**
 * Antigravity CLI (`agy`).
 *
 * Launch shape, effort values, trust store and busy markers follow agy 1.2.0
 * behavior: `--prompt-interactive` auto-submits the prompt,
 * `--dangerously-skip-permissions` is autonomy, effort is
 * `low|medium|high`. Every new folder asks `Do you trust the contents of this
 * project?` with trust preselected (one Enter answers it), and agy honours a
 * `trustedWorkspaces` entry in `~/.gemini/antigravity-cli/settings.json`
 * written ahead of launch. The busy row carries `esc to cancel`; the idle row
 * `? for shortcuts`. agy writes no transcript, so `last_message` is empty.
 */
import { realpath } from 'node:fs/promises'
import { join } from 'node:path'

import { AdapterError } from '../errors.ts'
import type { Harness, HarnessContext, SpawnRequest } from '../harness.ts'
import { foreignMarkers } from '../harness.ts'
import { commandModels, tabModels } from './models.ts'

/** Add `cwd` (logical and resolved, since agy compares the logical path) to agy's trusted workspaces. */
export async function registerAgyTrust(cwd: string, ctx: HarnessContext): Promise<void> {
  const path = join(ctx.home, '.gemini', 'antigravity-cli', 'settings.json')
  const text = await ctx.files.read(path)
  let settings: Record<string, unknown> = {}
  if (text !== undefined && text.trim() !== '') {
    try {
      settings = JSON.parse(text) as Record<string, unknown>
    } catch {
      throw new AdapterError(`${path} is not JSON; refusing to rewrite it`, false)
    }
  }
  const trusted = Array.isArray(settings['trustedWorkspaces']) ? (settings['trustedWorkspaces'] as unknown[]).map(String) : []
  const resolved = await realpath(cwd).catch(() => cwd)
  const missing = [cwd, resolved].filter((dir, i, all) => all.indexOf(dir) === i && !trusted.includes(dir))
  if (missing.length === 0) return
  settings['trustedWorkspaces'] = [...trusted, ...missing]
  await ctx.files.mkdirPrivate(join(ctx.home, '.gemini', 'antigravity-cli'))
  await ctx.files.write(path, `${JSON.stringify(settings, null, 2)}\n`, 0o600)
}

export const agy: Harness = {
  name: 'agy',
  title: 'Antigravity',
  bins: ['agy'],
  efforts: ['low', 'medium', 'high'],
  verified: 'agy 1.2.0 behavior; flags checked on 1.2.11',

  launch(request: SpawnRequest) {
    const argv = [request.bin, ...request.args, '--prompt-interactive', request.prompt]
    if (request.model) argv.push('--model', request.model)
    if (request.effort) argv.push('--effort', request.effort)
    if (request.yolo) argv.push('--dangerously-skip-permissions')
    return { argv, unset: foreignMarkers() }
  },

  register: (request, ctx) => registerAgyTrust(request.cwd, ctx),

  screen: {
    busy: [/esc\s+to\s+cancel/],
    idle: [/\? for shortcuts/],
    dialogs: [{ kind: 'trust', all: [/Do you trust the contents of this project\?/], answer: ['Enter'] }],
  },

  interrupt: { keys: ['Escape'], gapMs: 300 },
  models: commandModels(['models'], tabModels),
}
