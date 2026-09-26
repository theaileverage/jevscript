/**
 * The catalog: every agent CLI this suite knows, in the order hosts list them.
 */
import type { Harness } from '../harness.ts'
import { agy } from './agy.ts'
import { claude } from './claude.ts'
import { codex } from './codex.ts'
import { cursor } from './cursor.ts'
import { devin } from './devin.ts'
import { gemini } from './gemini.ts'
import { grok } from './grok.ts'
import { kimi } from './kimi.ts'
import { muse } from './muse.ts'
import { omp } from './omp.ts'
import { opencode } from './opencode.ts'
import { pi } from './pi.ts'
import { rovo } from './rovo.ts'

export { agy, registerAgyTrust } from './agy.ts'
export { claude, claudeConfigDir, claudeStatePath, promptVisible, registerClaudeTrust } from './claude.ts'
export {
  parseTranscript as parseClaudeTranscript,
  transcriptPath as claudeTranscriptPath,
  type TranscriptSummary as ClaudeTranscriptSummary,
} from './claude-transcript.ts'
export { codex, codexHome, locateRollout, parseRollout, sessionMeta } from './codex.ts'
export { cursor, parseCursorTranscript } from './cursor.ts'
export { devin } from './devin.ts'
export { gemini } from './gemini.ts'
export { grok } from './grok.ts'
export { kimi } from './kimi.ts'
export * from './models.ts'
export { muse } from './muse.ts'
export { omp } from './omp.ts'
export { opencode } from './opencode.ts'
export { pi, piAgentDir } from './pi.ts'
export { locatePiSession, parsePiSession } from './pi-session.ts'
export { rovo } from './rovo.ts'

/** Every harness in stable catalog order. */
export const HARNESSES: readonly Harness[] = [
  claude,
  codex,
  opencode,
  pi,
  omp,
  agy,
  cursor,
  gemini,
  grok,
  kimi,
  devin,
  rovo,
  muse,
]

/** A harness by name (`claude`, `codex`, ...), or `undefined`. */
export function harness(name: string): Harness | undefined {
  return HARNESSES.find((entry) => entry.name === name)
}
