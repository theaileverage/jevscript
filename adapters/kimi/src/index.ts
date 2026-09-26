/** Kimi agent adapter (spec section 9.1). */
import { AgentAdapter, type AgentAdapterOptions, kimi } from '@jevscript/adapter-core'

/** Bind this spec section 9.1 adapter to a JavaScript host or serve it over JSONL. */
export class KimiAdapter extends AgentAdapter {
  constructor(options: AgentAdapterOptions = {}) {
    super(kimi, options)
  }
}

export { kimi } from '@jevscript/adapter-core'
export type { AgentAdapterOptions, AgentObservation } from '@jevscript/adapter-core'
