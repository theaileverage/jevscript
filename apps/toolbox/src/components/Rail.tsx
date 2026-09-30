import { useState } from 'react'
import { IdeaDialog } from './IdeaDialog.tsx'
import { actions, type Screen, useStore } from '../store.ts'
import { ChatIcon, CodeIcon, MachineIcon, PlugIcon, TrashIcon } from './icons.tsx'

const TOOLS: { screen: Screen; label: string; icon: () => React.JSX.Element }[] = [
  { screen: 'chat', label: 'Chat', icon: ChatIcon },
  { screen: 'playground', label: 'Playground', icon: CodeIcon },
  { screen: 'machines', label: 'Machines', icon: MachineIcon },
  { screen: 'adapters', label: 'Adapters', icon: PlugIcon },
]

export function Rail() {
  const [menu, setMenu] = useState<string | null>(null)
  const [dialog, setDialog] = useState<{ id: string; kind: 'update' | 'delete' } | null>(null)
  const screen = useStore((s) => s.screen)
  const ideas = useStore((s) => s.ideas)
  const currentId = useStore((s) => s.currentId)
  const checks = useStore((s) => s.checks)
  const needs = useStore((s) => (s.currentId ? (s.checks[s.currentId]?.result?.ir?.needs.length ?? 0) : 0))
  const cards = useStore((s) => s.stack.cards)
  const runs = useStore((s) => s.runs)
  const services = useStore((s) => s.status?.services)
  const pausedIdeas = new Set(cards.map((card) => runs[card.runId]?.ideaId))

  return (
    <nav className="rail">
      <div className="wordmark">
        <b>jevs</b> <span>toolbox</span>
      </div>
      <button className="new-idea" onClick={() => void actions.createIdea()}>
        New idea <kbd>⌘ N</kbd>
      </button>
      <div className="rail-section">
        <h4>TOOLS</h4>
        {TOOLS.map(({ screen: target, label, icon: Icon }) => (
          <button key={target} className={`rail-item ${screen === target ? 'active' : ''}`} onClick={() => actions.setScreen(target)}>
            <Icon />
            {label}
            {target === 'adapters' && needs > 0 ? <span className="count">{needs}</span> : null}
          </button>
        ))}
      </div>
      <div className="rail-section">
        <h4>IDEAS</h4>
        {ideas.map((idea) => {
          const check = checks[idea.id]
          const errors = check?.result?.diagnostics.some((d) => d.severity === 'error') || (check?.result && !check.result.ir)
          const paused = pausedIdeas.has(idea.id)
          const tone = !idea.source.trim() ? '' : errors ? 'err' : paused ? 'warn' : check?.result ? 'ok' : ''
          return (
            <div key={idea.id} className={`idea-row ${idea.id === currentId ? 'selected' : ''} ${dialog?.id === idea.id && dialog.kind === 'delete' ? 'deleting' : ''}`}>
              <button className={`idea-item ${idea.id === currentId ? 'active' : ''}`} onClick={() => actions.selectIdea(idea.id)} title={idea.title}><span className={`dot ${tone}`} /><span className="name">{idea.title}</span></button>
              <button className="idea-more" aria-label={`${idea.title}: idea actions`} aria-expanded={menu === idea.id} onClick={() => { actions.selectIdea(idea.id); setMenu(menu === idea.id ? null : idea.id) }}>⋯</button>
              {menu === idea.id ? <div className="idea-menu"><button onClick={() => { setMenu(null); setDialog({ id: idea.id, kind: 'update' }) }}>✎ Update details</button><button className="delete-option" onClick={() => { setMenu(null); setDialog({ id: idea.id, kind: 'delete' }) }}><TrashIcon /> Delete idea…</button></div> : null}
            </div>
          )
        })}
      </div>
      {services === 'demo' ? <div className="service-note">Demo. Jev and machine annotation use fixtures. Chat and source annotation use local CLIs.</div> : null}
      {dialog && ideas.find(idea => idea.id === dialog.id) ? <IdeaDialog key={`${dialog.id}:${dialog.kind}`} idea={ideas.find(idea => idea.id === dialog.id)!} kind={dialog.kind} close={() => setDialog(null)} /> : null}
    </nav>
  )
}
