/**
 * Machines: the graph from the compiled IR, a timeline that scrubs the newest
 * run or recording, an inspector that shows each recorded step (Jev's menu,
 * the gate, what Jev saw), and the annotator.
 */
import { useRef, useState } from 'react'

import { applyEdits, describeTarget, EditError, type Pin, type PinTarget, revertEdits } from '../../shared/annotator.ts'
import { machineGraph, overlay } from '../../shared/graph.ts'
import type { Idea } from '../../shared/protocol.ts'
import { formatDiagnostic } from '../../shared/protocol.ts'
import { machineSteps, type MachineStepView, startInfo, usageSoFar } from '../../shared/recording.ts'
import { api } from '../api.ts'
import { Graph, type GraphTarget } from '../components/Graph.tsx'
import { SendIcon } from '../components/icons.tsx'
import { callsLimit, short } from '../runinfo.ts'
import { actions, latestRun, toast, useStore } from '../store.ts'

interface Popover {
  target: PinTarget
  x: number
  y: number
  pinId: string | null
}

export function MachinesScreen({ idea }: { idea: Idea }) {
  const check = useStore((s) => s.checks[idea.id])
  const run = useStore((s) => latestRun(s, idea.id))
  const ir = check?.result?.ir ?? null
  const machines = ir?.machines ?? []
  const [chosen, setChosen] = useState<string | null>(null)
  const machine = machines.find((candidate) => candidate.name === chosen) ?? machines[0] ?? null
  const events = run?.events ?? []
  const recordedIr = startInfo(events)?.ir ?? null
  const steps = machine ? machineSteps(events, recordedIr ?? ir).filter((step) => step.machine === machine.name || step.machine.endsWith(`.${machine.name}`)) : []
  const [scrub, setScrub] = useState<number | null>(null)
  const position = Math.min(scrub ?? steps.length, steps.length)
  const [popover, setPopover] = useState<Popover | null>(null)
  const wrap = useRef<HTMLDivElement>(null)

  if (!machine) {
    return (
      <>
        <main className="main">
          <header className="header">
            <div className="titles">
              <div className="eyebrow">Machine · {idea.fileName}</div>
              <h1>No machine</h1>
            </div>
          </header>
          <div className="empty">{check?.result && !ir ? 'The program does not compile; fix it in the Playground first.' : 'This program declares no machine. Machines are section 7.8 of the spec.'}</div>
        </main>
        <aside className="panel" />
      </>
    )
  }

  const graph = machineGraph(machine)
  const view = overlay(graph, steps, position)
  const step = position > 0 ? steps[position - 1] : undefined
  const usage = usageSoFar(events)
  const limit = callsLimit(recordedIr ?? ir)
  const pins = idea.pins.filter((pin) => pin.target.machine === machine.name)
  const machinePins = pins.filter((pin) => pin.target.kind === 'machine')
  const drawn = pins.filter((pin) => pin.target.kind !== 'machine' && pin.status !== 'discarded')

  function pick(target: GraphTarget, at: { x: number; y: number }) {
    const box = wrap.current?.getBoundingClientRect()
    const x = at.x - (box?.left ?? 0) + (wrap.current?.scrollLeft ?? 0)
    const y = at.y - (box?.top ?? 0) + (wrap.current?.scrollTop ?? 0)
    const full: PinTarget = target.kind === 'state' ? { kind: 'state', machine: machine!.name, state: target.state } : { kind: 'edge', machine: machine!.name, from: target.from, event: target.event }
    // An applied pin reopens too, so its edit can be reverted.
    const existing = pins.findLast((pin) => pin.status !== 'discarded' && JSON.stringify(pin.target) === JSON.stringify(full))
    setPopover({ target: full, x, y, pinId: existing?.id ?? null })
  }

  return (
    <>
      <main className="main">
        <header className="header">
          <div className="titles">
            <div className="eyebrow">Machine · {idea.fileName}</div>
            <h1>
              {machine.name}({machine.params.map((param) => param.name).join(', ')})
            </h1>
          </div>
          {machines.length > 1 ? (
            <select value={machine.name} onChange={(event) => setChosen(event.target.value)}>
              {machines.map((candidate) => (
                <option key={candidate.name}>{candidate.name}</option>
              ))}
            </select>
          ) : null}
          <div className="legend">
            <span>
              <i style={{ borderColor: '#1D5A3F' }} />
              guarded
            </span>
            <span>
              <i style={{ borderColor: '#B4BBB7' }} />
              Jev picks
            </span>
            <span>
              <i style={{ borderColor: '#C2740E', borderTopStyle: 'dashed' }} />
              risky
            </span>
          </div>
        </header>
        <div className="graph-wrap" ref={wrap}>
          <Graph graph={graph} overlay={view} pins={drawn} onPick={pick} />
          {popover ? (
            <Annotation
              idea={idea}
              popover={popover}
              pin={idea.pins.find((pin) => pin.id === popover.pinId) ?? null}
              onPin={(pin) => setPopover({ ...popover, pinId: pin.id })}
              onClose={() => setPopover(null)}
            />
          ) : null}
        </div>
        <AskBar idea={idea} machine={machine.name} pins={machinePins} drawnCount={drawn.length} />
        <div className="timeline">
          <div className="head">
            <span className="eyebrow">Steps · drag to replay</span>
            <span>
              {run ? `${steps.length} ${steps.length === 1 ? 'step' : 'steps'}${run.replay ? ' · replayed' : ''} · ${usage.calls}${limit !== null ? ` of ${limit}` : ''} calls` : 'no run yet'}
            </span>
          </div>
          {steps.length > 0 ? (
            <>
              <div className="steps">
                {steps.map((candidate) => (
                  <button
                    key={candidate.seq}
                    className={`step ${candidate.index === position ? 'now' : candidate.index < position ? 'done' : ''}`}
                    onClick={() => setScrub(candidate.index)}
                    title={`${candidate.from} → ${candidate.chosen} → ${candidate.to}`}
                  >
                    <i />
                    <span>
                      {candidate.index} {candidate.chosen}
                      {candidate.index === steps.length && position === steps.length ? ' · now' : ''}
                    </span>
                  </button>
                ))}
              </div>
              <input type="range" min={0} max={steps.length} value={position} onChange={(event) => setScrub(Number(event.target.value))} aria-label="scrub steps" />
            </>
          ) : (
            <div className="small muted">Run the program, or replay its last recording, to step through it here.</div>
          )}
        </div>
      </main>
      <Inspector step={step} />
    </>
  )
}

function Inspector({ step }: { step: MachineStepView | undefined }) {
  if (!step) {
    return (
      <aside className="panel">
        <section>
          <div className="eyebrow">Step inspector</div>
          <h2 style={{ margin: '6px 0 8px', fontSize: 20 }}>Jev’s menu</h2>
          <p className="small muted" style={{ lineHeight: 1.55 }}>
            Pick a step in the timeline. Each one shows the events whose guards passed, what Jev chose, the gate and what Jev saw, all from the recording.
          </p>
        </section>
      </aside>
    )
  }
  const moved = step.to !== step.from
  const verdictText =
    step.verdict === 'no_enabled_events'
      ? 'no enabled events · escalated'
      : step.verdict === 'stay'
        ? `stay in ${step.from}`
        : step.verdict === 'confirm'
          ? `confirm → ${step.answer ? `answered ${step.answer}` : 'asked'} → ${moved ? step.to : 'stayed'}`
          : `${step.verdict} → ${moved ? step.to : step.from}`
  return (
    <aside className="panel">
      <section>
        <div className="eyebrow">
          Step {step.index} · in {step.from}
        </div>
        <h2 style={{ margin: '6px 0 8px', fontSize: 20 }}>Jev’s menu</h2>
        <p className="small muted" style={{ lineHeight: 1.55, margin: '0 0 16px' }}>
          Only events whose guards pass are offered. Jev picks among them; code decides what happens.
        </p>
        {step.menu.map((entry) => (
          <div key={entry.event} className="menu-entry">
            <div className="row">
              <span>{entry.event}</span>
              <span className={entry.event === step.chosen ? 'green' : ''}>{entry.probability.toFixed(2)}</span>
            </div>
            <div className={`bar ${entry.risky ? 'low' : ''}`}>
              <i style={{ width: `${entry.probability * 100}%` }} />
            </div>
            <div className={`small ${entry.risky ? 'amber' : 'green'}`}>
              {entry.event === 'stay'
                ? 'nothing calls for a transition yet'
                : [entry.guard ? `guard ${entry.guard} → true` : 'no guard · Jev picks', entry.risky ? 'risky · picking it pauses to confirm' : null, entry.target ? `→ ${entry.target}` : null]
                    .filter(Boolean)
                    .join(' · ')}
            </div>
          </div>
        ))}
      </section>
      <section>
        <h5>Gate</h5>
        {step.checks.map((check) => (
          <div key={check.label} className="check-row">
            <span>
              {check.label.replace('confidence', `confidence ${step.confidence.toFixed(2)}`).replace('risk ', `risk ${check.value} `)} {check.threshold}
            </span>
            <span className={check.pass ? 'green' : 'amber'}>{check.pass ? '✓' : '✗'}</span>
          </div>
        ))}
        {step.checks.length === 0 && step.verdict !== 'no_enabled_events' ? <div className="small muted">The machine declares no thresholds.</div> : null}
        <div className={`verdict ${step.verdict === 'proceed' || step.verdict === 'stay' ? '' : 'amber'}`}>{verdictText}</div>
      </section>
      <section>
        <h5>What Jev saw</h5>
        {step.observed ? (
          <dl className="kv-grid">
            {Object.entries(step.observed).map(([key, value]) => (
              <FragmentRow key={key} name={key} value={value} />
            ))}
          </dl>
        ) : (
          <div className="small muted">No request was sent at this step.</div>
        )}
        {step.requestId ? <div className="small muted" style={{ marginTop: 10 }}>request {step.requestId}</div> : null}
      </section>
    </aside>
  )
}

function FragmentRow({ name, value }: { name: string; value: unknown }) {
  return (
    <>
      <dt>{name}</dt>
      <dd>{short(value, 400)}</dd>
    </>
  )
}

function Annotation({
  idea,
  popover,
  pin,
  onPin,
  onClose,
}: {
  idea: Idea
  popover: Popover
  pin: Pin | null
  onPin: (pin: Pin) => void
  onClose: () => void
}) {
  const [query, setQuery] = useState('')
  const [busy, setBusy] = useState(false)
  const left = Math.max(8, popover.x - 20)
  const top = popover.y + 16

  async function ask() {
    if (!query.trim()) return
    setBusy(true)
    try {
      const { pin: created } = await api.request('annotate', { idea, target: popover.target, query: query.trim() })
      actions.addPin(idea.id, created)
      onPin(created)
    } catch (error) {
      toast(error instanceof Error ? error.message : String(error))
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="popover" style={{ left, top }} onClick={(event) => event.stopPropagation()}>
      <div className="head">
        {pin?.number ? <span className="num">{pin.number}</span> : null}
        <span style={{ flex: 1 }}>{describeTarget(popover.target)}</span>
        <button className="btn" style={{ padding: '1px 8px' }} onClick={onClose}>
          ×
        </button>
      </div>
      {!pin ? (
        <>
          <textarea autoFocus value={query} placeholder="Ask about this, or ask for a change…" onChange={(event) => setQuery(event.target.value)} onKeyDown={(event) => {
            if (event.key === 'Enter' && (event.metaKey || event.ctrlKey)) void ask()
          }} />
          <div style={{ display: 'flex', justifyContent: 'flex-end', marginTop: 8 }}>
            <button className="btn primary" disabled={busy || !query.trim()} onClick={() => void ask()}>
              {busy ? 'Asking…' : 'Ask'}
            </button>
          </div>
        </>
      ) : (
        <PinBody idea={idea} pin={pin} onDone={onClose} />
      )}
    </div>
  )
}

function PinBody({ idea, pin, onDone }: { idea: Idea; pin: Pin; onDone?: () => void }) {
  const reply = pin.reply
  function apply() {
    if (reply?.kind !== 'edit') return
    try {
      actions.updateIdea(idea.id, { source: applyEdits(idea.source, reply.edits) })
      actions.updatePin(idea.id, pin.id, { status: 'applied' })
      onDone?.()
    } catch (error) {
      toast(error instanceof EditError ? `Cannot apply: ${error.message}. The source changed since this edit was checked.` : String(error))
    }
  }
  function revert() {
    if (reply?.kind !== 'edit') return
    try {
      actions.updateIdea(idea.id, { source: revertEdits(idea.source, reply.edits) })
      actions.updatePin(idea.id, pin.id, { status: 'open' })
    } catch (error) {
      toast(error instanceof EditError ? `Cannot revert: ${error.message}` : String(error))
    }
  }
  return (
    <>
      <div className="query">{pin.query}</div>
      {!reply ? null : reply.kind === 'answer' ? (
        <div style={{ lineHeight: 1.55 }}>{reply.text}</div>
      ) : reply.kind === 'error' ? (
        <div className="amber">{reply.text}</div>
      ) : (
        <>
          <div style={{ marginBottom: 8 }}>{reply.text}</div>
          {reply.refused ? (
            <div className="amber small">The edit does not apply: {reply.refused}</div>
          ) : (
            <>
              <div className="diff">
                {reply.diff.map((line, index) => (
                  <div key={index} className={line.op === '-' ? 'del' : line.op === '+' ? 'add' : ''}>
                    {line.op} {line.text.trim()}
                  </div>
                ))}
              </div>
              {reply.check ? (
                <div className="small muted" style={{ marginBottom: 10, lineHeight: 1.5 }}>
                  jevscript check: {reply.check.errors.length} {reply.check.errors.length === 1 ? 'error' : 'errors'},{' '}
                  {reply.check.newWarnings.length === 0 ? 'no new warnings' : `${reply.check.newWarnings.length} new ${reply.check.newWarnings.length === 1 ? 'warning' : 'warnings'}`}.
                  {reply.check.reachability.length > 0 ? ` ${reply.check.reachability.join(' ')}` : ''}
                  {[...reply.check.errors, ...reply.check.newWarnings].map((diagnostic, index) => (
                    <div key={index} className="amber">
                      {formatDiagnostic(diagnostic)}
                    </div>
                  ))}
                </div>
              ) : null}
              {pin.status === 'applied' ? (
                <div style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
                  <span className="green small" style={{ flex: 1 }}>
                    Applied to source.
                  </span>
                  <button className="btn" onClick={revert}>
                    Revert
                  </button>
                </div>
              ) : pin.status === 'discarded' ? (
                <div className="small muted">Discarded.</div>
              ) : (
                <div style={{ display: 'flex', gap: 8 }}>
                  <button className="btn primary" style={{ flex: 1 }} disabled={(reply.check?.errors.length ?? 1) > 0} onClick={apply}>
                    Apply to source
                  </button>
                  <button
                    className="btn"
                    onClick={() => {
                      actions.updatePin(idea.id, pin.id, { status: 'discarded' })
                      onDone?.()
                    }}
                  >
                    Discard
                  </button>
                </div>
              )}
            </>
          )}
        </>
      )}
    </>
  )
}

function AskBar({ idea, machine, pins, drawnCount }: { idea: Idea; machine: string; pins: Pin[]; drawnCount: number }) {
  const [query, setQuery] = useState('')
  const [busy, setBusy] = useState(false)
  async function ask() {
    const text = query.trim()
    if (!text || busy) return
    setBusy(true)
    try {
      const { pin } = await api.request('annotate', { idea, target: { kind: 'machine', machine }, query: text })
      actions.addPin(idea.id, pin)
      setQuery('')
    } catch (error) {
      toast(error instanceof Error ? error.message : String(error))
    } finally {
      setBusy(false)
    }
  }
  return (
    <div className="ask-bar">
      {pins
        .filter((pin) => pin.status !== 'discarded')
        .slice(-3)
        .map((pin) => (
          <div key={pin.id} className="qa">
            <i />
            <div>
              <PinBody idea={idea} pin={pin} />
            </div>
          </div>
        ))}
      <div className="ask">
        <input
          value={query}
          placeholder="Ask about the whole machine, or click an edge or state to annotate…"
          onChange={(event) => setQuery(event.target.value)}
          onKeyDown={(event) => event.key === 'Enter' && void ask()}
        />
        <span className="small muted">
          {drawnCount} {drawnCount === 1 ? 'annotation' : 'annotations'}
        </span>
        <button className="send" disabled={busy || !query.trim()} onClick={() => void ask()} aria-label="Ask">
          <SendIcon />
        </button>
      </div>
    </div>
  )
}
