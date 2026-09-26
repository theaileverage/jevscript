/** Codex agent adapter (spec section 9.1). */
import { AgentAdapter, type AgentAdapterOptions, codex } from '@jevscript/adapter-core'

/** Bind this spec section 9.1 adapter to a JavaScript host or serve it over JSONL. */
export class CodexAdapter extends AgentAdapter {
  constructor(options: AgentAdapterOptions = {}) {
    super(codex, options)
  }
}

export { codex } from '@jevscript/adapter-core'
export type { AgentAdapterOptions, AgentObservation } from '@jevscript/adapter-core'
