/**
 * Playground: the program in an LSP-backed editor, Check / Run / Replay last,
 * the run's console, and the tuning panel. Dials rewrite the source, which
 * stays the single source of truth, and every rewrite re-checks.
 */
import { useState } from 'react'

import { dialRange, findDials, formatDial, setDial } from '../../shared/dials.ts'
import type { Diagnostic, Idea } from '../../shared/protocol.ts'
import { formatDiagnostic } from '../../shared/protocol.ts'
import { outputLines, requests, startInfo, trail, usageSoFar } from '../../shared/recording.ts'
import { JevEditor } from '../components/JevEditor.tsx'
import { PlayIcon } from '../components/icons.tsx'
import { PauseStack } from '../components/PauseStack.tsx'
import { RequestsTab } from '../components/RequestsTab.tsx'
import { callsLimit, usd } from '../runinfo.ts'
import { actions, latestRun, type RunView, useStore } from '../store.ts'
import { bindingSummary } from './Adapters.tsx'

type Tab = 'output' | 'requests' | 'trail' | 'diagnostics'

export function PlaygroundScreen({ idea }: { idea: Idea }) {
  const run = useStore((s) => latestRun(s, idea.id))
  const check = useStore((s) => s.checks[idea.id])
  const lsp = useStore((s) => s.lsp)
  const lspDiagnostics = useStore((s) => s.lspDiagnostics[idea.id])
  const [tab, setTab] = useState<Tab>('output')
  const diagnostics: Diagnostic[] = lsp === 'connected' && lspDiagnostics ? lspDiagnostics : (check?.result?.diagnostics ?? [])
  const errors = diagnostics.some((d) => d.severity === 'error') || (check?.result !== null && check?.result !== undefined && !check.result.ir)
  const runIt = () => void actions.run(idea.id).then((id) => id && setTab('output'))

  return (
    <>
      <main className="main">
        <header className="header">
          <div className="titles">
            <div className="eyebrow">Playground</div>
            <h2>{idea.fileName}</h2>
            <div className="status-line">
              <span className={`dot ${lsp === 'connected' ? 'ok' : lsp === 'connecting' ? '' : 'err'}`} />
              {lsp === 'connected'
                ? 'jevscript lsp · diagnostics, hover, completion'
                : lsp === 'connecting'
                  ? 'jevscript lsp · connecting'
                  : 'jevscript lsp · disconnected, plain text and jevscript check diagnostics'}
            </div>
          </div>
          <div className="actions">
            <button className="btn" onClick={() => void actions.replayLast(idea.id).then(() => setTab('output'))} disabled={idea.runs.length === 0}>
              Replay last
            </button>
            <button className="btn" onClick={() => void actions.check(idea.id).then(() => setTab('diagnostics'))}>
              Check
            </button>
            <button className="btn primary" onClick={runIt} disabled={errors || !idea.source.trim() || (run !== null && !run.ended)}>
              <PlayIcon /> Run <kbd>⌘↵</kbd>
            </button>
          </div>
        </header>
        <div className="editor-area">
          <JevEditor
            value={idea.source}
            uri={`file:///toolbox/${idea.id}/${idea.fileName}`}
            fileName={idea.fileName}
            fallback={check?.result?.diagnostics ?? []}
            onChange={(source) => actions.updateIdea(idea.id, { source })}
            onDiagnostics={(found) => actions.setLspDiagnostics(idea.id, found)}
            onRun={runIt}
          />
        </div>
        <Console idea={idea} run={run} tab={tab} setTab={setTab} diagnostics={diagnostics} />
      </main>
      <TuningPanel idea={idea} />
    </>
  )
}

function Console({
  idea,
  run,
  tab,
  setTab,
  diagnostics,
}: {
  idea: Idea
  run: RunView | null
  tab: Tab
  setTab: (tab: Tab) => void
  diagnostics: Diagnostic[]
}) {
  const check = useStore((s) => s.checks[idea.id])
  const events = run?.events ?? []
  const usage = usageSoFar(events)
  const limit = callsLimit(startInfo(events)?.ir ?? check?.result?.ir)
  const requestCount = requests(events).length
  return (
    <div className="console">
      <div className="tabs">
        {(
          [
            ['output', 'Output'],
            ['requests', `Requests ${requestCount}`],
            ['trail', 'Trail'],
            ['diagnostics', `Diagnostics ${diagnostics.length}`],
          ] as const
        ).map(([id, label]) => (
          <button key={id} className={`tab ${tab === id ? 'active' : ''}`} onClick={() => setTab(id)}>
            {label}
          </button>
        ))}
        {run ? (
          <span className="status">
            {run.replay ? 'replay · no live calls · ' : ''}step {usage.steps} · {usage.calls}
            {limit !== null ? ` of ${limit}` : ''} calls · {usd(usage.usd)}
          </span>
        ) : null}
      </div>
      {tab === 'requests' ? (
        <RequestsTab idea={idea} events={events} recording={run?.recording ?? null} />
      ) : (
        <div className="console-body">
          {tab === 'output' ? (
            run ? (
              <>
                {outputLines(events).map((line) => (
                  <div key={line.seq} className={`line ${line.kind}`}>
                    {line.text}
                  </div>
                ))}
                {run.notices.map((notice, index) => (
                  <div key={`n${index}`} className="line">
                    {notice.capability}.notify {JSON.stringify(notice.message)}
                  </div>
                ))}
                {run.failed ? <div className="line error">{run.failed}</div> : null}
                {run.replay ? (
                  <div className="line end">
                    replay exit {String(run.replay.exitCode)} · {run.replay.pauses.length} pauses reproduced from {run.recording}
                  </div>
                ) : null}
              </>
            ) : (
              <span className="muted">Run the program to see its steps here.</span>
            )
          ) : tab === 'trail' ? (
            trail(events).map((record) => (
              <div key={record.step} className="line">
                {String(record.step).padStart(3)}  {record.target}.{record.action}  {record.args ? JSON.stringify(record.args) : ''}  {record.outcome}
                {record.changed ? '' : ' · unchanged'}
              </div>
            ))
          ) : diagnostics.length === 0 ? (
            <span className="muted">No diagnostics.</span>
          ) : (
            diagnostics.map((diagnostic, index) => (
              <div key={index} className={`line ${diagnostic.severity === 'error' ? 'error' : 'warning'}`}>
                {formatDiagnostic(diagnostic)}
              </div>
            ))
          )}
        </div>
      )}
    </div>
  )
}

function TuningPanel({ idea }: { idea: Idea }) {
  const profiles = useStore((s) => s.profiles)
  const check = useStore((s) => s.checks[idea.id])
  const dials = findDials(idea.source)
  const selected = profiles.find((profile) => profile.model === idea.model)
  const resolved = selected?.aliases ? profiles.find((profile) => profile.model === selected.aliases) : selected
  const units = [...new Set(dials.map((dial) => `${dial.unit} ${dial.unitName}`))]
  const needs = check?.result?.ir?.needs ?? []

  return (
    <aside className="panel">
      <section>
        <h5>Model profile</h5>
        <div className="profiles">
          {profiles.map((profile) => (
            <button key={profile.model} className={profile.model === idea.model ? 'active' : ''} onClick={() => actions.updateIdea(idea.id, { model: profile.model })} title={profile.source === 'overlay' ? 'from JEVSCRIPT_PROFILES' : 'bundled'}>
              {profile.model}
            </button>
          ))}
        </div>
        {resolved ? (
          <div className="small muted" style={{ marginTop: 10, lineHeight: 1.5 }}>
            {selected?.aliases ? `${selected.model} → ${resolved.model} · ` : ''}
            {resolved.max_questions_per_request} questions per request · {Math.round(resolved.total_tokens / 1000)}k tokens · ${resolved.price_per_million_input_usd} per M input
            {selected?.source === 'overlay' ? ' · from JEVSCRIPT_PROFILES' : ''}
          </div>
        ) : null}
      </section>
      <section>
        {units.length === 0 ? (
          <h5>Tune</h5>
        ) : null}
        {units.map((unit) => (
          <div key={unit}>
            <h5>
              <span>Tune · {unit}</span>
              <span className="green" style={{ letterSpacing: 0, textTransform: 'none' }}>
                Writes to source
              </span>
            </h5>
            {dials
              .filter((dial) => `${dial.unit} ${dial.unitName}` === unit)
              .map((dial) => {
                const range = dialRange(dial)
                const write = (value: number) => {
                  const fresh = findDials(idea.source).find((candidate) => candidate.id === dial.id)
                  if (fresh && Number.isFinite(value)) actions.updateIdea(idea.id, { source: setDial(idea.source, fresh, value) })
                }
                return (
                  <div key={dial.id} className="dial">
                    <div className="row">
                      <span>
                        {dial.clause === 'budget' ? `budget ${dial.key}` : dial.key}
                      </span>
                      <input
                        key={dial.value}
                        type="number"
                        step={range.step}
                        defaultValue={formatDial(dial, dial.value)}
                        onBlur={(event) => write(Number(event.target.value))}
                        onKeyDown={(event) => event.key === 'Enter' && write(Number(event.currentTarget.value))}
                      />
                    </div>
                    <input type="range" min={range.min} max={range.max} step={range.step} value={dial.value} onChange={(event) => write(Number(event.target.value))} />
                  </div>
                )
              })}
          </div>
        ))}
        {dials.length === 0 ? <div className="small muted">This program declares no budgets or thresholds to tune.</div> : null}
        <div className="dial" style={{ marginTop: 8 }}>
          <div className="row">
            <span>
              sample
              <div className="small muted">Draw labels by probability, recorded</div>
            </span>
            <button className={`toggle ${idea.sample ? 'on' : ''}`} aria-label="sample" onClick={() => actions.updateIdea(idea.id, { sample: !idea.sample })} />
          </div>
        </div>
      </section>
      <section>
        <h5>Inputs</h5>
        <textarea className="inputs" value={idea.inputs} onChange={(event) => actions.updateIdea(idea.id, { inputs: event.target.value })} />
      </section>
      <section>
        <h5>
          <span>Capabilities · needs</span>
          <button className="btn" style={{ padding: '2px 8px', fontSize: 12 }} onClick={() => actions.setScreen('adapters')}>
            Bind…
          </button>
        </h5>
        {needs.length === 0 ? <div className="small muted">No capabilities declared.</div> : null}
        {needs.map((need) => (
          <div key={need.name} className="need">
            <span className={`dot ${need.kind === 'person' ? 'warn' : 'ok'}`} />
            <b>{need.name}</b>
            <span className="kind">{need.kind}</span>
            <span className={`desc ${need.kind === 'person' ? 'amber' : ''}`}>{bindingSummary(need, idea.bindings[need.name])}</span>
          </div>
        ))}
      </section>
      <PauseStack />
    </aside>
  )
}
