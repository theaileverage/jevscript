/**
 * Chat: describe an idea, get a checked program. Every program shown here was
 * compiled first; one that does not compile is shown with its diagnostics.
 * The right panel follows the idea's newest run: inputs, what Jev answered,
 * earlier results and the pause stack.
 */
import { useEffect, useRef, useState } from 'react'

import type { ChatAgent, ChatMessage, Idea } from '../../shared/protocol.ts'
import { formatDiagnostic } from '../../shared/protocol.ts'
import { endOf, type JevAnswer, requests, startInfo, usageSoFar } from '../../shared/recording.ts'
import { requestEntries } from '../../shared/requests.ts'
import { api } from '../api.ts'
import { agentLabel } from '../agent-label.ts'
import { IdeaFiles } from '../components/IdeaFiles.tsx'
import { JevEditor } from '../components/JevEditor.tsx'
import { PlayIcon, SendIcon } from '../components/icons.tsx'
import { PauseStack } from '../components/PauseStack.tsx'
import { confidenceThreshold, short } from '../runinfo.ts'
import { actions, latestRun, type RunView, useStore } from '../store.ts'

export function ChatScreen({ idea }: { idea: Idea }) {
  const [draft, setDraft] = useState('')
  const [sending, setSending] = useState(false)
  const bottom = useRef<HTMLDivElement>(null)
  const run = useStore((s) => latestRun(s, idea.id))
  const status = useStore((s) => s.status)
  const agents = status?.agents ?? []
  const defaultAgent = agents.find(agent => agent.state === 'available') ?? agents[0]
  const selected: ChatAgent = idea.chatAgent ?? { harness: defaultAgent?.harness ?? 'claude-code', model: defaultAgent?.models[0]?.id ?? 'default' }
  const agentStatus = agents.find(agent => agent.harness === selected.harness)
  const label = selected.harness === 'codex' ? 'Codex' : 'Claude Code'
  const check = useStore((s) => s.checks[idea.id])
  const lastProgram = [...idea.messages].reverse().find((message) => message.program)
  // The saved source counts as checked only once a result for exactly this source is in.
  const settled = check && check.source === idea.source && !check.checking ? check : null
  const stale = !settled && !check?.checking && idea.source.trim() !== ''

  useEffect(() => {
    void bottom.current?.scrollIntoView({ block: 'end' })
  }, [idea.messages.length])

  useEffect(() => {
    if (stale) void actions.check(idea.id)
  }, [stale, idea.id])

  async function send() {
    const text = draft.trim()
    if (!text || sending) return
    setDraft('')
    setSending(true)
    try {
      if (text.startsWith('/')) await command(idea, text)
      else {
        actions.updateIdea(idea.id, { chatAgent: selected })
        await actions.chat(idea.id, text)
      }
    } finally {
      setSending(false)
    }
  }

  const usage = run ? usageSoFar(run.events) : null
  const limit = check?.result?.ir?.tasks.find((task) => task.name === 'main')?.budget.calls
  return (
    <>
      <main className="main">
        <header className="header">
          <div className="titles">
            <div className="eyebrow">Idea · {idea.fileName}</div>
            <h1>{idea.title === 'Untitled idea' ? 'New idea' : idea.title}</h1>
          </div>
          <div className="meta">
            {idea.model}
            {usage && limit ? ` · ${usage.calls} of ${limit} calls` : ''}
          </div>
        </header>
        <div className="scroll">
          <div className="chat">
            {status?.services === 'demo' ? <div className="service-note">Demo services. Jev runs and machine annotation use fixtures. Chat uses the selected local CLI and can reach its real service.</div> : null}
            {idea.messages.length === 0 && !idea.source.trim() ? (
              <div className="empty" style={{ padding: 0 }}>
                Describe an idea for {label}, or paste a Jevscript program to check it. Every draft is compiled
                with <code>jevscript check</code> before you see it.
              </div>
            ) : null}
            {idea.messages.map((message) =>
              message.role === 'user' ? (
                <div key={message.id} className="msg-user">
                  <div className="who">You</div>
                  <div className="bubble">{message.text}</div>
                </div>
              ) : (
                <div key={message.id} className="msg-jev">
                  <div className="who">
                    <i /> {agentLabel(message.origin)}
                  </div>
                  {message.text ? <div className="text">{message.text}</div> : null}
                  {message.program ? (
                    <ProgramBlock idea={idea} message={message} run={message.id === lastProgram?.id ? run : null} />
                  ) : null}
                </div>
              ),
            )}
            {!lastProgram && idea.source.trim() ? (
              <div className="msg-jev">
                <div className="who">
                  <i /> Jev
                </div>
                <div className="text">This idea’s program, as it stands.</div>
                <ProgramBlock
                  idea={idea}
                  message={{
                    id: `current-${idea.id}`,
                    role: 'assistant',
                    text: '',
                    at: idea.updatedAt,
                    program: {
                      source: idea.source,
                      diagnostics: settled?.result?.diagnostics ?? [],
                      clean: Boolean(settled?.result?.ir) && !settled?.result?.diagnostics.some((d) => d.severity === 'error'),
                      attempts: 0,
                    },
                  }}
                  pending={settled?.result ? null : settled?.error ? `check failed: ${settled.error}` : 'checking…'}
                  run={run}
                />
              </div>
            ) : null}
            <IdeaFiles idea={idea} agent={selected} />
            {sending ? <div className="muted small">Waiting for {label} · {selected.model}…</div> : null}
            <div ref={bottom} />
          </div>
        </div>
        <div className="composer">
          <div className="composer-controls">
            <label>Model
              <select aria-label="Harness model" value={`${selected.harness}:${selected.model}`} disabled={sending} onChange={event => {
                const [harness, ...model] = event.target.value.split(':')
                actions.updateIdea(idea.id, { chatAgent: { harness: harness === 'codex' ? 'codex' : 'claude-code', model: model.join(':') } })
              }}>
                <optgroup label="Harness">
                  {!agentStatus?.models.some(model => model.id === selected.model) ? <option value={`${selected.harness}:${selected.model}`}>{label} · {selected.model} · unavailable</option> : null}
                  {agents.filter(agent => agent.state === 'unavailable' && agent.harness !== selected.harness).map(agent => <option key={agent.harness} disabled>{agent.harness === 'codex' ? 'Codex' : 'Claude Code'} · unavailable</option>)}
                  {agents.flatMap(agent => agent.models.map(model => <option key={`${agent.harness}:${model.id}`} value={`${agent.harness}:${model.id}`}>{agent.harness === 'codex' ? 'Codex' : 'Claude Code'} · {model.name}</option>))}
                </optgroup>
              </select>
            </label>
            <button className="btn" disabled={sending} onClick={() => void actions.refreshAgents()}>Refresh agents</button>
            <div className="agent-state" role="status">
              {agentStatus?.state === 'available' ? `${label} · ${selected.model} · signed-in CLI. Shell commands and project writes are disabled.` : `${label} unavailable. ${agentStatus?.state === 'unavailable' ? agentStatus.reason : 'Discovering local CLI models…'}`}
            </div>
          </div>
          <textarea
            rows={1}
            value={draft}
            placeholder="Describe an idea, or ask the agent to change the program…"
            onChange={(event) => setDraft(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === 'Enter' && !event.shiftKey) {
                event.preventDefault()
                void send()
              }
            }}
          />
          <span className="hint">/judge /run /replay</span>
          <button className="send" disabled={!draft.trim() || sending} onClick={() => void send()} aria-label="Send">
            <SendIcon />
          </button>
        </div>
      </main>
      <ChatPanel idea={idea} run={run} />
    </>
  )
}

/** `/run`, `/replay`, `/check`, `/judge <name> <state json>`. */
async function command(idea: Idea, text: string): Promise<void> {
  const [name, ...rest] = text.slice(1).split(/\s+/)
  const note = (reply: string) =>
    actions.updateIdea(idea.id, {
      messages: [
        ...idea.messages,
        { id: crypto.randomUUID(), role: 'user', text, at: new Date().toISOString() },
        { id: crypto.randomUUID(), role: 'assistant', text: reply, at: new Date().toISOString() },
      ],
    })
  switch (name) {
    case 'run':
      await actions.run(idea.id)
      return
    case 'replay':
      await actions.replayLast(idea.id)
      return
    case 'check': {
      const result = await actions.check(idea.id)
      note(result ? result.diagnostics.map(formatDiagnostic).join('\n') || 'jevscript check: 0 errors, 0 warnings' : 'Nothing to check.')
      return
    }
    case 'judge': {
      const [judgment, ...json] = rest
      if (!judgment) {
        note('Usage: /judge <judgment> {"param": "value"}')
        return
      }
      try {
        const state = JSON.parse(json.join(' ') || '{}') as Record<string, unknown>
        const { answers } = await api.request('judge', { fileName: idea.fileName, source: idea.source, judgment, state, model: idea.model })
        note(`${judgment}: ${JSON.stringify(answers, null, 2)}`)
      } catch (error) {
        note(`/judge failed: ${error instanceof Error ? error.message : String(error)}`)
      }
      return
    }
    default:
      note(`Unknown command /${name ?? ''}. Try /run, /replay, /check or /judge.`)
  }
}

/** `pending` stands in for the check summary while the program has no check result of its own. */
function ProgramBlock({ idea, message, run, pending = null }: { idea: Idea; message: ChatMessage; run: RunView | null; pending?: string | null }) {
  const program = message.program!
  const errors = program.diagnostics.filter((d) => d.severity === 'error').length
  const warnings = program.diagnostics.length - errors
  const current = program.source === idea.source
  const runIt = () => {
    if (!program.clean || pending !== null) return
    if (!current) actions.updateIdea(idea.id, { source: program.source })
    void actions.run(idea.id)
  }
  return (
    <div className="codeblock">
      <div className="bar-top">
        <span>
          {idea.fileName} · jevscript check:{' '}
          {pending ?? `${errors} ${errors === 1 ? 'error' : 'errors'}, ${warnings} ${warnings === 1 ? 'warning' : 'warnings'}`}
          {program.attempts > 1 ? ` · ${program.attempts - 1} repair${program.attempts > 2 ? 's' : ''}` : ''}
        </span>
        <span className="spacer" />
        <button className="btn" onClick={() => void actions.check(idea.id)}>
          Check
        </button>
        <button className="btn primary" disabled={!program.clean || pending !== null} onClick={runIt}>
          <PlayIcon /> Run <kbd>⌘↵</kbd>
        </button>
      </div>
      <JevEditor value={program.source} uri={`file:///toolbox/chat/${message.id}.jev`} fileName={idea.fileName} readOnly onRun={runIt} />
      {program.diagnostics.length > 0 ? (
        <div className="diags">
          {program.diagnostics.map((diagnostic, index) => (
            <div key={index} className={diagnostic.severity === 'error' ? 'amber' : 'muted'}>
              {formatDiagnostic(diagnostic)}
            </div>
          ))}
        </div>
      ) : null}
      {run ? <RunResult run={run} /> : null}
    </div>
  )
}

function RunResult({ run }: { run: RunView }) {
  const end = endOf(run.events)
  const last = run.pauses.at(-1)
  const views = requests(run.events)
  const questions = views.reduce((sum, view) => sum + view.questions.length, 0)
  const latency = views.reduce((sum, view) => sum + (view.latencyMs ?? 0), 0)
  const kind = end ? String(end['kind']) : (last?.kind ?? 'running')
  const outputs = (end?.['outputs'] as Record<string, unknown>) ?? {}
  const inputs = startInfo(run.events)?.inputs ?? {}
  return (
    <div className="result-card">
      <div className="head">
        <span>
          <span className={`dot ${kind === 'done' ? 'ok' : kind === 'error' || run.failed ? 'err' : 'warn'}`} style={{ display: 'inline-block', marginRight: 8 }} />
          <b>{run.replay ? `replay · ${kind}` : kind}</b>
          {end?.['verified'] ? ' · verified' : ''}
        </span>
        <span className="muted">
          {views.length} {views.length === 1 ? 'request' : 'requests'} · {questions} {questions === 1 ? 'question' : 'questions'} · {latency} ms
        </span>
      </div>
      {Object.entries({ ...inputs, ...outputs }).map(([name, value]) => (
        <div key={name}>
          {name} = {short(value)}
        </div>
      ))}
      {run.failed ? <div className="amber">{run.failed}</div> : null}
    </div>
  )
}

function ChatPanel({ idea, run }: { idea: Idea; run: RunView | null }) {
  const check = useStore((s) => s.checks[idea.id])
  const ir = startInfo(run?.events ?? [])?.ir ?? check?.result?.ir ?? null
  const entries = run ? requestEntries(run.events, ir) : []
  const latest = entries.filter((entry) => entry.answers.length > 0).at(-1)
  const threshold = confidenceThreshold(ir, latest?.origin ?? null)
  const end = run ? endOf(run.events) : undefined
  const open = run && !run.ended
  const lastPause = run?.pauses.at(-1)
  const status = !run ? null : run.replay ? 'Replay' : end ? String(end['kind']) : lastPause && lastPause.kind !== 'waiting' ? 'Paused' : 'Running'
  const earlier = [...idea.runs].reverse().filter((summary) => summary.runId !== run?.runId).slice(0, 5)

  return (
    <aside className="panel">
      <section>
        <h5>
          <span>Run{run ? ` · ${entries.length} ${entries.length === 1 ? 'request' : 'requests'}` : ''}</span>
          {status ? <span className={`pill ${status === 'Paused' ? 'amber' : status === 'done' ? '' : 'grey'}`}>{status}</span> : null}
        </h5>
        <div className="small muted" style={{ marginBottom: 8 }}>
          Inputs (JSON), read by the program’s <code>in</code>
        </div>
        <textarea className="inputs" value={idea.inputs} onChange={(event) => actions.updateIdea(idea.id, { inputs: event.target.value })} disabled={Boolean(open)} />
      </section>
      {latest ? (
        <section>
          <h5>Jev answered</h5>
          {latest.answers.map((answer) => (
            <AnswerRow key={answer.id} answer={answer} threshold={threshold} />
          ))}
        </section>
      ) : null}
      {run?.notices.length ? (
        <section>
          <h5>Told you</h5>
          {run.notices.map((notice, index) => (
            <div key={index} className="small" style={{ marginBottom: 6 }}>
              <span className="muted">{notice.capability}.notify</span> {notice.message}
            </div>
          ))}
        </section>
      ) : null}
      {earlier.length > 0 ? (
        <section>
          <h5>Earlier results</h5>
          {earlier.map((summary) => (
            <div key={summary.runId} className="check-row">
              <span className="muted">{new Date(summary.startedAt).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}</span>
              <span className={summary.outcome === 'done' ? 'green' : 'amber'}>
                {summary.outcome ?? 'unfinished'}
                {summary.verified ? ' · verified' : ''}
                {Object.keys(summary.outputs).length > 0 ? ` · ${short(summary.outputs, 40)}` : ''}
              </span>
            </div>
          ))}
        </section>
      ) : null}
      <PauseStack />
    </aside>
  )
}

export function AnswerRow({ answer, threshold }: { answer: JevAnswer; threshold: { value: number; owner: string } | null }) {
  if (answer.type === 'noul') {
    return (
      <div className="answer">
        <div className="row">
          <span className="muted">{answer.id}</span>
          <span className="value">{answer.prob >= 0.5 ? 'true' : 'false'}</span>
        </div>
        <div className="bar">
          <i style={{ width: `${answer.prob * 100}%` }} />
        </div>
        <div className="small muted">{answer.prob.toFixed(2)} probability</div>
      </div>
    )
  }
  const low = threshold !== null && answer.confidence < threshold.value
  const value = answer.type === 'choice' ? answer.label : `level ${answer.level}`
  return (
    <div className="answer">
      <div className="row">
        <span className="muted">{answer.id}</span>
        <span className="value">{value}</span>
      </div>
      <div className={`bar ${low ? 'low' : ''}`}>
        <i style={{ width: `${answer.confidence * 100}%` }} />
      </div>
      <div className={`small ${low ? 'amber' : 'muted'}`}>
        {answer.confidence.toFixed(2)} confidence
        {low ? ` · below the ${threshold.value} threshold (${threshold.owner})` : ''}
      </div>
    </div>
  )
}
