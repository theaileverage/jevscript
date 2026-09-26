/**
 * Grok Build (`grok`).
 *
 * Grok Build 1.0.41 behavior: `--always-approve` is autonomy,
 * effort is `--reasoning-effort low|medium|high`, a linked worktree shows a
 * folder-trust gate (its text is not on record, so it is not detected), a running turn shows
 * `Ctrl+c:cancel` and the idle bar `Shift+Tab:mode`. Escape only focuses
 * scrollback, so the interrupt is Ctrl-C.
 */
import type { Harness, SpawnRequest } from '../harness.ts'
import { foreignMarkers } from '../harness.ts'
import { commandModels, lineModels } from './models.ts'

export const grok: Harness = {
  name: 'grok',
  title: 'Grok Build',
  bins: ['grok'],
  efforts: ['low', 'medium', 'high'],
  verified: 'Grok Build 1.0.41 behavior documented; not installed on the machine this adapter was built on',

  launch(request: SpawnRequest) {
    const argv = [request.bin, ...request.args]
    if (request.yolo) argv.push('--always-approve')
    if (request.model) argv.push('--model', request.model)
    if (request.effort) argv.push('--reasoning-effort', request.effort)
    argv.push(request.prompt)
    return { argv, unset: foreignMarkers('GROK_AGENT') }
  },

  screen: {
    busy: [/Ctrl\+c:cancel/],
    idle: [/Shift\+Tab:mode/],
    // The folder-trust gate is answered with Enter upstream, but its exact
    // text is not on record, so it is left for the program to see in `tail`.
  },

  interrupt: { keys: ['C-c'], gapMs: 300 },
  models: commandModels(['models'], lineModels),
}
