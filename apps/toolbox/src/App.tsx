import { useEffect } from 'react'

import { Rail } from './components/Rail.tsx'
import { AdaptersScreen } from './screens/Adapters.tsx'
import { ChatScreen } from './screens/Chat.tsx'
import { MachinesScreen } from './screens/Machines.tsx'
import { PlaygroundScreen } from './screens/Playground.tsx'
import { actions, useCurrentIdea, useStore } from './store.ts'

export function App() {
  const screen = useStore((s) => s.screen)
  const idea = useCurrentIdea()
  const toast = useStore((s) => s.toast)
  const connected = useStore((s) => s.connected)

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'n') {
        event.preventDefault()
        void actions.createIdea()
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [])

  return (
    <div className="app">
      <Rail />
      {!idea ? (
        <main className="main">
          <div className="empty">{connected ? 'Create an idea to start.' : 'Connecting to the toolbox server…'}</div>
        </main>
      ) : screen === 'chat' ? (
        <ChatScreen idea={idea} />
      ) : screen === 'playground' ? (
        <PlaygroundScreen idea={idea} />
      ) : screen === 'machines' ? (
        <MachinesScreen idea={idea} />
      ) : (
        <AdaptersScreen idea={idea} />
      )}
      {toast ? <div className="toast">{toast}</div> : null}
    </div>
  )
}
