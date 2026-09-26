/** Antigravity agent adapter (spec section 9.1). */
import { AgentAdapter, type AgentAdapterOptions, agy } from '@jevscript/adapter-core'

/** Bind this spec section 9.1 adapter to a JavaScript host or serve it over JSONL. */
export class AntigravityAdapter extends AgentAdapter {
  constructor(options: AgentAdapterOptions = {}) {
    super(agy, options)
  }
}

export { agy } from '@jevscript/adapter-core'
export type { AgentAdapterOptions, AgentObservation } from '@jevscript/adapter-core'
