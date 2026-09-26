/**
 * Claude Code.
 *
 * Launch shape, effort values, trust store and dialogs follow Claude Code
 * 2.1.196 to 2.1.269 behavior; the transcript and
 * input-box rules are the original Claude Code adapter's, smoke-tested on 2.1.
 * `status` comes from Claude Code's own transcript, never from a screen that
 * redraws: `waiting` needs the transcript's last turn to be a complete
 * assistant message with no tool use pending *and* an empty input box.
 */
import { AdapterError } from '../errors.ts'
import type { AgentHandle, Harness, HarnessContext, SpawnRequest } from '../harness.ts'
import { parseTranscript, transcriptPath } from './claude-transcript.ts'

/**
 * True when the last prompt marker on the screen is an empty input box.
 *
 * Claude Code draws its input box as `❯` at column 0 (or `>` on terminals
 * without the glyph) and echoes past prompts the same way, so only the last
 * marker counts and only when nothing follows it: an echoed prompt always has
 * text after the marker, and a draft the adapter has not submitted never
 * lingers because `send` always submits.
 */
export function promptVisible(screen: string): boolean {
  const lines = screen.split('\n')
  for (let i = lines.length - 1; i >= 0; i--) {
    const line = lines[i] ?? ''
    if (line.startsWith('❯') || line.startsWith('>')) {
      return /^[❯>][\s ]*$/.test(line)
    }
  }
  return false
}

/** `${CLAUDE_CONFIG_DIR:-~/.claude}`, or the adapter's `configDir` option. */
export function claudeConfigDir(ctx: Pick<HarnessContext, 'env' | 'home' | 'options'>): string {
  const configured = ctx.options['configDir']
  if (typeof configured === 'string' && configured !== '') return configured
  return ctx.env['CLAUDE_CONFIG_DIR'] ?? `${ctx.home}/.claude`
}

/**
 * Where Claude Code keeps per-project state, including trust:
 * `$CLAUDE_CONFIG_DIR/.claude.json`, else `~/.claude.json`.
 */
export function claudeStatePath(ctx: Pick<HarnessContext, 'env' | 'home'>): string {
  const dir = ctx.env['CLAUDE_CONFIG_DIR']
  return dir ? `${dir}/.claude.json` : `${ctx.home}/.claude.json`
}

/**
 * Record the working directory as trusted in Claude Code's own store using
 * `projects[<cwd>].hasTrustDialogAccepted`.
 * A project whose entry carries an explicit decline of external imports is
 * refused rather than given a consent the user never gave.
 */
export async function registerClaudeTrust(cwd: string, ctx: HarnessContext): Promise<void> {
  const path = claudeStatePath(ctx)
  const text = await ctx.files.read(path)
  let state: Record<string, unknown> = {}
  if (text !== undefined) {
    try {
      state = JSON.parse(text) as Record<string, unknown>
    } catch {
      throw new AdapterError(`${path} is not JSON; refusing to rewrite it`, false)
    }
  }
  const projects = (state['projects'] ?? {}) as Record<string, Record<string, unknown>>
  const entry = projects[cwd] ?? {}
  if (entry['hasTrustDialogAccepted'] === true) return
  if (entry['hasClaudeMdExternalIncludesApproved'] === false && entry['hasClaudeMdExternalIncludesWarningShown'] === true) {
    throw new AdapterError(`${cwd} carries an explicit decline in ${path}; not registering trust`, false)
  }
  projects[cwd] = { ...entry, hasTrustDialogAccepted: true }
  state['projects'] = projects
  await ctx.files.write(path, `${JSON.stringify(state, null, 2)}\n`, 0o600)
}

export const claude: Harness = {
  name: 'claude',
  title: 'Claude Code',
  bins: ['claude'],
  efforts: ['low', 'medium', 'high', 'xhigh', 'max'],
  verified: 'Claude Code 2.1.196-2.1.269; transcript format 2.1',

  launch(request: SpawnRequest) {
    const { named } = request
    const argv = [request.bin, '--session-id', request.sessionId, ...request.args]
    if (request.model) argv.push('--model', request.model)
    if (request.effort) argv.push('--effort', request.effort)
    if (typeof named['permissionMode'] === 'string') argv.push('--permission-mode', named['permissionMode'])
    if (request.yolo) argv.push('--dangerously-skip-permissions')
    argv.push(request.prompt)
    // Dim predicted text in an empty input box would read as a draft.
    return { argv, env: { CLAUDE_CODE_ENABLE_PROMPT_SUGGESTION: 'false' } }
  },

  handleFields: (request) => ({ session_id: request.sessionId }),

  validate(handle: AgentHandle) {
    if (typeof handle['session_id'] !== 'string') {
      throw new AdapterError('this handle was not made by the Claude Code adapter', false)
    }
  },

  register: (request, ctx) => registerClaudeTrust(request.cwd, ctx),

  screen: {
    busy: [/esc to interrupt/i],
    idle: promptVisible,
    dialogs: [
      // Both render with the cursor on the declining option: never answered with a key.
      { kind: 'trust', all: [/Is this a project you created or one you trust\?/] },
      { kind: 'imports', all: [/Allow external CLAUDE\.md file imports\?/] },
      { kind: 'permission', all: [/Bypass Permissions mode/i, /No, exit/] },
    ],
  },

  transcript: {
    async locate(handle, ctx) {
      return transcriptPath(claudeConfigDir(ctx), handle.cwd, String(handle['session_id']))
    },
    parse: parseTranscript,
  },

  classify: ({ transcript, screen }) => (transcript?.idle && promptVisible(screen) ? 'waiting' : 'running'),

  observationFields: (handle) => ({ session_id: handle['session_id'] }),

  // One interrupt ends the turn, two end Claude Code.
  interrupt: { keys: ['C-c', 'C-c'], gapMs: 300 },
  submit: { commandPrefixes: ['/'], commandSettleMs: 1200 },
}
