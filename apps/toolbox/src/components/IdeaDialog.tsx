/** Idea details are separate from the files, annotations and runtime history. */
import { useEffect, useRef, useState } from 'react'
import type { Idea } from '../../shared/protocol.ts'
import { actions, useStore } from '../store.ts'

export function IdeaDialog({ idea, kind, close }: { idea: Idea; kind: 'update' | 'delete'; close: () => void }) {
  const dialog = useRef<HTMLDialogElement>(null)
  const [title, setTitle] = useState(idea.title)
  const [description, setDescription] = useState(idea.description)
  const [agent, setAgent] = useState(idea.chatAgent ? `${idea.chatAgent.harness}:${idea.chatAgent.model}` : '')
  const [model, setModel] = useState(idea.model)
  const [error, setError] = useState('')
  const [busy, setBusy] = useState(false)
  const agents = useStore(s => s.status?.agents ?? [])
  const profiles = useStore(s => s.profiles)
  useEffect(() => { dialog.current?.showModal() }, [])
  const submit = async () => {
    setBusy(true)
    setError('')
    try {
      if (kind === 'delete') await actions.deleteIdea(idea.id)
      else {
        const [harness, ...ids] = agent.split(':')
        await actions.updateDetails(idea.id, { title: title.trim(), description, model, chatAgent: agent ? { harness: harness === 'codex' ? 'codex' : 'claude-code', model: ids.join(':') } : null })
      }
      close()
    } catch (error) { setError(error instanceof Error ? error.message : String(error)) }
    finally { setBusy(false) }
  }
  return <dialog ref={dialog} className="idea-dialog" aria-labelledby="idea-dialog-title" onCancel={event => { event.preventDefault(); if (!busy) close() }}>
    <div className="eyebrow">{kind === 'delete' ? 'Delete idea' : 'Update idea'}</div>
    <h2 id="idea-dialog-title">{kind === 'delete' ? `Delete “${idea.title}”?` : idea.title}</h2>
    {kind === 'update' ? <>
      <label>Title<input aria-label="Title" value={title} onChange={event => setTitle(event.target.value)} /></label>
      <label>Description<textarea aria-label="Description" value={description} onChange={event => setDescription(event.target.value)} /></label>
      <div className="dialog-models"><label>Harness · model<select aria-label="Idea Harness model" value={agent} onChange={event => setAgent(event.target.value)}><option value="">Choose a Harness model</option>{agent && !agents.some(item => item.models.some(choice => `${item.harness}:${choice.id}` === agent)) ? <option value={agent}>{agent} · unavailable</option> : null}{agents.flatMap(item => item.models.map(choice => <option key={`${item.harness}:${choice.id}`} value={`${item.harness}:${choice.id}`}>{item.harness === 'codex' ? 'Codex' : 'Claude Code'} · {choice.id === 'default' ? `Default · ${choice.resolved}` : choice.resolved}</option>))}</select></label>
      <label>Jev profile<select aria-label="Idea Jev profile" value={model} onChange={event => setModel(event.target.value)}>{!profiles.some(profile => profile.model === model) ? <option value={model}>{model}</option> : null}{profiles.map(profile => <option key={profile.model} value={profile.model}>{profile.model}</option>)}</select></label>
      </div><div className="details-note"><b>Only these details change</b><div>Files, annotations, conversation and runs stay with this idea. Changes guide future replies; existing files keep their saved edits.</div></div>
    </> : <>
      <p>This removes this idea’s {idea.workspace.files.length} files, {idea.workspace.annotations.length + idea.pins.length} annotations, conversation, run index and resend history from the toolbox database.</p>
      <p>Recording files stay on disk. Other ideas stay intact. This cannot be undone.</p>
    </>}
    {error ? <div className="amber" role="alert">{error}</div> : null}
    <div className="dialog-actions"><button className="btn" disabled={busy} onClick={close}>Cancel</button><button className={`btn ${kind === 'delete' ? 'danger' : 'primary'}`} disabled={busy || (kind === 'update' && !title.trim())} onClick={() => void submit()}>{busy ? 'Saving…' : kind === 'delete' ? 'Delete idea' : 'Save changes'}</button></div>
  </dialog>
}
