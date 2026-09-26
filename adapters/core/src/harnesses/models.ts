/**
 * Parsers for the model listings agent CLIs print, one per documented shape
 * for each harness. Each is a
 * pure function of the command's output so that discovery can be tested
 * without the CLI.
 */
import { AdapterError } from '../errors.ts'
import type { HarnessContext, ModelInfo, ModelSource } from '../harness.ts'
import { stripAnsi } from '../screen.ts'

/** A listing that runs `<bin> <args>` and parses its stdout. */
export function commandModels(
  args: readonly string[],
  parse: (stdout: string) => ModelInfo[],
  env: Record<string, string> = {},
): ModelSource {
  return {
    source: `\`<bin> ${args.join(' ')}\``,
    async list(bin: string, ctx: HarnessContext) {
      const result = await ctx.exec(bin, [...args], { timeoutMs: 20_000, env })
      if (result.code !== 0) {
        throw new AdapterError(`${bin} ${args.join(' ')}: ${result.stderr.trim() || `exit status ${result.code}`}`, true)
      }
      return parse(result.stdout)
    },
  }
}

/** One id per non-blank line (`opencode models`: `provider/model`). */
export function lineModels(stdout: string): ModelInfo[] {
  return stripAnsi(stdout)
    .split('\n')
    .map((line) => line.trim())
    .filter((line) => line !== '' && !/\s/.test(line))
    .map((id) => ({ id }))
}

/** `<id>\t<label>` per line (`agy models`). */
export function tabModels(stdout: string): ModelInfo[] {
  return stripAnsi(stdout)
    .split('\n')
    .map((line) => line.split('\t'))
    .filter(([id]) => id !== undefined && id.trim() !== '' && !/\s/.test(id.trim()))
    .map(([id, name]) => ({ id: (id as string).trim(), ...(name?.trim() ? { name: name.trim() } : {}) }))
}

/** `<id> - <label>` per line, other lines ignored (`cursor-agent --list-models`). */
export function dashModels(stdout: string): ModelInfo[] {
  const models: ModelInfo[] = []
  for (const line of stripAnsi(stdout).split('\n')) {
    const match = /^\s*([A-Za-z0-9][\w.:/-]*)\s+-\s+(.+?)\s*$/.exec(line)
    if (match) models.push({ id: match[1] as string, name: (match[2] as string).replace(/\s*\((current|default)\)\s*$/i, '') })
  }
  return models
}

/** A whitespace table whose first row names `provider` and `model` columns (`pi --list-models`). */
export function tableModels(stdout: string): ModelInfo[] {
  const rows = stripAnsi(stdout)
    .split('\n')
    .map((line) => line.trim().split(/\s+/))
    .filter((row) => row.length > 0 && row[0] !== '')
  const header = rows[0]?.map((cell) => cell.toLowerCase()) ?? []
  const provider = header.indexOf('provider')
  const model = header.indexOf('model')
  if (provider === -1 || model === -1) return []
  return rows.slice(1).flatMap((row) => {
    const p = row[provider]
    const m = row[model]
    return p && m ? [{ id: `${p}/${m}` }] : []
  })
}

/** `{"models": [{"provider", "id", "selector"}]}` (`omp models --json`). */
export function ompModels(stdout: string): ModelInfo[] {
  const parsed = JSON.parse(stdout) as { models?: Array<{ provider?: string; id?: string; selector?: string; name?: string }> }
  return (parsed.models ?? []).flatMap((model) => {
    const id = model.selector ?? (model.provider && model.id ? `${model.provider}/${model.id}` : model.id)
    return id ? [{ id, ...(model.name ? { name: model.name } : {}) }] : []
  })
}

/** `kimi provider list --json`: providers with models and `supportEfforts`. */
export function kimiModels(stdout: string): ModelInfo[] {
  const parsed = JSON.parse(stdout) as unknown
  const providers = Array.isArray(parsed) ? parsed : ((parsed as { providers?: unknown[] }).providers ?? [])
  const models: ModelInfo[] = []
  for (const provider of providers as Array<Record<string, unknown>>) {
    const name = typeof provider['name'] === 'string' ? provider['name'] : typeof provider['id'] === 'string' ? provider['id'] : ''
    const efforts = Array.isArray(provider['supportEfforts']) ? (provider['supportEfforts'] as unknown[]).map(String) : undefined
    const list = Array.isArray(provider['models']) ? (provider['models'] as unknown[]) : []
    for (const entry of list) {
      const id = typeof entry === 'string' ? entry : ((entry as { id?: string; name?: string }).id ?? (entry as { name?: string }).name)
      if (id) models.push({ id: name && !id.includes('/') ? `${name}/${id}` : id, ...(efforts ? { efforts } : {}) })
    }
  }
  return models
}
