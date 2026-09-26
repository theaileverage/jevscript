/**
 * Discovery reports the installed agent CLIs and terminal backends so a
 * host can offer only what is available. Everything here is read-only:
 * binaries are resolved on `PATH` and asked for their version and, only when
 * requested, their models, each with stdin
 * detached and a timeout, because some listings are remote fetches.
 */
import { homedir } from 'node:os'
import { realpath } from 'node:fs/promises'

import { type AnyBackendOptions, type BackendProbe, discoverBackends, firstVersion } from './backends/index.ts'
import type { Harness, HarnessContext, ModelInfo } from './harness.ts'
import { HARNESSES } from './harnesses/index.ts'
import type { Exec, Files } from './process.ts'
import { defaultExec, defaultFiles, which } from './process.ts'

/** What discovery says about one agent CLI. */
export interface AgentDiscovery {
  harness: string
  title: string
  installed: boolean
  /** The binary found on `PATH`. */
  bin: string | null
  version: string | null
  /** Effort levels the adapter passes through, or `null` when the CLI has no verified effort flag. */
  efforts: string[] | null
  /** The CLI's models when `models` discovery was asked for and the CLI can list them; else `null`. */
  models: ModelInfo[] | null
  /** Where models come from, or why they cannot be listed. */
  modelsSource: string
  modelsError?: string
  verified: string
}

export interface DiscoverOptions {
  /** Also list each installed CLI's models (slower; some listings reach the network). */
  models?: boolean
  /** Only these harnesses. Defaults to every harness in the catalog. */
  harnesses?: readonly Harness[]
  exec?: Exec
  files?: Files
  home?: string
  env?: Record<string, string | undefined>
  /** Per-command timeout in milliseconds. Default 15000. */
  timeoutMs?: number
}

/** Discover one harness. */
export async function discoverHarness(harness: Harness, options: DiscoverOptions = {}): Promise<AgentDiscovery> {
  const env = options.env ?? process.env
  const exec = options.exec ?? defaultExec
  const base = {
    harness: harness.name,
    title: harness.title,
    efforts: harness.efforts ? [...harness.efforts] : null,
    verified: harness.verified,
    modelsSource: harness.models?.source ?? 'no listing command (the CLI chooses models interactively)',
  }
  const bin = await findBinary(harness, env['PATH'])
  if (!bin) return { ...base, installed: false, bin: null, version: null, models: null }

  let version: string | null = null
  try {
    const result = await exec(bin, [...(harness.versionArgs ?? ['--version'])], { timeoutMs: options.timeoutMs ?? 15_000 })
    version = result.code === 0 ? firstVersion(result.stdout || result.stderr) : null
  } catch {
    version = null
  }

  let models: ModelInfo[] | null = null
  let modelsError: string | undefined
  if (options.models && harness.models) {
    const ctx: HarnessContext = {
      exec,
      files: options.files ?? defaultFiles,
      home: options.home ?? homedir(),
      env,
      options: {},
    }
    try {
      models = await harness.models.list(bin, ctx)
    } catch (error) {
      modelsError = error instanceof Error ? error.message : String(error)
    }
  }
  return { ...base, installed: true, bin, version, models, ...(modelsError ? { modelsError } : {}) }
}

/** Discover every harness in the catalog (or `options.harnesses`), in order. */
export async function discover(options: DiscoverOptions = {}): Promise<AgentDiscovery[]> {
  return Promise.all((options.harnesses ?? HARNESSES).map((harness) => discoverHarness(harness, options)))
}

/** Agents and backends together: what a host needs to offer a choice. */
export async function discoverAll(
  options: DiscoverOptions & { backendOptions?: AnyBackendOptions } = {},
): Promise<{ agents: AgentDiscovery[]; backends: BackendProbe[] }> {
  const [agents, backends] = await Promise.all([discover(options), discoverBackends(options.backendOptions ?? {})])
  return { agents, backends }
}

/**
 * The first of a harness's binaries on `PATH`. Cursor's legacy name `agent`
 * is generic, so it counts only when it resolves into a Cursor Agent install.
 */
async function findBinary(harness: Harness, path: string | undefined): Promise<string | undefined> {
  for (const name of harness.bins) {
    const found = await which(name, path)
    if (!found) continue
    if (harness.name === 'cursor' && name === 'agent') {
      const target = await realpath(found).catch(() => found)
      if (!target.includes('cursor-agent')) continue
    }
    return found
  }
  return undefined
}
