/** Toolbox host entry point over Program.task().start(), spec section 11.2. */
export const DEFAULT_HOST = `import { runIdea } from './.toolbox/host.ts'

// Uses this idea's selected Jev file, inputs, bindings and profile.
// Person pauses are answered in the toolbox; the runtime records every effect.
const result = await runIdea()
console.log(result.outputs)
`
