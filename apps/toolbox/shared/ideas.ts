/** Building and back-filling ideas, shared by the server's store and the page. */
import type { Idea } from './protocol.ts'

export function newIdea(partial: Partial<Idea> = {}): Idea {
  const now = new Date().toISOString()
  return normalize({ id: crypto.randomUUID(), createdAt: now, updatedAt: now, ...partial })
}

/** Fill fields an older file may lack. */
export function normalize(idea: Partial<Idea>): Idea {
  const now = new Date().toISOString()
  return {
    id: idea.id ?? crypto.randomUUID(),
    title: idea.title ?? 'Untitled idea',
    fileName: idea.fileName ?? 'program.jev',
    source: idea.source ?? '',
    createdAt: idea.createdAt ?? now,
    updatedAt: idea.updatedAt ?? now,
    messages: idea.messages ?? [],
    claudeHistory: idea.claudeHistory ?? [],
    inputs: idea.inputs ?? '{}',
    bindings: idea.bindings ?? {},
    model: idea.model ?? 'jev-latest',
    sample: idea.sample ?? false,
    runs: idea.runs ?? [],
    pins: idea.pins ?? [],
  }
}
