/**
 * Devin CLI (`devin`).
 *
 * Devin CLI behavior: `--permission-mode dangerous` is
 * autonomy, `--respect-workspace-trust false` skips the trust gate, the prompt
 * follows `--`, and there is no effort flag (effort is part of the model id,
 * e.g. `swe-2-medium`). A running turn shows `esc twice to interrupt` or
 * `❭ Guide Devin while it works`; the idle placeholder is
 * `Ask Devin to build features, fix bugs, or work on your code`. The
 * interrupt is Escape, then Escape again after a pause (a fast double Escape
 * while idle opens the revert menu instead).
 */
import type { Harness, SpawnRequest } from '../harness.ts'
import { foreignMarkers } from '../harness.ts'
import { commandModels, lineModels } from './models.ts'

export const devin: Harness = {
  name: 'devin',
  title: 'Devin CLI',
  bins: ['devin'],
  efforts: null,
  verified: 'Devin CLI behavior documented; not installed on the machine this adapter was built on',

  launch(request: SpawnRequest) {
    const argv = [request.bin, ...request.args]
    if (request.yolo) argv.push('--permission-mode', 'dangerous')
    if (request.trust !== 'off') argv.push('--respect-workspace-trust', 'false')
    if (request.model) argv.push('--model', request.model)
    argv.push('--', request.prompt)
    return { argv, unset: [...foreignMarkers(), 'NO_COLOR'] }
  },

  screen: {
    busy: [/esc twice to interrupt/, /^\s*❭ Guide Devin while it works$/],
    idle: [/Ask Devin to build features, fix bugs, or work on your code/],
  },

  interrupt: { keys: ['Escape', 'Escape'], gapMs: 600 },
  models: commandModels(['models', 'list'], lineModels),
}
