/** Devin agent adapter (spec section 9.1). */
import { AgentAdapter, type AgentAdapterOptions, devin } from '@jevscript/adapter-core'

/** Bind this spec section 9.1 adapter to a JavaScript host or serve it over JSONL. */
export class DevinAdapter extends AgentAdapter {
  constructor(options: AgentAdapterOptions = {}) {
    super(devin, options)
  }
}

export { devin } from '@jevscript/adapter-core'
export type { AgentAdapterOptions, AgentObservation } from '@jevscript/adapter-core'
