/**
 * Ideas on disk: one JSON file per idea under `<home>/ideas`. Recordings sit
 * next to them under `<home>/recordings`. None of it is in the repository.
 *
 * The page saves whole ideas, but the server owns their run history: a run
 * ends on the server whether or not a page is open. So every save keeps the
 * runs already on disk, and writes to one idea are serialized.
 */
import { randomUUID } from 'node:crypto'
import { mkdir, readdir, readFile, rename, rm, writeFile } from 'node:fs/promises'
import { join } from 'node:path'

import { normalize } from '../shared/ideas.ts'
import type { Idea, RunSummary } from '../shared/protocol.ts'

export class IdeaStore {
  readonly dir: string

  constructor(home: string) {
    this.dir = join(home, 'ideas')
  }

  async list(): Promise<Idea[]> {
    await mkdir(this.dir, { recursive: true })
    const ideas: Idea[] = []
    for (const name of await readdir(this.dir)) {
      if (!name.endsWith('.json')) continue
      ideas.push(normalize(JSON.parse(await readFile(join(this.dir, name), 'utf8')) as Partial<Idea>))
    }
    return ideas.sort((a, b) => b.updatedAt.localeCompare(a.updatedAt))
  }

  async get(id: string): Promise<Idea | undefined> {
    try {
      return normalize(JSON.parse(await readFile(this.path(id), 'utf8')) as Partial<Idea>)
    } catch {
      return undefined
    }
  }

  /** Save an idea, keeping every run already recorded for it. */
  save(idea: Idea): Promise<Idea> {
    return this.#serialized(idea.id, async () => {
      const onDisk = await this.get(idea.id)
      return this.#write({ ...idea, runs: mergeRuns(onDisk?.runs ?? [], idea.runs) })
    })
  }

  /** Record a finished run on its idea. Idempotent by run id; undefined if the idea is gone. */
  appendRun(id: string, summary: RunSummary): Promise<Idea | undefined> {
    return this.#serialized(id, async () => {
      const idea = await this.get(id)
      if (!idea) return undefined
      return this.#write({ ...idea, runs: mergeRuns(idea.runs, [summary]) })
    })
  }

  readonly #queues = new Map<string, Promise<unknown>>()

  #serialized<T>(id: string, work: () => Promise<T>): Promise<T> {
    const next = (this.#queues.get(id) ?? Promise.resolve()).then(work, work)
    this.#queues.set(id, next.catch(() => undefined))
    return next
  }

  /** Write atomically, so a crash mid-save never leaves half an idea. */
  async #write(idea: Idea): Promise<Idea> {
    await mkdir(this.dir, { recursive: true })
    const saved = { ...idea, updatedAt: new Date().toISOString() }
    const temp = `${this.path(idea.id)}.${randomUUID()}.tmp`
    await writeFile(temp, JSON.stringify(saved, null, 2))
    await rename(temp, this.path(idea.id))
    return saved
  }

  async delete(id: string): Promise<void> {
    await rm(this.path(id), { force: true })
  }

  private path(id: string): string {
    if (!/^[A-Za-z0-9_-]+$/.test(id)) throw new Error(`invalid idea id \`${id}\``)
    return join(this.dir, `${id}.json`)
  }
}

/** Runs from both lists, one per run id, oldest first; a later copy of a run replaces an earlier one. */
export function mergeRuns(a: readonly RunSummary[], b: readonly RunSummary[]): RunSummary[] {
  const byId = new Map<string, RunSummary>()
  for (const run of [...a, ...b]) byId.set(run.runId, run)
  return [...byId.values()].sort((x, y) => x.startedAt.localeCompare(y.startedAt))
}

export { newIdea, normalize } from '../shared/ideas.ts'
