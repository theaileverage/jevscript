/**
 * The page's state: ideas, runs and the pause stack, with the actions that
 * change them. One external store read through `useStore`, so every screen
 * sees the same runs and pauses.
 */
import { useSyncExternalStore } from 'react'

import type { Pin } from '../shared/annotator.ts'
import { newIdea } from '../shared/ideas.ts'
import { emptyStack, type Pause, type PauseStack, pauseReducer, type Resume } from '../shared/pauses.ts'
import type { CheckResult, Diagnostic, Idea, ProfileInfo, ReplayResult, ServerStatus } from '../shared/protocol.ts'
import type { RecordingEvent } from '../shared/recording.ts'
import { api } from './api.ts'

export type Screen = 'chat' | 'playground' | 'machines' | 'adapters'

export interface RunView {
  runId: string
  ideaId: string
  title: string
  recording: string | null
  events: RecordingEvent[]
  pauses: Pause[]
  ended: boolean
  failed: string | null
  /** A replay's view: pauses from `jevscrypt replay`, events from the recording file. */
  replay: ReplayResult | null
  notices: { capability: string; message: string }[]
  startedAt: string
}

export interface CheckState {
  source: string
  result: CheckResult | null
  error: string | null
  checking: boolean
}

export interface State {
  connected: boolean
  status: ServerStatus | null
  profiles: ProfileInfo[]
  ideas: Idea[]
  currentId: string | null
  screen: Screen
  runs: Record<string, RunView>
  stack: PauseStack
  checks: Record<string, CheckState>
  errorReference: Record<string, { meaning: string; correction: string }>
  lsp: 'connecting' | 'connected' | 'disconnected'
  /** The language server's diagnostics for the Playground document, by idea. */
  lspDiagnostics: Record<string, Diagnostic[]>
  toast: string | null
}

let state: State = {
  connected: false,
  status: null,
  profiles: [],
  ideas: [],
  currentId: null,
  screen: 'chat',
  runs: {},
  stack: emptyStack,
  checks: {},
  errorReference: {},
  lsp: 'connecting',
  lspDiagnostics: {},
  toast: null,
}
const listeners = new Set<() => void>()

function set(update: Partial<State> | ((current: State) => Partial<State>)): void {
  state = { ...state, ...(typeof update === 'function' ? update(state) : update) }
  for (const listener of listeners) listener()
}

export function getState(): State {
  return state
}

export function useStore<T>(select: (state: State) => T): T {
  return useSyncExternalStore(
    (listener) => {
      listeners.add(listener)
      return () => listeners.delete(listener)
    },
    () => select(state),
  )
}

export function useCurrentIdea(): Idea | null {
  return useStore((s) => s.ideas.find((idea) => idea.id === s.currentId) ?? null)
}

/** The run a screen shows for an idea: its newest live or loaded run. */
export function latestRun(s: State, ideaId: string | undefined): RunView | null {
  if (!ideaId) return null
  return (
    Object.values(s.runs)
      .filter((run) => run.ideaId === ideaId)
      .sort((a, b) => b.startedAt.localeCompare(a.startedAt))[0] ?? null
  )
}

export function toast(message: string): void {
  set({ toast: message })
  setTimeout(() => {
    if (state.toast === message) set({ toast: null })
  }, 5000)
}

function fail(error: unknown): void {
  toast(error instanceof Error ? error.message : String(error))
}

const saveTimers = new Map<string, ReturnType<typeof setTimeout>>()

function replaceIdea(idea: Idea): void {
  set((s) => ({ ideas: s.ideas.map((candidate) => (candidate.id === idea.id ? idea : candidate)) }))
}

export const actions = {
  async init(): Promise<void> {
    api.onConnection((open) => set({ connected: open }))
    api.onPush(onPush)
    api.connect()
    const [status, profiles, list, errorReference] = await Promise.all([
      api.request('status', {}),
      api.request('profiles', {}),
      api.request('ideas.list', {}),
      api.request('errors.reference', {}),
    ])
    set({
      status,
      profiles: profiles.profiles,
      ideas: list.ideas,
      currentId: list.ideas[0]?.id ?? null,
      errorReference,
      lsp: status.lsp.available ? 'connecting' : 'disconnected',
    })
    for (const idea of list.ideas) void actions.check(idea.id)
    const { runs: live } = await api.request('runs.live', {})
    for (const run of live) {
      set((s) => ({
        runs: {
          ...s.runs,
          [run.runId]: {
            ...emptyRun(run.runId, run.ideaId, run.title),
            ...s.runs[run.runId],
            recording: run.recording,
            // Pushes may have arrived while this request was in flight; keep whichever view is longer.
            events: run.events.length >= (s.runs[run.runId]?.events.length ?? 0) ? run.events : (s.runs[run.runId]?.events ?? []),
            pauses: run.pauses.length >= (s.runs[run.runId]?.pauses.length ?? 0) ? run.pauses : (s.runs[run.runId]?.pauses ?? []),
            startedAt: run.startedAt,
          },
        },
        stack: run.waitingOn ? pauseReducer(s.stack, { type: 'paused', runId: run.runId, source: run.title, pause: run.waitingOn }) : s.stack,
      }))
    }
    void actions.loadLastRecording(list.ideas[0]?.id)
  },

  setScreen(screen: Screen): void {
    set({ screen })
  },

  selectIdea(id: string): void {
    set({ currentId: id })
    void actions.loadLastRecording(id)
  },

  async createIdea(): Promise<void> {
    const idea = newIdea()
    set((s) => ({ ideas: [idea, ...s.ideas], currentId: idea.id, screen: 'chat' }))
    await api.request('ideas.save', { idea }).catch(fail)
  },

  async deleteIdea(id: string): Promise<void> {
    await api.request('ideas.delete', { id }).catch(fail)
    set((s) => {
      const ideas = s.ideas.filter((idea) => idea.id !== id)
      return { ideas, currentId: s.currentId === id ? (ideas[0]?.id ?? null) : s.currentId }
    })
  },

  /** Change an idea locally at once and save it shortly after. A source change re-checks. */
  updateIdea(id: string, patch: Partial<Idea>): void {
    const idea = state.ideas.find((candidate) => candidate.id === id)
    if (!idea) return
    const next = { ...idea, ...patch }
    replaceIdea(next)
    clearTimeout(saveTimers.get(id))
    saveTimers.set(
      id,
      setTimeout(() => {
        saveTimers.delete(id)
        const latest = state.ideas.find((candidate) => candidate.id === id)
        if (latest) void api.request('ideas.save', { idea: latest }).catch(fail)
      }, 400),
    )
    if (patch.source !== undefined) actions.scheduleCheck(id)
  },

  scheduleCheck: (() => {
    const timers = new Map<string, ReturnType<typeof setTimeout>>()
    return (id: string) => {
      clearTimeout(timers.get(id))
      timers.set(id, setTimeout(() => void actions.check(id), 350))
    }
  })(),

  async check(id: string): Promise<CheckResult | null> {
    const idea = state.ideas.find((candidate) => candidate.id === id)
    if (!idea) return null
    if (!idea.source.trim()) {
      set((s) => ({ checks: { ...s.checks, [id]: { source: '', result: null, error: null, checking: false } } }))
      return null
    }
    const source = idea.source
    set((s) => ({ checks: { ...s.checks, [id]: { source, result: s.checks[id]?.result ?? null, error: null, checking: true } } }))
    try {
      const result = await api.request('check', { fileName: idea.fileName, source })
      set((s) => (s.checks[id]?.source === source ? { checks: { ...s.checks, [id]: { source, result, error: null, checking: false } } } : {}))
      return result
    } catch (error) {
      set((s) => ({ checks: { ...s.checks, [id]: { source, result: null, error: String(error), checking: false } } }))
      return null
    }
  },

  async chat(id: string, text: string): Promise<void> {
    const idea = state.ideas.find((candidate) => candidate.id === id)
    if (!idea) return
    clearTimeout(saveTimers.get(id))
    const pending = { ...idea, messages: [...idea.messages, { id: 'pending', role: 'user' as const, text, at: new Date().toISOString() }] }
    replaceIdea(pending)
    try {
      const reply = await api.request('chat.send', { idea, text })
      replaceIdea(reply.idea)
      void actions.check(id)
    } catch (error) {
      replaceIdea(idea)
      fail(error)
    }
  },

  /**
   * Start `main` with the idea's source, inputs, bindings, profile and sample
   * option. One run per idea at a time: a held Enter on a focused Run button
   * repeats its click, and every start is a live, paid run.
   */
  async run(id: string): Promise<string | null> {
    const idea = state.ideas.find((candidate) => candidate.id === id)
    if (!idea) return null
    if (starting.has(id) || Object.values(state.runs).some((run) => run.ideaId === id && !run.ended && !run.replay)) {
      toast('This idea already has a run in progress. Answer or end it first.')
      return null
    }
    starting.add(id)
    try {
      return await startRun(idea)
    } finally {
      starting.delete(id)
    }
  },

  async replayLast(id: string): Promise<void> {
    await replayLast(id)
  },
  /** Show the newest recorded run of an idea after a reload. */
  async loadLastRecording(id: string | undefined): Promise<void> {
    if (!id || latestRun(state, id)) return
    const last = state.ideas.find((idea) => idea.id === id)?.runs.at(-1)
    if (!last) return
    try {
      const { events } = await api.request('recording.read', { recording: last.recording })
      set((s) => ({
        runs: {
          ...s.runs,
          [last.runId]: { ...emptyRun(last.runId, id, ''), recording: last.recording, events, ended: true, startedAt: last.startedAt },
        },
      }))
    } catch {
      // The recording was removed; the idea still lists the run.
    }
  },

  async resume(runId: string, payload: Resume): Promise<void> {
    set((s) => ({ stack: pauseReducer(s.stack, { type: 'answered', runId }) }))
    await api.request('run.resume', { runId, payload }).catch(fail)
  },

  async abort(runId: string): Promise<void> {
    set((s) => ({ stack: pauseReducer(s.stack, { type: 'answered', runId }) }))
    await api.request('run.abort', { runId }).catch(fail)
  },

  addPin(ideaId: string, pin: Pin): void {
    const idea = state.ideas.find((candidate) => candidate.id === ideaId)
    if (idea) actions.updateIdea(ideaId, { pins: [...idea.pins, pin] })
  },

  updatePin(ideaId: string, pinId: string, patch: Partial<Pin>): void {
    const idea = state.ideas.find((candidate) => candidate.id === ideaId)
    if (idea) actions.updateIdea(ideaId, { pins: idea.pins.map((pin) => (pin.id === pinId ? { ...pin, ...patch } : pin)) })
  },

  setLsp(lsp: State['lsp']): void {
    set({ lsp })
  },

  setLspDiagnostics(ideaId: string, diagnostics: Diagnostic[]): void {
    set((s) => ({ lspDiagnostics: { ...s.lspDiagnostics, [ideaId]: diagnostics } }))
  },
}


const starting = new Set<string>()

async function startRun(idea: Idea): Promise<string | null> {
  const id = idea.id
  let inputs: Record<string, unknown>
  try {
    inputs = JSON.parse(idea.inputs || '{}') as Record<string, unknown>
  } catch {
    toast('The inputs are not valid JSON.')
    return null
  }
  try {
    const started = await api.request('run.start', {
      run: {
        ideaId: id,
        title: idea.title,
        fileName: idea.fileName,
        source: idea.source,
        inputs,
        bindings: idea.bindings,
        model: idea.model,
        sample: idea.sample,
      },
    })
    set((s) => ({
      runs: {
        ...s.runs,
        [started.runId]: {
          ...emptyRun(started.runId, id, idea.title),
          ...s.runs[started.runId],
          ideaId: id,
          title: idea.title,
          recording: started.recording,
        },
      },
    }))
    return started.runId
  } catch (error) {
    fail(error)
    return null
  }
}

/** Replay the idea's newest recording through `jevscrypt replay`: no model, no adapters. */
async function replayLast(id: string): Promise<void> {
  const idea = state.ideas.find((candidate) => candidate.id === id)
  const last = idea?.runs.filter((run) => !run.replay).at(-1)
  if (!idea || !last) {
    toast('Nothing to replay yet: run the program first.')
    return
  }
  try {
    const result = await api.request('replay', { recording: last.recording })
    const runId = `replay:${last.runId}:${Date.now()}`
    set((s) => ({
      runs: {
        ...s.runs,
        [runId]: {
          ...emptyRun(runId, id, idea.title),
          recording: last.recording,
          events: result.events,
          pauses: result.pauses,
          ended: true,
          replay: result,
          failed: result.exitCode === 0 || result.exitCode === 3 ? null : result.stderr.trim() || `exit ${String(result.exitCode)}`,
        },
      },
    }))
  } catch (error) {
    fail(error)
  }
}

function emptyRun(runId: string, ideaId: string, title: string): RunView {
  return {
    runId,
    ideaId,
    title,
    recording: null,
    events: [],
    pauses: [],
    ended: false,
    failed: null,
    replay: null,
    notices: [],
    startedAt: new Date().toISOString(),
  }
}

function onPush(push: Parameters<Parameters<typeof api.onPush>[0]>[0]): void {
  const update = (runId: string, ideaId: string, change: (run: RunView) => Partial<RunView>) =>
    set((s) => {
      const run = s.runs[runId] ?? emptyRun(runId, ideaId, '')
      return { runs: { ...s.runs, [runId]: { ...run, ...change(run) } } }
    })
  switch (push.type) {
    case 'run.event':
      update(push.runId, state.runs[push.runId]?.ideaId ?? '', (run) => ({ events: [...run.events, push.event] }))
      break
    case 'run.pause':
      update(push.runId, push.ideaId, (run) => ({ pauses: [...run.pauses, push.pause], title: push.title }))
      set((s) => ({ stack: pauseReducer(s.stack, { type: 'paused', runId: push.runId, source: push.title, pause: push.pause }) }))
      break
    case 'run.failed':
      update(push.runId, push.ideaId, () => ({ failed: push.message }))
      break
    case 'run.ended': {
      update(push.runId, push.ideaId, () => ({ ended: true }))
      set((s) => ({ stack: pauseReducer(s.stack, { type: 'ended', runId: push.runId }) }))
      break
    }
    case 'notify':
      update(push.runId, state.runs[push.runId]?.ideaId ?? '', (run) => ({
        notices: [...run.notices, { capability: push.capability, message: push.message }],
      }))
      break
    case 'idea.updated': {
      // The server owns run history; keep any local edits still waiting to be saved.
      const local = state.ideas.find((candidate) => candidate.id === push.idea.id)
      replaceIdea(local ? { ...local, runs: push.idea.runs } : push.idea)
      break
    }
  }
}
