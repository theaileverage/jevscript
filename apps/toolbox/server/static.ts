/** The built page from `dist`, as `pnpm start` and `pnpm demo` serve it. */
import { createReadStream, statSync } from 'node:fs'
import type { IncomingMessage, ServerResponse } from 'node:http'
import { extname, join, normalize, resolve, sep } from 'node:path'

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
    let path: string
    try {
      path = normalize(decodeURIComponent((request.url ?? '/').split('?')[0] ?? '/'))
    } catch {
      response.writeHead(400).end()
      return
    }
    const root = resolve(dist)
    let file = resolve(root, `.${sep}${path}`)
    if (file !== root && !file.startsWith(root + sep)) {
      response.writeHead(404).end()
      return
    }
    try {
      const stat = statSync(file)
      if (path !== '/' && !stat.isFile()) {
        response.writeHead(404).end()
        return
      }
      if (path === '/') file = join(root, 'index.html')
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== 'ENOENT') {
        response.writeHead(500).end()
        return
      }
      file = join(root, 'index.html')
    }
    response.setHeader('content-type', TYPES[extname(file)] ?? 'application/octet-stream')
    createReadStream(file).on('error', () => {
      if (response.headersSent) response.destroy()
      else response.writeHead(500).end()
    }).pipe(response)
  }
}
