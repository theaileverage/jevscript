import { actions, type Screen, useStore } from '../store.ts'
import { ChatIcon, CodeIcon, MachineIcon, PlugIcon } from './icons.tsx'

const TOOLS: { screen: Screen; label: string; icon: () => React.JSX.Element }[] = [
  { screen: 'chat', label: 'Chat', icon: ChatIcon },
  { screen: 'playground', label: 'Playground', icon: CodeIcon },
  { screen: 'machines', label: 'Machines', icon: MachineIcon },
  { screen: 'adapters', label: 'Adapters', icon: PlugIcon },
]

export function Rail() {
  const screen = useStore((s) => s.screen)
  const ideas = useStore((s) => s.ideas)
  const currentId = useStore((s) => s.currentId)
  const checks = useStore((s) => s.checks)
  const needs = useStore((s) => (s.currentId ? (s.checks[s.currentId]?.result?.ir?.needs.length ?? 0) : 0))
  const cards = useStore((s) => s.stack.cards)
  const runs = useStore((s) => s.runs)
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
            <button key={idea.id} className={`idea-item ${idea.id === currentId ? 'active' : ''}`} onClick={() => actions.selectIdea(idea.id)} title={idea.title}>
              <span className={`dot ${tone}`} />
              <span className="name">{idea.title}</span>
            </button>
          )
        })}
      </div>
    </nav>
  )
}
