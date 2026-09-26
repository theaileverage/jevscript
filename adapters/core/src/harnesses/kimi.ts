/**
 * Kimi Code (`kimi`).
 *
 * Kimi Code behavior: `--auto` is autonomy (`--yolo` is a weaker
 * tier), the CLI takes no prompt on its command line so the prompt is typed
 * once `Welcome to Kimi Code!` shows, and the folder-trust dialog (all of
 * `Trust this folder?`, `Don't trust`) is answered with Enter. Busy state is
 * unverified upstream; the rendered moon-phase row is the only marker.
 */
import type { Harness, SpawnRequest } from '../harness.ts'
import { foreignMarkers } from '../harness.ts'
import { commandModels, kimiModels } from './models.ts'

export const kimi: Harness = {
  name: 'kimi',
  title: 'Kimi Code',
  bins: ['kimi'],
  efforts: null,
  verified: 'Kimi Code behavior documented; busy state unverified upstream; not installed on the machine this adapter was built on',

  launch(request: SpawnRequest) {
    const argv = [request.bin, ...request.args]
    if (request.model) argv.push('--model', request.model)
    if (request.yolo) argv.push('--auto')
    return { argv, unset: foreignMarkers(), typePrompt: true }
  },

  ready: [/Welcome to Kimi Code!/],

  screen: {
    busy: [/^\s*[🌑🌒🌓🌔🌕🌖🌗🌘]\s+·\s+/u],
    idle: [/context:\s*[\d.]+\s*%/],
    dialogs: [{ kind: 'trust', all: [/Trust this folder\?/, /Don't trust/], answer: ['Enter'] }],
  },

  interrupt: { keys: ['Escape'], gapMs: 300 },
  models: commandModels(['provider', 'list', '--json'], kimiModels),
}
