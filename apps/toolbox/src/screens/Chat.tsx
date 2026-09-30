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
  const selectedModel = agentStatus?.models.find(model => model.id === selected.model)
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

  const fresh = idea.messages.length === 0 && !idea.source.trim()
  const usage = run ? usageSoFar(run.events) : null
  const limit = check?.result?.ir?.tasks.find((task) => task.name === 'main')?.budget.calls
  const choose = (value: string) => {
    const [harness, ...model] = value.split(':')
    actions.updateIdea(idea.id, { chatAgent: { harness: harness === 'codex' ? 'codex' : 'claude-code', model: model.join(':') } })
  }
  const modelSelect = () => (
    <select aria-label="Harness model" role="combobox" value={`${selected.harness}:${selected.model}`} disabled={sending} onChange={event => choose(event.target.value)}>
      <optgroup label="Harness">
        {!agentStatus?.models.some(model => model.id === selected.model) ? <option value={`${selected.harness}:${selected.model}`}>{label} · {selected.model} · unavailable</option> : null}
        {agents.filter(agent => agent.state === 'unavailable' && agent.harness !== selected.harness).map(agent => <option key={agent.harness} disabled>{agent.harness === 'codex' ? 'Codex' : 'Claude Code'} · unavailable</option>)}
        {agents.flatMap(agent => agent.models.map(model => <option key={`${agent.harness}:${model.id}`} value={`${agent.harness}:${model.id}`}>{agent.harness === 'codex' ? 'Codex' : 'Claude Code'} · {model.id === 'default' ? `Default · ${model.resolved}` : model.resolved}</option>))}
      </optgroup>
    </select>
  )
  const composerText = (
    <textarea rows={fresh ? 6 : 1} wrap={fresh ? 'soft' : 'off'} value={draft} placeholder="Describe an idea, or ask for a change…" onChange={event => setDraft(event.target.value)} onKeyDown={event => {
      if (event.key === 'Enter' && !event.shiftKey) { event.preventDefault(); void send() }
    }} />
  )
  const sendButton = <button className="send" disabled={!draft.trim() || sending} onClick={() => void send()} aria-label="Send"><SendIcon /></button>
  const agentState = agentStatus?.state === 'available'
    ? `${label} · ${selected.model} · signed-in CLI`
    : `${label} unavailable. ${agentStatus?.state === 'unavailable' ? agentStatus.reason : 'Discovering local CLI models…'}`
  return (
    <>
      <main className={`main chat-main ${fresh ? 'new-idea-main' : ''}`}>
        <header className="header">
          <div className="titles"><div className="eyebrow">{fresh ? 'New idea · no files yet' : `Idea · ${idea.fileName}`}</div><h1>{idea.title}</h1></div>
          <div className="meta">{idea.model}{usage && limit ? ` · ${usage.calls} of ${limit} calls` : ''}{!fresh ? <button className="refresh-agents" aria-label="Refresh agents" disabled={sending} onClick={() => void actions.refreshAgents()}>↻</button> : null}</div>
        </header>
        {fresh ? <div className="new-idea-area">
          <div className="new-idea-form">
            <div className="new-idea-composer">{composerText}<div className="new-idea-footer"><span>Drafts a .jev program and its host file</span>{sendButton}</div></div>
            <div className="harness-list"><div className="harness-catalog" role="listbox" aria-label="Harness model">{agents.map(agent => <div key={agent.harness} className="harness-group"><div className="harness-heading"><span>Harness · {agent.harness === 'codex' ? 'Codex' : 'Claude Code'}</span><span>{agent.state === 'available' ? 'live' : 'unavailable'}</span></div><div className="harness-models">{agent.models.map(model => <button key={model.id} role="option" aria-selected={selected.harness === agent.harness && selected.model === model.id} data-model={`${agent.harness}:${model.id}`} className="harness-model" title={model.name} disabled={sending} onClick={() => choose(`${agent.harness}:${model.id}`)}><span>{model.id === 'default' ? `Default · ${model.resolved}` : model.resolved}</span>{selected.harness === agent.harness && selected.model === model.id ? <span>✓</span> : null}</button>)}</div>{agent.state === 'unavailable' ? <div className="model-help">{agent.reason}</div> : null}</div>)}</div><div className="model-help">Models are listed from each harness CLI. The harness drafts and answers; Jev judgments use the idea’s Jev profile.</div><button className="refresh-models" disabled={sending} onClick={() => void actions.refreshAgents()}>Refresh agents</button></div>
          </div>
          <div className="agent-state" role="status">{agentState}</div>
        </div> : <>
          <div className="scroll"><div className="chat">
            {idea.messages.map(message => message.role === 'user' ? <div key={message.id} className="msg-user"><div className="who">You</div><div className="bubble">{message.text}</div></div> : <div key={message.id} className="msg-jev"><div className="who"><i /> <b>Jev</b><span className="reply-provenance">{agentLabel(message.origin)}</span></div>{message.text ? <div className="text">{message.text}</div> : null}{message.program ? <ProgramBlock idea={idea} message={message} run={message.id === lastProgram?.id ? run : null} /> : null}</div>)}
            {!lastProgram && idea.source.trim() ? <div className="msg-jev"><div className="who"><i /> Jev</div><div className="text">This idea’s program, as it stands.</div><ProgramBlock idea={idea} message={{ id: `current-${idea.id}`, role: 'assistant', text: '', at: idea.updatedAt, program: { source: idea.source, diagnostics: settled?.result?.diagnostics ?? [], clean: Boolean(settled?.result?.ir) && !settled?.result?.diagnostics.some(d => d.severity === 'error'), attempts: 0 } }} pending={settled?.result ? null : settled?.error ? `check failed: ${settled.error}` : 'checking…'} run={run} /></div> : null}
            {sending ? <div className="muted small">Waiting for {label} · {selected.model}…</div> : null}<div ref={bottom} />
          </div></div>
          <div className="composer">{composerText}<div className="model-chip"><span>{label}</span><b>{selectedModel?.resolved ?? selected.model}</b><span aria-hidden="true">⌄</span>{modelSelect()}</div><span className="hint">/judge /run /replay</span>{sendButton}</div>
          {agentStatus?.state !== 'available' ? <div className="composer-status"><span className="agent-state" role="status">{agentState}</span></div> : null}
        </>}
      </main>
      {fresh ? <aside className="panel idea-panel"><section><h5>Idea files</h5><h2>Nothing drafted yet</h2><div className="muted">An idea owns its files. Each file opens in the Playground with its annotations.</div></section><section><h5>Jev programs</h5><div className="file-placeholder">.jev programs appear here, one or more</div></section><section><h5>Host files</h5><div className="file-placeholder">Host scripts that bind and run them</div></section></aside> : <ChatPanel idea={idea} run={run} />}
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
        const { answers } = await api.request('judge', { fileName: idea.fileName, source: idea.source, judgment, state, model: idea.model, files: idea.workspace.files })
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
      <section className="idea-panel"><h5>Idea files</h5><IdeaFileList idea={idea} /></section>
      <PauseStack />
    </aside>
  )
}

function IdeaFileList({ idea }: { idea: Idea }) {
  return <>{(['jev', 'host'] as const).map(kind => <div key={kind} className="idea-file-group"><div className="eyebrow">{kind === 'jev' ? 'Jev programs' : 'Host files'}</div>{idea.workspace.files.filter(file => file.kind === kind).map(file => <button key={file.id} onClick={() => actions.selectFile(idea.id, file.id, true)}>{file.name}<span>{idea.workspace.annotations.filter(note => note.fileId === file.id).length || ''}</span></button>)}</div>)}</>
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
