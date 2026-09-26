/**
 * Muse (`muse`).
 *
 * Muse CLI behavior: `--yolo` is autonomy and suppresses the
 * workspace-trust dialog, effort is `--reasoning-effort` (`low` to `xhigh`,
 * with `max` passed as `ultra`), models are the `meta` provider's only, and an
 * interrupt is Escape followed by Ctrl-U because the cancelled prompt is put
 * back into the input box. The input box is `⟩`.
 */
import type { Harness, SpawnRequest } from '../harness.ts'
import { foreignMarkers } from '../harness.ts'

export const muse: Harness = {
  name: 'muse',
  title: 'Muse',
  bins: ['muse'],
  efforts: ['low', 'medium', 'high', 'xhigh', 'max'],
  verified: 'Muse CLI behavior documented; not installed on the machine this adapter was built on',

  launch(request: SpawnRequest) {
    const argv = [request.bin, ...request.args]
    if (request.yolo) argv.push('--yolo')
    if (request.model) argv.push('--model', request.model)
    if (request.effort) argv.push('--reasoning-effort', request.effort === 'max' ? 'ultra' : request.effort)
    argv.push(request.prompt)
    return { argv, unset: foreignMarkers() }
  },

  screen: {
    busy: [/esc to (interrupt|cancel)/i],
    idle: [/^\s*⟩\s*$/],
    // Only `--yolo` suppresses it; there is no key answer on record.
    dialogs: [{ kind: 'trust', all: [/Do you trust this workspace\?/] }],
  },

  interrupt: { keys: ['Escape', 'C-u'], gapMs: 300 },
}
