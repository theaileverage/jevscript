import type { ChatOrigin } from '../shared/protocol.ts'

export function agentLabel(origin?: ChatOrigin): string {
  if (!origin) return 'Earlier reply · source not recorded'
  switch (origin.kind) {
    case 'agent': return `${origin.harness === 'codex' ? 'Codex' : 'Claude Code'} · ${origin.model} · local CLI response`
    case 'error': return `${origin.harness === 'codex' ? 'Codex' : 'Claude Code'} · ${origin.model} · error`
    case 'api': return `Anthropic API · ${origin.model}`
    case 'fixture': return `Fixture response · ${origin.model}`
    case 'check': return 'jevscript check'
  }
}
