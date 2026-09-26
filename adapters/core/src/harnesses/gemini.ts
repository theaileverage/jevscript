/**
 * Gemini CLI.
 *
 * Gemini CLI behavior: `-y` is autonomy, the folder-trust
 * dialog is suppressed by `GEMINI_CLI_TRUST_WORKSPACE=true` (never
 * `--skip-trust`, which skips project skills), there is no effort flag and no
 * model listing command (the `/model` dialog only). A running turn shows
 * `(esc to cancel, <n>s)`; the idle placeholder is
 * `Type your message or @path/to/file`. The API-key dialog is never typed
 * into: text typed there is saved as a credential.
 */
import type { Harness, SpawnRequest } from '../harness.ts'
import { foreignMarkers } from '../harness.ts'

export const gemini: Harness = {
  name: 'gemini',
  title: 'Gemini CLI',
  bins: ['gemini'],
  efforts: null,
  verified: 'Gemini CLI behavior documented; not installed on the machine this adapter was built on',

  launch(request: SpawnRequest) {
    const argv = [request.bin, ...request.args]
    if (request.yolo) argv.push('-y')
    if (request.model) argv.push('--model', request.model)
    argv.push(request.prompt)
    return {
      argv,
      unset: foreignMarkers('GEMINI_CLI'),
      ...(request.trust !== 'off' ? { env: { GEMINI_CLI_TRUST_WORKSPACE: 'true' } } : {}),
    }
  },

  screen: {
    busy: [/\(esc to cancel, \d+s\)/],
    idle: [/Type your message or @path\/to\/file/],
    dialogs: [
      { kind: 'trust', all: [/Do you trust the files in this folder\?/], answer: ['Enter'] },
      { kind: 'auth', all: [/How would you like to authenticate/] },
      { kind: 'auth', all: [/Enter Gemini API Key/] },
    ],
  },

  interrupt: { keys: ['Escape'], gapMs: 300 },
}
