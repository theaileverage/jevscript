/** Grok agent adapter (spec section 9.1). */
import { AgentAdapter, type AgentAdapterOptions, grok } from '@jevscript/adapter-core'

/** Bind this spec section 9.1 adapter to a JavaScript host or serve it over JSONL. */
export class GrokAdapter extends AgentAdapter {
  constructor(options: AgentAdapterOptions = {}) {
    super(grok, options)
  }
}

export { grok } from '@jevscript/adapter-core'
export type { AgentAdapterOptions, AgentObservation } from '@jevscript/adapter-core'
