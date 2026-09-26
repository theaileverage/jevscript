/**
 * Rovo Dev CLI (`rovo`).
 *
 * Rovo Dev CLI behavior: `rovo run --yolo`, effort is the config
 * override `agent.efficiencyLevel` (`low|medium|high|max`, one
 * `--config-override` only, since a second replaces the first), and a
 * positional prompt never starts a turn, so the prompt is typed once
 * `Welcome to Rovo!` shows. A running turn shows `Rovo is thinking`; the idle
 * input box carries `? for shortcuts.`.
 */
import type { Harness, SpawnRequest } from '../harness.ts'
import { foreignMarkers } from '../harness.ts'

export const rovo: Harness = {
  name: 'rovo',
  title: 'Rovo Dev',
  bins: ['rovo'],
  efforts: ['low', 'medium', 'high', 'max'],
  verified: 'Rovo Dev CLI behavior documented; not installed on the machine this adapter was built on',

  launch(request: SpawnRequest) {
    const argv = [request.bin, 'run', ...request.args]
    if (request.yolo) argv.push('--yolo')
    if (request.model) argv.push('--model', request.model)
    if (request.effort) argv.push('--config-override', JSON.stringify({ agent: { efficiencyLevel: request.effort } }))
    return { argv, unset: foreignMarkers('ATLASSIAN_AGENT_TYPE', 'ROVODEV_CLI'), typePrompt: true }
  },

  ready: [/Welcome to Rovo!/],

  screen: {
    busy: [/Rovo is thinking/],
    idle: [/\? for shortcuts\./],
  },

  interrupt: { keys: ['Escape'], gapMs: 300 },
}
