/** Rovo agent adapter (spec section 9.1). */
import { AgentAdapter, type AgentAdapterOptions, rovo } from '@jevscript/adapter-core'

/** Bind this spec section 9.1 adapter to a JavaScript host or serve it over JSONL. */
export class RovoAdapter extends AgentAdapter {
  constructor(options: AgentAdapterOptions = {}) {
    super(rovo, options)
  }
}

export { rovo } from '@jevscript/adapter-core'
export type { AgentAdapterOptions, AgentObservation } from '@jevscript/adapter-core'
