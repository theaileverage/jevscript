/**
 * Live runs through the JavaScript SDK (spec section 11.2): each run loads its
 * program into its own `jevscript serve`, binds the adapters the user chose,
 * records to a new file, and is stepped here. A pause the host must answer
 * parks the loop until the page answers it.
 */
import { randomUUID } from 'node:crypto'
import { mkdir, readFile, rm } from 'node:fs/promises'
import { dirname, join } from 'node:path'

import { type Adapter, load, type Program, type Run } from 'jevscript'

import type { Pause, Resume } from '../shared/pauses.ts'
import { needsHost } from '../shared/pauses.ts'
import type { LiveRunState, RunSummary, ServerMessage, StartRun } from '../shared/protocol.ts'
import { formatDiagnostic } from '../shared/protocol.ts'
import { endOf, parseRecording, type RecordingEvent } from '../shared/recording.ts'
import { bindAdapter, type BoundAdapter } from './adapters.ts'
import type { Jevscript } from './jevscript.ts'

type Answer = { resume: Resume } | { abort: true }

interface Live {
  runId: string
  ideaId: string
  title: string
  recording: string
  startedAt: string
  program: Program
  sourceDir: string
  run: Run
  adapters: BoundAdapter[]
  answer: ((answer: Answer) => void) | null
  events: RecordingEvent[]
  pauses: Pause[]
  /** Settles when the drive loop has finished and the run is closed and reported. */
  settled: Promise<void>
}

/** Called once per run after it ends, before `run.ended` is broadcast. */
export type OnEnded = (ideaId: string, summary: RunSummary) => Promise<void>

export class RunManager {
  readonly #jev: Jevscript
  readonly #recordings: string
  readonly #send: (message: ServerMessage) => void
  readonly #live = new Map<string, Live>()
  /** Ideas whose run is being started: every start is a live, paid run, so one per idea at a time. */
  readonly #starting = new Set<string>()
  readonly #onEnded: OnEnded

  constructor(jev: Jevscript, home: string, send: (message: ServerMessage) => void, onEnded: OnEnded = async () => {}) {
    this.#jev = jev
    this.#recordings = join(home, 'recordings')
    this.#send = send
    this.#onEnded = onEnded
  }

  get recordingsDir(): string {
    return this.#recordings
  }

  /** A fresh recording path. The runtime refuses an existing one (spec section 10.3). */
  async newRecordingPath(title: string): Promise<string> {
    await mkdir(this.#recordings, { recursive: true })
    const slug = title.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '').slice(0, 40) || 'run'
    const stamp = new Date().toISOString().replace(/[:.]/g, '-')
    return join(this.#recordings, `${stamp}-${slug}-${randomUUID().slice(0, 8)}.jsonl`)
  }

  async start(request: StartRun): Promise<{ runId: string; recording: string }> {
    if (this.#starting.has(request.ideaId) || [...this.#live.values()].some((live) => live.ideaId === request.ideaId)) {
      throw new Error('This idea already has a run in progress. Answer or end it first.')
    }
    this.#starting.add(request.ideaId)
    try {
      return await this.#start(request)
    } finally {
      this.#starting.delete(request.ideaId)
    }
  }

  async #start(request: StartRun): Promise<{ runId: string; recording: string }> {
    const compiled = await this.#jev.compile(request.fileName, request.source)
    if (!compiled.ir) {
      const first = compiled.diagnostics.find((diagnostic) => diagnostic.severity === 'error')
      throw new Error(`the program does not compile: ${first ? formatDiagnostic(first) : 'unknown error'}`)
    }
    const path = await this.#jev.materialize(request.fileName, request.source)
    let program: Program | null = null
    const adapters: BoundAdapter[] = []
    let runId = ''
    try {
      const recording = await this.newRecordingPath(request.title)
      program = await load(path, { bin: this.#jev.bin })
      const bind: Record<string, Adapter> = {}
      for (const need of compiled.ir.needs) {
        const bound = bindAdapter(need, request.bindings[need.name], (capability, message) =>
          this.#send({ type: 'notify', runId, capability, message }),
        )
        adapters.push(bound)
        bind[need.name] = bound.adapter
      }
      const run = program.task('main').start({
        inputs: request.inputs,
        bind,
        record: recording,
        model: request.model,
        ...(request.sample ? { sample: true } : {}),
      })
      runId = await run.id()
      const live: Live = {
        runId,
        ideaId: request.ideaId,
        title: request.title,
        recording,
        startedAt: new Date().toISOString(),
        program,
        sourceDir: dirname(path),
        run,
        adapters,
        answer: null,
        events: [],
        pauses: [],
        settled: Promise.resolve(),
      }
      this.#live.set(runId, live)
      void this.#forwardEvents(live)
      live.settled = this.#drive(live)
      return { runId, recording }
    } catch (error) {
      await Promise.allSettled(adapters.map((bound) => bound.close()))
      await program?.close()
      await rm(dirname(path), { recursive: true, force: true })
      throw error
    }
  }

  resume(runId: string, payload: Resume): void {
    this.#pending(runId)({ resume: payload })
  }

  async abort(runId: string): Promise<void> {
    const live = this.#live.get(runId)
    if (!live) throw new Error(`no live run \`${runId}\``)
    if (live.answer) {
      this.#pending(runId)({ abort: true })
      return
    }
    await live.run.abort()
  }

  async inject(runId: string, capability: string, message: string): Promise<void> {
    const live = this.#live.get(runId)
    if (!live) throw new Error(`no live run \`${runId}\``)
    await live.run.inject(capability, message)
  }

  /** Every run still in progress, for a page that reconnects. */
  live(): LiveRunState[] {
    return [...this.#live.values()].map((live) => ({
      runId: live.runId,
      ideaId: live.ideaId,
      title: live.title,
      recording: live.recording,
      startedAt: live.startedAt,
      events: live.events,
      pauses: live.pauses,
      waitingOn: live.answer ? (live.pauses.at(-1) ?? null) : null,
    }))
  }

  /**
   * Shut down: abort every live run so its recording ends with `abort`, the
   * terminal `stopped` pause and `end` (spec section 10.3), then close it.
   * A run that does not settle in time is closed anyway.
   */
  async closeAll(timeoutMs = 5000): Promise<void> {
    await Promise.allSettled(
      [...this.#live.values()].map(async (live) => {
        if (live.answer) this.#pending(live.runId)({ abort: true })
        else await live.run.abort().catch(() => undefined)
        await Promise.race([live.settled, new Promise((resolve) => setTimeout(resolve, timeoutMs))])
        await this.#close(live)
      }),
    )
  }

  #pending(runId: string): (answer: Answer) => void {
    const live = this.#live.get(runId)
    const answer = live?.answer
    if (!live || !answer) throw new Error(`run \`${runId}\` has no open pause to answer`)
    live.answer = null
    return answer
  }

  async #forwardEvents(live: Live): Promise<void> {
    try {
      for await (const event of live.run.events()) {
        live.events.push(event as RecordingEvent)
        this.#send({ type: 'run.event', runId: live.runId, event: event as RecordingEvent })
      }
    } catch {
      // The drive loop reports failures; the event stream just stops.
    }
  }

  async #drive(live: Live): Promise<void> {
    const pausePush = (pause: Pause) => {
      live.pauses.push(pause)
      this.#send({ type: 'run.pause', runId: live.runId, ideaId: live.ideaId, title: live.title, pause })
    }
    try {
      for (;;) {
        const pause = (await live.run.next()) as Pause
        pausePush(pause)
        if (pause.kind === 'done' || pause.kind === 'stopped') break
        if (pause.kind === 'error' && !pause.retryable) break
        if (!needsHost(pause)) continue
        const answer = await new Promise<Answer>((resolve) => {
          live.answer = resolve
        })
        if ('abort' in answer) {
          await live.run.abort()
          const stopped = await live.run.next().catch(() => null)
          if (stopped) pausePush(stopped as Pause)
          break
        }
        await live.run.resume(answer.resume)
      }
    } catch (error) {
      this.#send({
        type: 'run.failed',
        runId: live.runId,
        ideaId: live.ideaId,
        message: error instanceof Error ? error.message : String(error),
      })
    } finally {
      await this.#close(live)
      const summary = await this.#summary(live)
      await this.#onEnded(live.ideaId, summary).catch(() => undefined)
      this.#send({ type: 'run.ended', runId: live.runId, ideaId: live.ideaId, summary })
    }
  }

  async #close(live: Live): Promise<void> {
    if (!this.#live.delete(live.runId)) return
    await Promise.allSettled(live.adapters.map((bound) => bound.close()))
    try {
      await live.program.close().catch(() => undefined)
    } finally {
      await rm(live.sourceDir, { recursive: true, force: true })
    }
  }

  async #summary(live: Live): Promise<RunSummary> {
    return summarize(live.runId, live.recording, live.startedAt, false)
  }
}

/** A run's summary, read from its recording's `end` event. */
export async function summarize(
  runId: string,
  recording: string,
  startedAt: string,
  replay: boolean,
): Promise<RunSummary> {
  let events: RecordingEvent[] = []
  try {
    events = parseRecording(await readFile(recording, 'utf8'))
  } catch {
    // A run refused before it wrote anything has no recording.
  }
  const end = endOf(events)
  return {
    runId,
    recording,
    startedAt,
    replay,
    outcome: end ? String(end['kind']) : null,
    verified: end?.['verified'] === true,
    outputs: (end?.['outputs'] as Record<string, unknown>) ?? {},
    usage: (end?.['usage'] as RunSummary['usage']) ?? null,
  }
}
