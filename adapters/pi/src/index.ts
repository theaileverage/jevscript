/** Pi agent adapter (spec section 9.1). */
import { AgentAdapter, type AgentAdapterOptions, pi } from '@jevscript/adapter-core'

/** Bind this spec section 9.1 adapter to a JavaScript host or serve it over JSONL. */
export class PiAdapter extends AgentAdapter {
  constructor(options: AgentAdapterOptions = {}) {
    super(pi, options)
  }
}

export { pi } from '@jevscript/adapter-core'
export type { AgentAdapterOptions, AgentObservation } from '@jevscript/adapter-core'
