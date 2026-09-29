/** The built page from `dist`, as `pnpm start` and `pnpm demo` serve it. */
import { createReadStream, existsSync } from 'node:fs'
import type { IncomingMessage, ServerResponse } from 'node:http'
import { extname, join, normalize } from 'node:path'

const TYPES: Record<string, string> = {
  '.html': 'text/html; charset=utf-8',
  '.js': 'text/javascript',
  '.css': 'text/css',
  '.woff2': 'font/woff2',
  '.woff': 'font/woff',
  '.svg': 'image/svg+xml',
}

/** Any path that is not a file in `dist` gets `index.html`, so the page's own routes load. */
export function serveBuilt(dist: string): (request: IncomingMessage, response: ServerResponse) => void {
  return (request, response) => {
    const path = normalize(decodeURIComponent((request.url ?? '/').split('?')[0] ?? '/')).replace(/^(\.\.[/\\])+/, '')
    let file = join(dist, path)
    if (!file.startsWith(dist) || !existsSync(file) || path === '/') file = join(dist, 'index.html')
    response.setHeader('content-type', TYPES[extname(file)] ?? 'application/octet-stream')
    createReadStream(file).pipe(response)
  }
}
