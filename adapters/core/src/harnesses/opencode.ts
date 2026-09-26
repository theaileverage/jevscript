/**
 * OpenCode.
 *
 * Launch shape and the autonomy override follow OpenCode 1.15.7 to 1.18.4
 * behavior: the prompt goes in with `--prompt`, there is no
 * trust dialog and no interactive effort flag, and autonomy is the config
 * override `{"permission":{"*":"allow"}}`. OpenCode writes no transcript an
 * adapter can read cheaply, so status is the screen's: the `esc interrupt`
 * footer while a turn runs, and the idle `Ask anything` placeholder. The
 * locally installed 2.0.6 could not complete a live authentication control
 * (`opencode run`: `User not found.`), so its turn behavior is unverified.
 */
import type { Harness, SpawnRequest } from '../harness.ts'
import { foreignMarkers } from '../harness.ts'
import { commandModels, lineModels } from './models.ts'

export const opencode: Harness = {
  name: 'opencode',
  title: 'OpenCode',
  bins: ['opencode'],
  efforts: null,
  verified: 'OpenCode 1.15.7-1.18.4',

  launch(request: SpawnRequest) {
    const argv = [request.bin, ...request.args]
    if (request.model) argv.push('--model', request.model)
    argv.push('--prompt', request.prompt)
    return {
      argv,
      unset: foreignMarkers(),
      ...(request.yolo ? { env: { OPENCODE_CONFIG_CONTENT: '{"permission":{"*":"allow"}}' } } : {}),
    }
  },

  handleFields: (request) => ({ started_at: request.startedAt }),

  screen: {
    busy: [/esc interrupt/i],
    idle: [/Ask anything/],
    scanLines: 20,
  },

  // Double Escape interrupts a turn.
  interrupt: { keys: ['Escape', 'Escape'], gapMs: 300 },
  models: commandModels(['models'], lineModels),
}
