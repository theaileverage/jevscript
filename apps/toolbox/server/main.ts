/**
 * `pnpm dev` and `pnpm start`: load the `.env` keys, start the toolbox server,
 * and serve the page through Vite (dev) or from `dist` (production).
 */
import { join } from 'node:path'

import { startToolbox, type ToolboxOptions } from './app.ts'
import { AnthropicModel } from './claude.ts'
import { APP_ROOT, envFiles, loadEnvFiles } from './env.ts'
import { serveBuilt } from './static.ts'

const loaded = loadEnvFiles(envFiles())
const modelName = process.env['JEVS_TOOLBOX_MODEL'] ?? 'claude-opus-5-5'
const model = process.env['ANTHROPIC_API_KEY'] ? new AnthropicModel(modelName, process.env['ANTHROPIC_API_KEY']) : null
const production = process.env['NODE_ENV'] === 'production'
const port = Number(process.env['JEVS_TOOLBOX_PORT'] ?? 5178)

let serveStatic: ToolboxOptions['serveStatic']
if (production) {
  serveStatic = serveBuilt(join(APP_ROOT, 'dist'))
} else {
  const { createServer } = await import('vite')
  const vite = await createServer({ root: APP_ROOT, server: { middlewareMode: true, hmr: { port: port + 1 } }, appType: 'spa' })
  serveStatic = (request, response) => vite.middlewares(request, response)
}

const toolbox = await startToolbox({ port, model, modelName, serveStatic })
console.log(`jevs toolbox on ${toolbox.url}`)
console.log(`  state: ${toolbox.database}`)
console.log(`  keys loaded from .env: ${loaded.length > 0 ? loaded.join(', ') : 'none'}`)
console.log(`  chat drafting: ${model ? modelName : 'off (no ANTHROPIC_API_KEY); pasted programs still work'}`)
console.log(`  Jev: ${process.env['TYPESAFE_API_KEY'] ? 'TYPESAFE_API_KEY set' : 'no TYPESAFE_API_KEY; runs that ask Jev will pause with an error'}`)

const shutdown = async () => {
  await toolbox.close()
  process.exit(0)
}
process.on('SIGINT', shutdown)
process.on('SIGTERM', shutdown)
