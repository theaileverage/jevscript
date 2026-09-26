/** Gemini agent adapter (spec section 9.1). */
import { AgentAdapter, type AgentAdapterOptions, gemini } from '@jevscript/adapter-core'

/** Bind this spec section 9.1 adapter to a JavaScript host or serve it over JSONL. */
export class GeminiAdapter extends AgentAdapter {
  constructor(options: AgentAdapterOptions = {}) {
    super(gemini, options)
  }
}

export { gemini } from '@jevscript/adapter-core'
export type { AgentAdapterOptions, AgentObservation } from '@jevscript/adapter-core'
