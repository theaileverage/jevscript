/**
 * The toolbox's own state in one SQLite database, `<home>/toolbox.sqlite`:
 * ideas with their chat and bindings, pins, the index of runs per idea, and
 * resend history. Runtime recordings are not in it; they stay JSONL files
 * under `<home>/recordings` (spec section 10.3), and a run row only points at
 * one.
 *
 * The page saves whole ideas, but the server owns their run history: a run
 * ends on the server whether or not a page is open. So a save never removes a
 * run, and a run the page sends replaces the stored copy with the same id.
 *
 * Builds before this one kept the same state as JSON files under
 * `<home>/ideas`. The first open imports them in one transaction, records that
 * in `meta`, and leaves the files where they are.
 */
import { existsSync, mkdirSync, readdirSync, readFileSync } from 'node:fs'
import { join } from 'node:path'
import { DatabaseSync, type SQLInputValue } from 'node:sqlite'

import type { Pin } from '../shared/annotator.ts'
import { normalize } from '../shared/ideas.ts'
import type { Idea, RunSummary } from '../shared/protocol.ts'
import type { ResendRecord } from '../shared/requests.ts'

export { newIdea, normalize } from '../shared/ideas.ts'

/** Bump with a new entry in {@link MIGRATIONS}; `PRAGMA user_version` records how far a database has come. */
const MIGRATIONS = [
  `CREATE TABLE ideas (
     id             TEXT PRIMARY KEY,
     title          TEXT NOT NULL,
     file_name      TEXT NOT NULL,
     source         TEXT NOT NULL,
     model          TEXT NOT NULL,
     sample         INTEGER NOT NULL,
     inputs         TEXT NOT NULL,
     bindings       TEXT NOT NULL,
     messages       TEXT NOT NULL,
     claude_history TEXT NOT NULL,
     created_at     TEXT NOT NULL,
     updated_at     TEXT NOT NULL
   );
   CREATE TABLE pins (
     idea_id    TEXT NOT NULL REFERENCES ideas(id) ON DELETE CASCADE,
     id         TEXT NOT NULL,
     position   INTEGER NOT NULL,
     number     INTEGER,
     target     TEXT NOT NULL,
     query      TEXT NOT NULL,
     reply      TEXT,
     status     TEXT NOT NULL,
     created_at TEXT NOT NULL,
     PRIMARY KEY (idea_id, id)
   );
   CREATE TABLE runs (
     idea_id    TEXT NOT NULL REFERENCES ideas(id) ON DELETE CASCADE,
     run_id     TEXT NOT NULL,
     recording  TEXT NOT NULL,
     started_at TEXT NOT NULL,
     replay     INTEGER NOT NULL,
     outcome    TEXT,
     verified   INTEGER NOT NULL,
     outputs    TEXT NOT NULL,
     usage      TEXT,
     PRIMARY KEY (idea_id, run_id)
   );
   CREATE TABLE resends (
     seq        INTEGER PRIMARY KEY,
     idea_id    TEXT NOT NULL,
     recording  TEXT NOT NULL,
     request_id TEXT NOT NULL,
     record     TEXT NOT NULL
   );
   CREATE INDEX resends_by_request ON resends (idea_id, recording, request_id, seq);
   CREATE TABLE meta (
     key   TEXT PRIMARY KEY,
     value TEXT NOT NULL
   );`,
]

type Row = Record<string, SQLInputValue>

export class ToolboxStore {
  readonly path: string
  /** Ideas imported from the JSON files of an earlier build on this open, if any were. */
  readonly imported: { ideas: number; resends: number } | null
  readonly #db: DatabaseSync

  constructor(home: string) {
    mkdirSync(home, { recursive: true })
    this.path = join(home, 'toolbox.sqlite')
    this.#db = new DatabaseSync(this.path)
    this.#db.exec('PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON; PRAGMA busy_timeout = 5000;')
    this.#migrate()
    this.imported = this.#importLegacyJson(join(home, 'ideas'))
  }

  close(): void {
    this.#db.close()
  }

  async list(): Promise<Idea[]> {
    const rows = this.#db.prepare('SELECT * FROM ideas ORDER BY updated_at DESC').all() as Row[]
    return rows.map((row) => this.#idea(row))
  }

  async get(id: string): Promise<Idea | undefined> {
    const row = this.#db.prepare('SELECT * FROM ideas WHERE id = ?').get(id) as Row | undefined
    return row ? this.#idea(row) : undefined
  }

  /** Save an idea, keeping every run already recorded for it. */
  async save(idea: Idea): Promise<Idea> {
    return this.#transaction(() => {
      const saved = { ...idea, updatedAt: new Date().toISOString() }
      this.#writeIdea(saved)
      this.#db.prepare('DELETE FROM pins WHERE idea_id = ?').run(idea.id)
      saved.pins.forEach((pin, position) => this.#writePin(idea.id, pin, position))
      for (const run of saved.runs) this.#writeRun(idea.id, run)
      return this.#idea(this.#db.prepare('SELECT * FROM ideas WHERE id = ?').get(idea.id) as Row)
    })
  }

  /** Record a finished run on its idea. Idempotent by run id; undefined if the idea is gone. */
  async appendRun(id: string, summary: RunSummary): Promise<Idea | undefined> {
    return this.#transaction(() => {
      const touched = this.#db.prepare('UPDATE ideas SET updated_at = ? WHERE id = ?').run(new Date().toISOString(), id)
      if (touched.changes === 0) return undefined
      this.#writeRun(id, summary)
      return this.#idea(this.#db.prepare('SELECT * FROM ideas WHERE id = ?').get(id) as Row)
    })
  }

  /** Remove an idea with its pins, run index and resend history. Its recordings stay on disk. */
  async delete(id: string): Promise<void> {
    this.#transaction(() => {
      this.#db.prepare('DELETE FROM resends WHERE idea_id = ?').run(id)
      this.#db.prepare('DELETE FROM ideas WHERE id = ?').run(id)
    })
  }

  async appendResend(ideaId: string, record: ResendRecord): Promise<void> {
    this.#db
      .prepare('INSERT INTO resends (idea_id, recording, request_id, record) VALUES (?, ?, ?, ?)')
      .run(ideaId, record.recording, record.requestId, JSON.stringify(record))
  }

  /** Every resend of one recorded request, oldest first. */
  async resendHistory(ideaId: string, recording: string, requestId: string): Promise<ResendRecord[]> {
    const rows = this.#db
      .prepare('SELECT record FROM resends WHERE idea_id = ? AND recording = ? AND request_id = ? ORDER BY seq')
      .all(ideaId, recording, requestId) as { record: string }[]
    return rows.map((row) => JSON.parse(row.record) as ResendRecord)
  }

  #migrate(): void {
    const { user_version: version } = this.#db.prepare('PRAGMA user_version').get() as { user_version: number }
    if (version > MIGRATIONS.length) {
      throw new Error(`${this.path} is schema version ${version}, newer than this toolbox (${MIGRATIONS.length}); use a newer toolbox`)
    }
    for (const [index, sql] of MIGRATIONS.entries()) {
      if (index < version) continue
      this.#transaction(() => {
        this.#db.exec(sql)
        this.#db.exec(`PRAGMA user_version = ${index + 1}`)
      })
    }
  }

  /**
   * Import `<home>/ideas/<id>.json` and `<home>/ideas/<id>/resends/<run>/<request>.json`
   * once. Nothing is deleted, so the files remain a backup; the `meta` row
   * keeps a later open from importing them again over newer edits.
   */
  #importLegacyJson(dir: string): { ideas: number; resends: number } | null {
    if (this.#db.prepare("SELECT 1 FROM meta WHERE key = 'legacy_json_import'").get()) return null
    const counts = { ideas: 0, resends: 0 }
    this.#transaction(() => {
      if (existsSync(dir)) {
        for (const name of readdirSync(dir)) {
          if (!name.endsWith('.json')) continue
          const idea = normalize(JSON.parse(readFileSync(join(dir, name), 'utf8')) as Partial<Idea>)
          if (this.#db.prepare('SELECT 1 FROM ideas WHERE id = ?').get(idea.id)) continue
          this.#writeIdea(idea)
          idea.pins.forEach((pin, position) => this.#writePin(idea.id, pin, position))
          for (const run of idea.runs) this.#writeRun(idea.id, run)
          counts.ideas += 1
        }
        for (const ideaId of readdirSync(dir)) {
          const resends = join(dir, ideaId, 'resends')
          if (!existsSync(resends)) continue
          for (const run of readdirSync(resends)) {
            for (const file of readdirSync(join(resends, run)).filter((name) => name.endsWith('.json')).sort()) {
              for (const record of JSON.parse(readFileSync(join(resends, run, file), 'utf8')) as ResendRecord[]) {
                this.#db
                  .prepare('INSERT INTO resends (idea_id, recording, request_id, record) VALUES (?, ?, ?, ?)')
                  .run(ideaId, record.recording, record.requestId, JSON.stringify(record))
                counts.resends += 1
              }
            }
          }
        }
      }
      this.#db
        .prepare("INSERT INTO meta (key, value) VALUES ('legacy_json_import', ?)")
        .run(JSON.stringify({ at: new Date().toISOString(), from: dir, ...counts }))
    })
    return counts.ideas + counts.resends > 0 ? counts : null
  }

  #writeIdea(idea: Idea): void {
    this.#db
      .prepare(
        `INSERT INTO ideas (id, title, file_name, source, model, sample, inputs, bindings, messages, claude_history, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT (id) DO UPDATE SET
           title = excluded.title, file_name = excluded.file_name, source = excluded.source, model = excluded.model,
           sample = excluded.sample, inputs = excluded.inputs, bindings = excluded.bindings, messages = excluded.messages,
           claude_history = excluded.claude_history, updated_at = excluded.updated_at`,
      )
      .run(
        idea.id,
        idea.title,
        idea.fileName,
        idea.source,
        idea.model,
        idea.sample ? 1 : 0,
        idea.inputs,
        JSON.stringify(idea.bindings),
        JSON.stringify(idea.messages),
        JSON.stringify(idea.claudeHistory),
        idea.createdAt,
        idea.updatedAt,
      )
  }

  #writePin(ideaId: string, pin: Pin, position: number): void {
    this.#db
      .prepare('INSERT INTO pins (idea_id, id, position, number, target, query, reply, status, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)')
      .run(ideaId, pin.id, position, pin.number, JSON.stringify(pin.target), pin.query, pin.reply ? JSON.stringify(pin.reply) : null, pin.status, pin.createdAt)
  }

  #writeRun(ideaId: string, run: RunSummary): void {
    this.#db
      .prepare(
        `INSERT OR REPLACE INTO runs (idea_id, run_id, recording, started_at, replay, outcome, verified, outputs, usage)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)`,
      )
      .run(
        ideaId,
        run.runId,
        run.recording,
        run.startedAt,
        run.replay ? 1 : 0,
        run.outcome,
        run.verified ? 1 : 0,
        JSON.stringify(run.outputs),
        run.usage ? JSON.stringify(run.usage) : null,
      )
  }

  #idea(row: Row): Idea {
    const id = row['id'] as string
    const pins = this.#db.prepare('SELECT * FROM pins WHERE idea_id = ? ORDER BY position').all(id) as Row[]
    const runs = this.#db.prepare('SELECT * FROM runs WHERE idea_id = ? ORDER BY started_at, run_id').all(id) as Row[]
    return {
      id,
      title: row['title'] as string,
      fileName: row['file_name'] as string,
      source: row['source'] as string,
      model: row['model'] as string,
      sample: row['sample'] === 1,
      inputs: row['inputs'] as string,
      bindings: JSON.parse(row['bindings'] as string) as Idea['bindings'],
      messages: JSON.parse(row['messages'] as string) as Idea['messages'],
      claudeHistory: JSON.parse(row['claude_history'] as string) as unknown[],
      createdAt: row['created_at'] as string,
      updatedAt: row['updated_at'] as string,
      pins: pins.map((pin) => ({
        id: pin['id'] as string,
        number: pin['number'] as number | null,
        target: JSON.parse(pin['target'] as string) as Pin['target'],
        query: pin['query'] as string,
        reply: pin['reply'] === null ? null : (JSON.parse(pin['reply'] as string) as Pin['reply']),
        status: pin['status'] as Pin['status'],
        createdAt: pin['created_at'] as string,
      })),
      runs: runs.map((run) => ({
        runId: run['run_id'] as string,
        recording: run['recording'] as string,
        startedAt: run['started_at'] as string,
        replay: run['replay'] === 1,
        outcome: run['outcome'] as string | null,
        verified: run['verified'] === 1,
        outputs: JSON.parse(run['outputs'] as string) as Record<string, unknown>,
        usage: run['usage'] === null ? null : (JSON.parse(run['usage'] as string) as RunSummary['usage']),
      })),
    }
  }

  #transaction<T>(work: () => T): T {
    this.#db.exec('BEGIN IMMEDIATE')
    try {
      const result = work()
      this.#db.exec('COMMIT')
      return result
    } catch (error) {
      this.#db.exec('ROLLBACK')
      throw error
    }
  }
}
