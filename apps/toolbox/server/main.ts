/**
 * `pnpm dev` and `pnpm start`: load the `.env` keys, start the toolbox server,
 * and serve the page through Vite (dev) or from `dist` (production).
 */
import { createReadStream, existsSync } from 'node:fs'
import { extname, join, normalize } from 'node:path'

import { startToolbox, type ToolboxOptions } from './app.ts'
import { AnthropicModel } from './claude.ts'
import { APP_ROOT, envFiles, loadEnvFiles } from './env.ts'

const loaded = loadEnvFiles(envFiles())
const modelName = process.env['JEVS_TOOLBOX_MODEL'] ?? 'claude-opus-5-5'
const model = process.env['ANTHROPIC_API_KEY'] ? new AnthropicModel(modelName, process.env['ANTHROPIC_API_KEY']) : null
const production = process.env['NODE_ENV'] === 'production'
const port = Number(process.env['JEVS_TOOLBOX_PORT'] ?? 5178)

let serveStatic: ToolboxOptions['serveStatic']
if (production) {
  const dist = join(APP_ROOT, 'dist')
  const types: Record<string, string> = {
    '.html': 'text/html; charset=utf-8',
    '.js': 'text/javascript',
    '.css': 'text/css',
    '.woff2': 'font/woff2',
    '.woff': 'font/woff',
    '.svg': 'image/svg+xml',
  }
  serveStatic = (request, response) => {
    const path = normalize(decodeURIComponent((request.url ?? '/').split('?')[0] ?? '/')).replace(/^(\.\.[/\\])+/, '')
    let file = join(dist, path)
    if (!file.startsWith(dist) || !existsSync(file) || path === '/') file = join(dist, 'index.html')
    response.setHeader('content-type', types[extname(file)] ?? 'application/octet-stream')
    createReadStream(file).pipe(response)
  }
} else {
  const { createServer } = await import('vite')
  const vite = await createServer({ root: APP_ROOT, server: { middlewareMode: true, hmr: { port: port + 1 } }, appType: 'spa' })
  serveStatic = (request, response) => vite.middlewares(request, response)
}

const toolbox = await startToolbox({ port, model, modelName, serveStatic })
console.log(`jevs toolbox on ${toolbox.url}`)
console.log(`  keys loaded from .env: ${loaded.length > 0 ? loaded.join(', ') : 'none'}`)
console.log(`  chat drafting: ${model ? modelName : 'off (no ANTHROPIC_API_KEY); pasted programs still work'}`)
console.log(`  Jev: ${process.env['TYPESAFE_API_KEY'] ? 'TYPESAFE_API_KEY set' : 'no TYPESAFE_API_KEY; runs that ask Jev will pause with an error'}`)

const shutdown = async () => {
  await toolbox.close()
  process.exit(0)
}
process.on('SIGINT', shutdown)
process.on('SIGTERM', shutdown)
