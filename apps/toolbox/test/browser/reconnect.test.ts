import { mkdtemp } from 'node:fs/promises'
import type { IncomingMessage, ServerResponse } from 'node:http'
import { createServer } from 'node:net'
import { tmpdir } from 'node:os'

import { afterAll, describe, expect, it } from 'vitest'
import { chromium, type Browser } from 'playwright-core'
import { createServer as createViteServer } from 'vite'

import { newIdea } from '../../shared/ideas.ts'
import type { Idea } from '../../shared/protocol.ts'
import { startToolbox, type Toolbox } from '../../server/app.ts'
import { APP_ROOT } from '../../server/env.ts'
import { connect } from '../harness.ts'

let browser: Browser | null = null
let toolbox: Toolbox | null = null
let vite: Awaited<ReturnType<typeof createViteServer>> | null = null

afterAll(async () => {
  await browser?.close()
  await toolbox?.close()
  await vite?.close()
})

describe('browser request delivery across a failed connection', () => {
  it('never sends an ideas.save request after its promise rejected', async () => {
    const probe = createServer()
    await new Promise<void>((done) => probe.listen(0, '127.0.0.1', done))
    const port = (probe.address() as { port: number }).port
    await new Promise<void>((done) => probe.close(() => done()))
    const home = await mkdtemp(`${tmpdir()}/jevs-reconnect-`)
    vite = await createViteServer({ root: APP_ROOT, server: { middlewareMode: true, hmr: false }, appType: 'spa' })
    const options = { home, port, serveStatic: (request: IncomingMessage, response: ServerResponse) => vite!.middlewares(request, response) }
    toolbox = await startToolbox(options)
    const executablePath = process.env['JEVS_BROWSER']
    browser = await chromium.launch(executablePath ? { executablePath } : { channel: 'chrome' })
    const page = await browser.newPage()
    await page.goto(toolbox.url)
    await page.addScriptTag({ type: 'module', content: 'import { Api } from "/src/api.ts"; window.reviewApi = new Api()' })
    await page.waitForFunction(() => 'reviewApi' in globalThis)
    const candidate = newIdea({ title: 'must not be saved after failure' })
    await toolbox.close()
    toolbox = null
    const result = await page.evaluate(async (idea) => {
      const api = (globalThis as unknown as { reviewApi: { connect(): void; request(type: string, payload: object): Promise<unknown> } }).reviewApi
      api.connect()
      try {
        await api.request('ideas.save', { idea })
        return 'resolved'
      } catch {
        return 'rejected'
      }
    }, candidate)
    expect(result).toBe('rejected')
    toolbox = await startToolbox(options)
    await page.waitForTimeout(1800)
    const client = await connect(toolbox)
    const { ideas } = await client.request<{ ideas: Idea[] }>('ideas.list')
    expect(ideas.some((idea) => idea.id === candidate.id)).toBe(false)
    client.close()
    await page.close()
  })
})
