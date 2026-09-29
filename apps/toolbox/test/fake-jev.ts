import { createServer, type Server } from 'node:http'
import type { AddressInfo } from 'node:net'
import { readFile, writeFile } from 'node:fs/promises'
import { join } from 'node:path'

import { paths } from '../server/env.ts'

/**
 * A stand-in for the TypeSafe endpoint. It keeps every raw body it receives
 * and answers a Choice with the first of `prefer` it was offered (else
 * `stay`, else the first label), a Noul with `noul`.
 */
export interface FakeJev {
  endpoint: string
  bodies: string[]
  prefer: string[]
  noul: number
  close(): Promise<void>
  /** A profiles file pointing every bundled model at this endpoint. */
  profiles(dir: string): Promise<string>
}

export async function fakeJev(prefer: string[] = []): Promise<FakeJev> {
  const fake = { bodies: [] as string[], prefer, noul: 0.9 }
  const server: Server = createServer((req, res) => {
    let body = ''
    req.on('data', (chunk: Buffer) => (body += chunk.toString('utf8')))
    req.on('end', () => {
      fake.bodies.push(body)
      const sent = JSON.parse(body) as { model: string; questions: Record<string, { type: string; criteria?: unknown }> }
      const answers: Record<string, unknown> = {}
      for (const [id, question] of Object.entries(sent.questions)) {
        if (question.type === 'noul') {
          answers[id] = { noul: fake.noul }
          continue
        }
        if (question.type === 'score') {
          const levels = (question.criteria as unknown[]).length
          answers[id] = { score: 0, probabilities: Object.fromEntries(Array.from({ length: levels }, (_, i) => [String(i), i === 0 ? 1 : 0])), confidence: 1 }
          continue
        }
        const labels = Object.keys(question.criteria as Record<string, unknown>)
        const pick = fake.prefer.find((label) => labels.includes(label)) ?? (labels.includes('stay') ? 'stay' : labels[0]!)
        const probabilities = Object.fromEntries(labels.map((label) => [label, label === pick ? 0.8 : 0.2 / (labels.length - 1)]))
        answers[id] = { choice: pick, probabilities, confidence: 0.8 }
      }
      res.setHeader('content-type', 'application/json')
      res.end(JSON.stringify({ model: sent.model, answers, usage: { input_tokens: 100, output_tokens: 0 } }))
    })
  })
  await new Promise<void>((done) => server.listen(0, '127.0.0.1', done))
  const endpoint = `http://127.0.0.1:${(server.address() as AddressInfo).port}/v1/systemone`
  return Object.assign(fake, {
    endpoint,
    close: () => new Promise<void>((done) => server.close(() => done())),
    async profiles(dir: string) {
      const bundled = JSON.parse(await readFile(paths().bundledProfiles, 'utf8')) as Record<string, unknown>[]
      const file = join(dir, 'profiles.json')
      await writeFile(file, JSON.stringify(bundled.map((profile) => ({ ...profile, endpoint }))))
      return file
    },
  })
}
