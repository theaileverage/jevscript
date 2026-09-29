/** Idea-owned Jev and host files, with saved source selections and annotations. */
import { useEffect, useState } from 'react'
import type { ChatAgent, Idea } from '../../shared/protocol.ts'
import { formatDiagnostic } from '../../shared/protocol.ts'
import { api } from '../api.ts'
import { agentLabel } from '../agent-label.ts'
import { actions, useStore } from '../store.ts'
import { JevEditor } from './JevEditor.tsx'

export function IdeaFiles({ idea, agent }: { idea: Idea; agent: ChatAgent }) {
  const workspace = idea.workspace
  const file = workspace.files.find(file => file.id === workspace.activeFileId) ?? workspace.files[0]
  const [name, setName] = useState('host.ts')
  const [query, setQuery] = useState('')
  const [selection, setSelection] = useState({ from: 0, to: 0 })
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const [pairCheck, setPairCheck] = useState('')
  const hostResult = useStore(s => s.hosts[idea.id])
  const check = useStore(s => s.checks[idea.id])
  useEffect(() => { setSelection({ from: 0, to: 0 }); setError('') }, [file?.id])
  if (!file) return null
  const change = (source: string) => actions.updateIdea(idea.id, {
    workspace: { ...workspace, files: workspace.files.map(candidate => candidate.id === file.id ? { ...candidate, source } : candidate) },
    ...(file.id === workspace.entryFileId ? { source } : {}),
  })
  const choose = (id: string) => actions.selectFile(idea.id, id)
  const add = () => {
    if (!name.split('/').every(part => /^[A-Za-z0-9_.-]+$/.test(part) && !['.', '..'].includes(part)) || workspace.files.some(file => file.name === name)) {
      setError('Choose a unique relative file name without parent-directory segments.')
      return
    }
    const added = { id: crypto.randomUUID(), name, kind: name.endsWith('.jev') ? 'jev' as const : 'host' as const, source: '' }
    actions.updateIdea(idea.id, { workspace: { ...workspace, files: [...workspace.files, added], activeFileId: added.id, ...(added.kind === 'jev' ? { entryFileId: added.id } : {}) }, ...(added.kind === 'jev' ? { fileName: added.name, source: '' } : {}) })
    setError('')
  }
  const annotate = async () => {
    if (!query.trim() || selection.to <= selection.from || busy) return
    setBusy(true)
    setError('')
    try {
      const result = await api.request('file.annotate', { idea: { ...idea, chatAgent: agent }, fileId: file.id, ...selection, query: query.trim() })
      actions.updateIdea(idea.id, { workspace: result.idea.workspace, chatAgent: agent })
      setQuery('')
    } catch (error) { setError(error instanceof Error ? error.message : String(error)) }
    finally { setBusy(false) }
  }
  const checkPair = async () => {
    setBusy(true)
    setError('')
    try {
      const result = await api.request('pair.check', { idea, hostFileId: file.id })
      setPairCheck(`${idea.fileName}: ${result.jev.ir ? 'Jev compile passed' : 'Jev compile failed'}. ${result.host.message}\n${result.jev.diagnostics.map(formatDiagnostic).join('\n')}`.trim())
    } catch (error) { setError(error instanceof Error ? error.message : String(error)) }
    finally { setBusy(false) }
  }
  return (
    <section className="idea-files">
      <div className="file-tabs" role="tablist" aria-label="Idea files">
        {workspace.files.map(candidate => <button key={candidate.id} role="tab" aria-selected={candidate.id === file.id} onClick={() => choose(candidate.id)}>{candidate.name} · {candidate.kind === 'jev' ? 'Jev' : 'Host'}</button>)}
      </div>
      <div className="file-tools">
        <input aria-label="New file name" value={name} onChange={event => setName(event.target.value)} />
        <button className="btn" onClick={add}>Add file</button>
        {file.kind === 'jev' ? <><button className="btn" onClick={() => void actions.check(idea.id)}>Check Jev</button><button className="btn" onClick={() => void actions.run(idea.id)}>Run Jev</button></> : <><button className="btn" disabled={busy} onClick={() => void checkPair()}>Check pair</button><button className="btn" disabled={busy} onClick={() => void actions.run(idea.id, file.id)}>Run pair</button></>}
      </div>
      {file.kind === 'jev'
        ? <JevEditor value={file.source} fileName={file.name} uri={`file:///toolbox/${idea.id}/${file.name}`} fallback={check?.result?.diagnostics ?? []} onDiagnostics={found => actions.setLspDiagnostics(idea.id, found)} onRun={() => void actions.run(idea.id)} onChange={change} onSelection={setSelection} className="workspace-editor" />
        : <textarea className="host-source" aria-label="Host file source" value={file.source} onChange={event => change(event.target.value)} onSelect={event => setSelection({ from: event.currentTarget.selectionStart, to: event.currentTarget.selectionEnd })} />}
      {file.kind === 'host' ? <div className="small muted">Run pair explicitly executes {file.name} locally. Its runIdea() call starts {idea.fileName} with the idea’s inputs, bindings and Jev profile; pauses and recordings use the normal runtime. No host source runs during drafting or checks.</div> : null}
      {pairCheck ? <div role="status" className="pair-check">{pairCheck}</div> : null}
      {hostResult ? <div role="status" className={hostResult.ok ? 'green' : 'amber'}>{hostResult.message}</div> : null}
      <div className="file-annotation">
        <div className="muted small">Select text in {file.name}, then annotate it with the selected Harness model.</div>
        <blockquote>{file.source.slice(selection.from, selection.to) || 'No text selected.'}</blockquote>
        <textarea aria-label="File annotation question" value={query} onChange={event => setQuery(event.target.value)} placeholder="Ask about the selected source…" />
        <button className="btn" disabled={busy || !query.trim() || selection.to <= selection.from} onClick={() => void annotate()}>{busy ? 'Annotating…' : 'Annotate selection'}</button>
        {error ? <div className="amber" role="alert">{error}</div> : null}
        {workspace.annotations.filter(note => note.fileId === file.id).map(note => <article key={note.id} className="file-note">
          <div className="who">{agentLabel(note.origin)}</div>
          <blockquote>{note.selected}</blockquote>
          {file.source.slice(note.from, note.to) !== note.selected ? <div className="amber small">Source changed since this annotation.</div> : null}
          <div>{note.query}</div><div>{note.reply}</div>
        </article>)}
      </div>
    </section>
  )
}
