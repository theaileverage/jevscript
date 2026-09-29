/**
 * Adapters: what each declared capability is bound to for the next run, how
 * often the newest run called it, and the manifest check of spec section 9.4.
 * An agent bound to Claude Code shows a live tail of its tmux pane, which is
 * agent-written text and is only ever displayed.
 */
import { useEffect, useState } from 'react'

import type { CapabilityKind, IrNeed } from '../../shared/ir.ts'
import type { BindingSpec, Idea } from '../../shared/protocol.ts'
import { callCounts, type RecordingEvent } from '../../shared/recording.ts'
import { api } from '../api.ts'
import { PauseStack } from '../components/PauseStack.tsx'
import { actions, latestRun, toast, useStore } from '../store.ts'

/** Section 9's fixed verbs per kind, and the subsection that defines them. */
const VERBS: Record<CapabilityKind, { section: string; verbs: string[] }> = {
  agent: { section: '9.1', verbs: ['spawn', 'observe', 'send', 'wait', 'stop'] },
  person: { section: '9.2', verbs: ['ask', 'notify', 'take_over'] },
  llm: { section: '9.3', verbs: ['write'] },
  tool: { section: '9.4', verbs: [] },
}

export function defaultBinding(need: IrNeed): BindingSpec {
  return need.kind === 'person' ? { kind: 'toolbox' } : { kind: 'stub' }
}

export function bindingSummary(need: IrNeed, spec: BindingSpec | undefined): string {
  const binding = spec ?? defaultBinding(need)
  switch (binding.kind) {
    case 'stub':
      return `Built-in stub${need.signatures ? ` · manifest from ${need.signatures.length} declared verbs` : ''}`
    case 'subprocess':
      return `JSONL subprocess · ${binding.command || 'no command yet'}`
    case 'codex':
      return `Codex in tmux · session ${binding.session}`
    case 'claude-code':
      return `Claude Code in tmux · session ${binding.session}`
    case 'toolbox':
      return 'This toolbox · ask and take_over land in the pause stack'
  }
}

function bindingTitle(spec: BindingSpec): string {
  return { stub: 'Built-in stub', subprocess: 'JSONL subprocess', 'claude-code': 'Claude Code in tmux', codex: 'Codex in tmux', toolbox: 'This toolbox' }[spec.kind]
}

/** Person `ask` and `take_over` are pauses, not calls, so they are counted from the recorded pauses. */
function verbCounts(events: readonly RecordingEvent[], need: IrNeed): Record<string, number> {
  const counts = { ...(callCounts(events)[need.name] ?? {}) }
  if (need.kind === 'person') {
    for (const event of events) {
      if (event.event !== 'pause') continue
      const kind = String(event['kind'])
      if (kind === 'confirm') counts['ask'] = (counts['ask'] ?? 0) + 1
      if (kind === 'escalate' && String((event['payload'] as { reason?: string })?.reason) === `${need.name}.take_over`) {
        counts['take_over'] = (counts['take_over'] ?? 0) + 1
      }
    }
  }
  return counts
}

export function AdaptersScreen({ idea }: { idea: Idea }) {
  const check = useStore((s) => s.checks[idea.id])
  const run = useStore((s) => latestRun(s, idea.id))
  const cards = useStore((s) => s.stack.cards)
  const needs = check?.result?.ir?.needs ?? []
  const [selected, setSelected] = useState<string | null>(null)
  const [missing, setMissing] = useState<string[] | null>(null)
  const need = needs.find((candidate) => candidate.name === selected) ?? needs[0] ?? null
  const events = run?.events ?? []
  const openPauses = cards.filter((card) => run && card.runId === run.runId).length

  function bind(name: string, spec: BindingSpec) {
    actions.updateIdea(idea.id, { bindings: { ...idea.bindings, [name]: spec } })
  }

  async function checkManifests() {
    const manifests: Record<string, unknown> = {}
    for (const candidate of needs) {
      if (candidate.kind !== 'tool') continue
      const spec = idea.bindings[candidate.name] ?? defaultBinding(candidate)
      if (spec.kind === 'stub' && candidate.signatures) {
        manifests[candidate.name] = { verbs: Object.fromEntries(candidate.signatures.map((s) => [s.name, { returns: s.returns }])) }
      } else if (spec.kind === 'subprocess' && spec.manifest?.trim()) {
        try {
          manifests[candidate.name] = JSON.parse(spec.manifest)
        } catch {
          toast(`The manifest for ${candidate.name} is not JSON.`)
          return
        }
      }
    }
    try {
      const result = await api.request('tools.check', { fileName: idea.fileName, source: idea.source, manifests })
      setMissing(result.missing)
    } catch (error) {
      toast(error instanceof Error ? error.message : String(error))
    }
  }

  return (
    <>
      <main className="main">
        <header className="header">
          <div className="titles">
            <div className="eyebrow">
              Adapters · {idea.fileName} needs {needs.length}
            </div>
            <h1>Capabilities</h1>
          </div>
          <div className="actions">
            <button className="btn" onClick={() => void checkManifests()} disabled={!needs.some((candidate) => candidate.kind === 'tool')}>
              Check manifests
            </button>
          </div>
        </header>
        <div className="scroll">
          {needs.length === 0 ? <div className="empty">This program declares no capabilities.</div> : null}
          {needs.length > 0 ? (
            <table className="caps">
              <thead>
                <tr>
                  <th>NEEDS</th>
                  <th>KIND</th>
                  <th>BOUND TO</th>
                  <th style={{ textAlign: 'right' }}>CALLS</th>
                  <th>STATUS</th>
                </tr>
              </thead>
              <tbody>
                {needs.map((candidate) => {
                  const spec = idea.bindings[candidate.name] ?? defaultBinding(candidate)
                  const calls = Object.values(verbCounts(events, candidate)).reduce((a, b) => a + b, 0)
                  const gaps = (missing ?? []).filter((entry) => entry.startsWith(`${candidate.name}.`))
                  const status =
                    gaps.length > 0
                      ? { tone: 'err', text: `verb_missing: ${gaps.map((gap) => gap.split(' ')[0]).join(', ')}` }
                      : candidate.kind === 'person' && openPauses > 0
                        ? { tone: 'warn', text: `${openPauses} pause` }
                        : spec.kind === 'subprocess' && !spec.command.trim()
                          ? { tone: 'err', text: 'needs a command' }
                          : missing !== null && candidate.kind === 'tool'
                            ? { tone: 'ok', text: 'manifest ok' }
                            : { tone: 'ok', text: spec.kind === 'stub' ? 'stub' : 'ready' }
                  return (
                    <tr key={candidate.name} className={candidate === need ? 'active' : ''} onClick={() => setSelected(candidate.name)}>
                      <td>{candidate.name}</td>
                      <td className="muted">{candidate.kind}</td>
                      <td>
                        {bindingTitle(spec)}
                        <div className="sub">{bindingSummary(candidate, spec)}</div>
                      </td>
                      <td className="num">{calls}</td>
                      <td>
                        <span className={`dot ${status.tone}`} style={{ display: 'inline-block', marginRight: 8 }} />
                        <span className={status.tone === 'ok' ? 'green' : status.tone === 'warn' ? 'amber' : ''}>{status.text}</span>
                      </td>
                    </tr>
                  )
                })}
              </tbody>
            </table>
          ) : null}
          {need ? (
            <>
              <h5 className="eyebrow" style={{ padding: '36px 40px 14px', margin: 0 }}>
                Available to bind · {need.name}
              </h5>
              <div className="bind-cards">
                {bindChoices(need).map((choice) => {
                  const active = (idea.bindings[need.name] ?? defaultBinding(need)).kind === choice.spec.kind
                  return (
                    <button key={choice.spec.kind} className={`bind-card ${active ? 'active' : ''}`} onClick={() => !active && bind(need.name, choice.spec)}>
                      <div className="row">
                        {choice.title} <span>{choice.flag}</span>
                      </div>
                      <p>{choice.text}</p>
                    </button>
                  )
                })}
              </div>
            </>
          ) : null}
        </div>
      </main>
      {need ? <AdapterDetail idea={idea} need={need} events={events} onBind={(spec) => bind(need.name, spec)} /> : <aside className="panel" />}
    </>
  )
}

function bindChoices(need: IrNeed): { title: string; flag: string; text: string; spec: BindingSpec }[] {
  const stub = { title: 'Stub', flag: '--stub', text: 'Built-in demonstration fixture. Typed tool verbs return a value of their declared type. Visible in the recording.', spec: { kind: 'stub' } as BindingSpec }
  const subprocess = { title: 'Subprocess', flag: '--bind', text: 'Any command speaking JSONL on stdio. Checked against its manifest.', spec: { kind: 'subprocess', command: '' } as BindingSpec }
  switch (need.kind) {
    case 'agent':
      return [stub, subprocess, { title: 'Claude Code', flag: 'SDK host', text: 'Runs claude in a tmux pane. All five agent verbs.', spec: { kind: 'claude-code', session: 'jevscript', idleSeconds: 20, pollMs: 1000 } }, { title: 'Codex', flag: 'SDK host', text: 'Runs the signed-in codex CLI in a tmux pane. All five agent verbs; tool approval stays on.', spec: { kind: 'codex', session: 'jevscript', idleSeconds: 20, pollMs: 1000 } }]
    case 'person':
      return [{ title: 'This toolbox', flag: 'pause stack', text: 'ask and take_over land in the pause stack; notify shows in the run panel.', spec: { kind: 'toolbox' } }, subprocess]
    default:
      return [stub, subprocess]
  }
}

function AdapterDetail({ idea, need, events, onBind }: { idea: Idea; need: IrNeed; events: RecordingEvent[]; onBind: (spec: BindingSpec) => void }) {
  const spec = idea.bindings[need.name] ?? defaultBinding(need)
  const counts = verbCounts(events, need)
  const table = VERBS[need.kind]
  const verbs = need.kind === 'tool' ? (need.signatures?.map((signature) => signature.name) ?? Object.keys(counts)) : table.verbs
  return (
    <aside className="panel">
      <section>
        <div className="eyebrow">
          {need.name} · {need.kind}
        </div>
        <h2 style={{ margin: '6px 0 0', fontSize: 18 }}>{bindingTitle(spec)}</h2>
      </section>
      <section>
        <h5>Verbs · spec {table.section}</h5>
        {verbs.length === 0 ? <div className="small muted">An open tool: no declared verbs, nothing checked before run time.</div> : null}
        {verbs.map((verb) => (
          <div key={verb} className="check-row">
            <span>{verb}</span>
            <span className="muted">{counts[verb] ? `${counts[verb]} ${counts[verb] === 1 ? 'call' : 'calls'}` : '—'}</span>
          </div>
        ))}
      </section>
      <section>
        <h5>Options</h5>
        {spec.kind === 'stub' ? <div className="small muted">No options. Stubs are for trying a program’s control flow, not its effects.</div> : null}
        {spec.kind === 'toolbox' ? <div className="small muted">Answer this person’s pauses in the pause stack.</div> : null}
        {spec.kind === 'subprocess' ? (
          <>
            <label className="field">
              command
              <input value={spec.command} placeholder="node adapters/tree.js" onChange={(event) => onBind({ ...spec, command: event.target.value })} />
            </label>
            {need.kind === 'tool' ? (
              <label className="field">
                manifest
                <textarea value={spec.manifest ?? ''} placeholder='{ "verbs": { "diff": { "returns": "record" } } }' onChange={(event) => onBind({ ...spec, manifest: event.target.value })} />
              </label>
            ) : null}
          </>
        ) : null}
        {(spec.kind === 'claude-code' || spec.kind === 'codex') ? (
          <>
            <label className="field">
              session
              <input value={spec.session} onChange={(event) => onBind({ ...spec, session: event.target.value })} />
            </label>
            <label className="field">
              idleSeconds
              <input type="number" value={spec.idleSeconds} onChange={(event) => onBind({ ...spec, idleSeconds: Number(event.target.value) })} />
            </label>
            <label className="field">
              pollMs
              <input type="number" value={spec.pollMs} onChange={(event) => onBind({ ...spec, pollMs: Number(event.target.value) })} />
            </label>
          </>
        ) : null}
      </section>
      {need.kind === 'agent' && (spec.kind === 'claude-code' || spec.kind === 'codex') ? <PaneTail events={events} capability={need.name} /> : null}
      <PauseStack />
    </aside>
  )
}

/** The newest spawned pane of this capability, from the recorded `spawn` result. Display only. */
function PaneTail({ events, capability }: { events: RecordingEvent[]; capability: string }) {
  const spawn = [...events].reverse().find((event) => event.event === 'call' && event['capability'] === capability && event['verb'] === 'spawn')
  const pane = (spawn?.['result'] as { pane?: string } | undefined)?.pane
  const [text, setText] = useState<string | null>(null)
  useEffect(() => {
    if (!pane) return
    let stopped = false
    const poll = async () => {
      try {
        const result = await api.request('pane.tail', { pane })
        if (!stopped) setText(result.text)
      } catch (error) {
        if (!stopped) setText(`(${error instanceof Error ? error.message : String(error)})`)
      }
    }
    void poll()
    const timer = setInterval(() => void poll(), 2000)
    return () => {
      stopped = true
      clearInterval(timer)
    }
  }, [pane])
  return (
    <section>
      <h5>
        <span>Live pane · tail</span>
        {pane ? <span className="muted" style={{ textTransform: 'none', letterSpacing: 0 }}>tmux {pane}</span> : null}
      </h5>
      <div className="pane">{pane ? (text ?? 'reading…') : 'No pane yet: run the program to spawn one.'}</div>
      <p className="small amber" style={{ marginTop: 10 }}>
        Agent-written. Programs should shape it before any judgment reads it.
      </p>
    </section>
  )
}
