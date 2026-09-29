/** Local Chat drafting uses signed-in CLI sessions, separate from section 9.1's runtime adapters. */
import { spawn } from 'node:child_process'
import { mkdir, mkdtemp, rm } from 'node:fs/promises'
import { join } from 'node:path'

import type { AgentModel, AgentStatus, ChatAgent } from '../shared/protocol.ts'
import type { Completion, Model } from './claude.ts'

const LABELS = { 'claude-code': 'Claude Code', codex: 'Codex' }
const MAX_BYTES = 2 * 1024 * 1024
const CLAUDE_ARGS = ['--print', '--tools', '', '--safe-mode', '--strict-mcp-config', '--mcp-config', '{"mcpServers":{}}', '--no-session-persistence', '--permission-mode', 'dontAsk', '--disable-slash-commands']
const CODEX_DISABLED = ['shell_tool', 'unified_exec', 'apps', 'plugins', 'hooks', 'multi_agent', 'multi_agent_v2', 'code_mode', 'code_mode_host', 'in_app_browser', 'sleep_tool', 'tool_suggest', 'skill_mcp_dependency_install', 'request_permissions_tool']

/** The server owns executable paths and bounds; the browser supplies only discovered model IDs. */
export interface AgentOptions {
  env: NodeJS.ProcessEnv
  home: string
  timeoutMs?: number
}

function object(value: unknown): Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value) ? value as Record<string, unknown> : {}
}

/** Keep the CLI's signed-in HOME/keychain, without demo endpoints, API keys or project configuration. */
function cliEnv(env: NodeJS.ProcessEnv): NodeJS.ProcessEnv {
  const result: NodeJS.ProcessEnv = {}
  for (const key of ['PATH', 'HOME', 'USER', 'LOGNAME', 'SHELL', 'TMPDIR', 'LANG', 'LC_ALL', 'CODEX_HOME', 'CLAUDE_CONFIG_DIR', 'CLAUDE_CODE_OAUTH_TOKEN']) {
    if (env[key] !== undefined) result[key] = env[key]
  }
  return result
}

/** Discards stderr and never includes raw subprocess errors, credentials or configuration in replies. */
async function cli(
  bin: string, args: string[], options: AgentOptions,
  drive: (send: (value: unknown) => void, stop: (value: unknown) => void, fail: (reason: string) => void) => (value: unknown) => void,
  timeoutMs: number,
  document = false,
): Promise<unknown> {
  const root = join(options.home, 'agent-work')
  await mkdir(root, { recursive: true })
  const cwd = await mkdtemp(join(root, 'turn-'))
  try {
    return await new Promise((resolve, reject) => {
      const child = spawn(bin, args, { cwd, env: cliEnv(options.env), detached: process.platform !== 'win32', stdio: ['pipe', 'pipe', 'pipe'] })
      let settled = false
      let bytes = 0
      let pending = ''
      let outcome: { error: Error | null; value?: unknown } | null = null
      const kill = () => {
        if (child.pid && process.platform !== 'win32') {
          try { process.kill(-child.pid, 'SIGKILL') } catch { /* Already exited. */ }
        } else child.kill('SIGKILL')
      }
      const finish = (error: Error | null, value?: unknown) => {
        if (settled) return
        settled = true
        clearTimeout(timer)
        outcome = { error, value }
        kill()
      }
      const timer = setTimeout(() => finish(new Error('The CLI timed out. Try again with a shorter request.')), timeoutMs)
      const consume = drive(
        value => typeof value === 'string' ? child.stdin.end(value) : child.stdin.write(JSON.stringify(value) + '\n'),
        value => finish(null, value),
        reason => finish(new Error(reason)),
      )
      child.stdin.on('error', () => {})
      child.on('error', () => finish(new Error('The CLI executable is unavailable. Install it and sign in, then refresh agents.')))
      child.stderr.on('data', (chunk: Buffer) => {
        bytes += chunk.length
        if (bytes > MAX_BYTES) finish(new Error('The CLI exceeded the response size limit.'))
      })
      child.stdout.setEncoding('utf8').on('data', (chunk: string) => {
        if (settled) return
        bytes += Buffer.byteLength(chunk)
        if (bytes > MAX_BYTES) return finish(new Error('The CLI exceeded the response size limit.'))
        pending += chunk
        if (document) return
        let end: number
        while ((end = pending.indexOf('\n')) >= 0) {
          const line = pending.slice(0, end)
          pending = pending.slice(end + 1)
          if (!line.trim()) continue
          try { consume(JSON.parse(line)) } catch {
            finish(new Error('The CLI returned an invalid response. Update the CLI or try again.'))
          }
        }
      })
      child.on('close', code => {
        if (!settled && pending.trim()) {
          try { consume(JSON.parse(pending)) } catch { /* Report only a safe error below. */ }
        }
        if (!settled) finish(new Error(code === 0 ? 'The CLI returned no response.' : 'The CLI failed. Check its sign-in, model access and usage limits, then try again.'))
        if (outcome?.error) reject(outcome.error)
        else resolve(outcome?.value)
      })
    })
  } finally {
    await rm(cwd, { recursive: true, force: true })
  }
}

/** Discovery and completion share the same signed-in local CLI, never a provider API fallback. */
export class LocalAgents {
  readonly #options: AgentOptions
  #statuses: AgentStatus[] = []

  constructor(options: AgentOptions) { this.#options = options }

  #bin(harness: ChatAgent['harness']): string {
    return this.#options.env[harness === 'codex' ? 'JEVS_TOOLBOX_CODEX_BIN' : 'JEVS_TOOLBOX_CLAUDE_BIN'] ?? (harness === 'codex' ? 'codex' : 'claude')
  }

  async discover(): Promise<AgentStatus[]> {
    this.#statuses = await Promise.all((['claude-code', 'codex'] as const).map(async harness => {
      try {
        const models = harness === 'codex' ? await this.#codexModels() : await this.#claudeModels()
        if (!models.length) throw new Error('The CLI advertised no models. Sign in or update it, then refresh agents.')
        return { harness, models, state: 'available' } satisfies AgentStatus
      } catch (error) {
        return { harness, models: [], state: 'unavailable', reason: error instanceof Error ? error.message : 'Discovery failed.' } satisfies AgentStatus
      }
    }))
    return this.#statuses
  }

  async #claudeModels(): Promise<AgentModel[]> {
    const auth = object(await cli(this.#bin('claude-code'), ['auth', 'status', '--json'], this.#options, (_send, stop) => stop, 15_000, true))
    if (auth['loggedIn'] !== true) throw new Error('Claude Code is signed out. Run `claude auth login`, then refresh agents.')
    const result = object(await cli(this.#bin('claude-code'), [...CLAUDE_ARGS, '--input-format', 'stream-json', '--output-format', 'stream-json', '--verbose'], this.#options, (send, stop) => {
      send({ type: 'control_request', request_id: 'models', request: { subtype: 'initialize' } })
      return value => {
        const event = object(value)
        if (event['type'] === 'control_response') stop(object(object(event['response'])['response']))
      }
    }, 15_000))
    if (!Array.isArray(result['models'])) return []
    return result['models'].flatMap(value => {
      const model = object(value)
      return typeof model['value'] === 'string' && typeof model['displayName'] === 'string' && typeof model['resolvedModel'] === 'string'
        ? [{ id: model['value'], name: model['displayName'], resolved: model['resolvedModel'] }] : []
    })
  }

  async #codexModels(): Promise<AgentModel[]> {
    const models: AgentModel[] = []
    await cli(this.#bin('codex'), ['app-server', '--stdio', '-c', 'features.hooks=false', '-c', 'features.plugins=false', '-c', 'mcp_servers={}'], this.#options, (send, stop, fail) => {
      send({ id: 1, method: 'initialize', params: { clientInfo: { name: 'jevs_toolbox', version: '0.1.0' } } })
      const list = (cursor: string | null) => send({ id: 2, method: 'model/list', params: { limit: 100, includeHidden: false, cursor } })
      return value => {
        const event = object(value)
        if (event['error']) return fail('Codex model discovery failed. Update the CLI or check its sign-in, then refresh agents.')
        if (event['id'] === 1) { send({ method: 'initialized' }); send({ id: 3, method: 'account/read', params: { refreshToken: false } }) }
        if (event['id'] === 3) {
          if (!object(event['result'])['account']) return fail('Codex is signed out. Run `codex login`, then refresh agents.')
          list(null)
        }
        if (event['id'] !== 2) return
        const result = object(event['result'])
        if (!Array.isArray(result['data'])) return fail('Codex returned an invalid model list. Update the CLI, then refresh agents.')
        for (const value of result['data']) {
          const model = object(value)
          if (typeof model['model'] === 'string' && typeof model['displayName'] === 'string' && model['hidden'] !== true) {
            const entry = { id: model['model'], name: model['displayName'], resolved: model['model'] }
            if (model['isDefault']) models.unshift(entry)
            else models.push(entry)
          }
        }
        if (typeof result['nextCursor'] === 'string') list(result['nextCursor'])
        else stop(null)
      }
    }, 15_000)
    return models
  }

  model(selection: ChatAgent): Model {
    if (!['claude-code', 'codex'].includes(selection.harness) || typeof selection.model !== 'string') throw new Error('Choose a discovered local agent and model.')
    const status = this.#statuses.find(item => item.harness === selection.harness)
    if (!status || status.state === 'unavailable') throw new Error(`${LABELS[selection.harness]} is unavailable. Install the CLI and sign in, then refresh agents.`)
    if (!status.models.some(model => model.id === selection.model)) throw new Error('This model is no longer advertised by the CLI. Refresh agents and choose a listed model.')
    return {
      name: selection.model,
      complete: async (system, messages): Promise<Completion> => {
        const prompt = system + '\n\nRespond only with text. Do not use tools, run commands or change files.\n\nConversation:\n' + JSON.stringify(messages)
        if (Buffer.byteLength(prompt) > 512 * 1024) throw new Error('This conversation exceeds the CLI input limit. Start a new idea or shorten it.')
        const timeout = Math.min(this.#options.timeoutMs ?? 90_000, 90_000)
        const completion = selection.harness === 'claude-code'
          ? await this.#claudeResponse(selection, prompt, timeout)
          : await this.#codexResponse(selection, prompt, timeout)
        return { content: [{ type: 'text', text: completion.text }], ...completion }
      },
    }
  }

  async #claudeResponse(selection: ChatAgent, prompt: string, timeout: number): Promise<{ text: string; model: string }> {
    const result = object(await cli(this.#bin(selection.harness), [...CLAUDE_ARGS, '--model', selection.model, '--output-format', 'json'], this.#options, (send, stop) => {
      // Text input goes through stdin, so prompts never become shell code or process arguments.
      send(prompt)
      return value => stop(value)
    }, timeout, true))
    if (result['is_error'] || typeof result['result'] !== 'string' || !result['result'].trim()) throw new Error('Claude Code failed. Check its sign-in, model access and usage limits, then try again.')
    const resolved = Object.keys(object(result['modelUsage']))
    return { text: result['result'], model: resolved.join(', ') || selection.model }
  }

  async #codexResponse(selection: ChatAgent, prompt: string, timeout: number): Promise<{ text: string; model: string }> {
    let text = ''
    const args = ['exec', '--ignore-user-config', '--ignore-rules', '--ephemeral', '--skip-git-repo-check', '--sandbox', 'read-only', '--json', '--model', selection.model, '-c', 'approval_policy="never"', '-c', 'web_search="disabled"', ...CODEX_DISABLED.flatMap(feature => ['--disable', feature]), '-']
    await cli(this.#bin(selection.harness), args, this.#options, (send, stop, fail) => {
      send(prompt)
      return value => {
        const event = object(value)
        const item = object(event['item'])
        if (event['type'] === 'item.completed' && item['type'] === 'agent_message' && typeof item['text'] === 'string') text = item['text']
        if (event['type'] === 'turn.failed' || event['type'] === 'error') return fail('Codex failed. Check its sign-in, model access and usage limits, then try again.')
        if (event['type'] === 'turn.completed') stop(null)
      }
    }, timeout)
    if (!text.trim()) throw new Error('Codex returned no assistant response.')
    return { text, model: selection.model }
  }
}
