/**
 * Every Jev request of a run, from the recording. A selected request opens as
 * editable JSON beside its recorded answers; Resend makes a separate live
 * call and never touches the run or its recording.
 */
import { json } from '@codemirror/lang-json'
import { EditorState, RangeSetBuilder, StateField } from '@codemirror/state'
import { Decoration, EditorView, lineNumbers } from '@codemirror/view'
import { useEffect, useMemo, useRef, useState } from 'react'

import type { Idea } from '../../shared/protocol.ts'
import type { RecordingEvent } from '../../shared/recording.ts'
import { startInfo } from '../../shared/recording.ts'
import {
  answerSummary,
  describeOrigin,
  editedPaths,
  parseEditedRequest,
  type RequestEntry,
  requestEntries,
  requestText,
  type ResendRecord,
} from '../../shared/requests.ts'
import { api } from '../api.ts'
import { toast } from '../store.ts'

export function RequestsTab({ idea, events, recording }: { idea: Idea; events: RecordingEvent[]; recording: string | null }) {
  const entries = useMemo(() => requestEntries(events, startInfo(events)?.ir ?? null), [events])
  const [selected, setSelected] = useState<string | null>(null)
  const entry = entries.find((candidate) => candidate.requestId === selected) ?? entries.at(-1) ?? null
  if (entries.length === 0) return <div className="console-body muted">No Jev requests in this run yet.</div>
  return (
    <div className="requests">
      <div className="list">
        {entries.map((candidate) => (
          <button key={candidate.requestId} className={candidate === entry ? 'active' : ''} onClick={() => setSelected(candidate.requestId)}>
            <span className="n">{candidate.index}</span>
            <span>
              {describeOrigin(candidate.origin)}
              <span className="muted small"> · {candidate.request.questions.length}q</span>
            </span>
            <span className="muted">{candidate.error ? 'failed' : candidate.latencyMs !== null ? `${candidate.latencyMs} ms` : ''}</span>
          </button>
        ))}
      </div>
      {entry ? <RequestDetail key={`${recording}:${entry.requestId}`} idea={idea} entry={entry} recording={recording} /> : null}
    </div>
  )
}

function RequestDetail({ idea, entry, recording }: { idea: Idea; entry: RequestEntry; recording: string | null }) {
  const recordedText = useMemo(() => requestText(entry.request), [entry])
  const [text, setText] = useState(recordedText)
  const [history, setHistory] = useState<ResendRecord[]>([])
  const [sending, setSending] = useState(false)
  const parsed = parseEditedRequest(text)
  const edited = 'request' in parsed ? editedPaths(entry.request, parsed.request) : []

  useEffect(() => {
    if (!recording || recording.startsWith('replay:')) return
    void api
      .request('resends.list', { ideaId: idea.id, recording, requestId: entry.requestId })
      .then((result) => setHistory(result.history))
      .catch(() => setHistory([]))
  }, [idea.id, recording, entry.requestId])

  async function resend() {
    if (!recording || !('request' in parsed)) return
    setSending(true)
    try {
      const record = await api.request('resend', {
        ideaId: idea.id,
        recording,
        requestId: entry.requestId,
        request: parsed.request,
      })
      setHistory((previous) => [...previous, record])
    } catch (error) {
      toast(error instanceof Error ? error.message : String(error))
    } finally {
      setSending(false)
    }
  }

  const latest = history.at(-1)
  return (
    <>
      <div className="detail">
        <div className="small">
          <b>
            Request {entry.index} · {describeOrigin(entry.origin)} · {entry.request.questions.length}{' '}
            {entry.request.questions.length === 1 ? 'question' : 'questions'}
          </b>
          {edited.length > 0 ? <span className="amber"> · edited {edited.map((path) => path.replace(/^state\./, '')).join(', ')}</span> : null}
          {'error' in parsed ? <span className="amber"> · {parsed.error}</span> : null}
        </div>
        <JsonEditor value={text} recorded={recordedText} onChange={setText} />
      </div>
      <div className="answers">
        <b>Answers</b>
        {entry.answers.map((answer) => (
          <div key={`r-${answer.id}`} className="kv">
            <span className="muted">recorded {entry.answers.length > 1 ? answer.id : ''}</span>
            <b>{answerSummary(answer)}</b>
          </div>
        ))}
        {entry.error ? <div className="amber small">recorded: {entry.error}</div> : null}
        {latest?.answers?.map((answer) => (
          <div key={`s-${answer.id}`} className="kv">
            <span className="muted">resent {entry.answers.length > 1 ? answer.id : ''}</span>
            <b className="amber">{answerSummary(answer)}</b>
          </div>
        ))}
        {latest?.error ? <div className="amber small">resend failed: {latest.error}</div> : null}
        <div className="small muted">
          Resending is a separate live call{latest ? ` (via ${latest.via}, ${history.length} so far)` : ''}. The run and its recording don’t change.
        </div>
        <div className="buttons">
          <button className="btn primary" disabled={sending || !recording || !('request' in parsed)} onClick={() => void resend()}>
            {sending ? 'Sending…' : 'Resend'}
          </button>
          <button className="btn" disabled={text === recordedText} onClick={() => setText(recordedText)}>
            Reset
          </button>
        </div>
      </div>
    </>
  )
}

/** A JSON editor that tints every line that is not in the recorded request. */
function JsonEditor({ value, recorded, onChange }: { value: string; recorded: string; onChange: (value: string) => void }) {
  const host = useRef<HTMLDivElement>(null)
  const view = useRef<EditorView | null>(null)
  const latest = useRef(onChange)
  latest.current = onChange

  useEffect(() => {
    const original = new Map<string, number>()
    for (const line of recorded.split('\n')) original.set(line, (original.get(line) ?? 0) + 1)
    const tint = Decoration.line({ class: 'edited-line' })
    const editedLines = StateField.define({
      create: (state) => mark(state),
      update: (value, transaction) => (transaction.docChanged ? mark(transaction.state) : value),
      provide: (field) => EditorView.decorations.from(field),
    })
    function mark(state: EditorState) {
      const remaining = new Map(original)
      const builder = new RangeSetBuilder<Decoration>()
      for (let number = 1; number <= state.doc.lines; number++) {
        const line = state.doc.line(number)
        const left = remaining.get(line.text) ?? 0
        if (left > 0) remaining.set(line.text, left - 1)
        else builder.add(line.from, line.from, tint)
      }
      return builder.finish()
    }
    const editor = new EditorView({
      parent: host.current as HTMLElement,
      state: EditorState.create({
        doc: value,
        extensions: [
          lineNumbers(),
          json(),
          editedLines,
          EditorView.updateListener.of((update) => {
            if (update.docChanged) latest.current(update.state.doc.toString())
          }),
        ],
      }),
    })
    view.current = editor
    return () => editor.destroy()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [recorded])

  useEffect(() => {
    const editor = view.current
    if (editor && editor.state.doc.toString() !== value) {
      editor.dispatch({ changes: { from: 0, to: editor.state.doc.length, insert: value } })
    }
  }, [value])

  return <div ref={host} className="json" />
}
