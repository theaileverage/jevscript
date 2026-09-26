/**
 * Choosing a backend by name, and probing all of them for discovery.
 */
import { AdapterError } from '../errors.ts'
import { CmuxBackend, type CmuxOptions } from './cmux.ts'
import type { BackendOptions } from './common.ts'
import { HerdrBackend, type HerdrOptions } from './herdr.ts'
import { OrcaBackend } from './orca.ts'
import { TmuxBackend } from './tmux.ts'
import { BACKEND_NAMES, type BackendName, type BackendProbe, type TerminalBackend } from './types.ts'
import { ZellijBackend } from './zellij.ts'

export { CmuxBackend, type CmuxOptions } from './cmux.ts'
export {
  type BackendContext,
  type BackendOptions,
  compareVersions,
  firstVersion,
} from './common.ts'
export { HerdrBackend, type HerdrOptions } from './herdr.ts'
export { defaultStateDir, launchScript, readExitFile, writeLaunchScript } from './launch.ts'
export { OrcaBackend } from './orca.ts'
export { TmuxBackend } from './tmux.ts'
export * from './types.ts'
export { ZellijBackend } from './zellij.ts'

/** The backend adapters use when nothing names one: Herdr. */
export const DEFAULT_BACKEND: BackendName = 'herdr'

/** Options for any backend; the backend-specific ones are ignored by the others. */
export type AnyBackendOptions = HerdrOptions & CmuxOptions & BackendOptions

/** Parse a backend name, refusing anything this package does not drive. */
export function backendName(value: unknown): BackendName {
  if (typeof value === 'string' && (BACKEND_NAMES as readonly string[]).includes(value)) return value as BackendName
  throw new AdapterError(`unknown backend \`${String(value)}\`; expected one of ${BACKEND_NAMES.join(', ')}`, false)
}

/**
 * The backend to use when a caller names none: `$JEVSCRIPT_AGENT_BACKEND`, else
 * {@link DEFAULT_BACKEND}.
 */
export function defaultBackendName(env: Record<string, string | undefined> = process.env): BackendName {
  const configured = env['JEVSCRIPT_AGENT_BACKEND']
  return configured ? backendName(configured) : DEFAULT_BACKEND
}

/** Construct a backend by name. */
export function createBackend(name: BackendName, options: AnyBackendOptions = {}): TerminalBackend {
  switch (name) {
    case 'herdr':
      return new HerdrBackend(options)
    case 'tmux':
      return new TmuxBackend(options)
    case 'cmux':
      return new CmuxBackend(options)
    case 'orca':
      return new OrcaBackend(options)
    case 'zellij':
      return new ZellijBackend(options)
  }
}

/** Probe every backend (read-only), in {@link BACKEND_NAMES} order. */
export async function discoverBackends(options: AnyBackendOptions = {}): Promise<BackendProbe[]> {
  return Promise.all(BACKEND_NAMES.map((name) => createBackend(name, withoutBin(options)).probe()))
}

function withoutBin(options: AnyBackendOptions): AnyBackendOptions {
  const { bin: _bin, ...rest } = options
  return rest
}
