/** Building and back-filling ideas, shared by the server's store and the page. */
import type { Idea } from './protocol.ts'
import { DEFAULT_HOST } from './host.ts'

export function newIdea(partial: Partial<Idea> = {}): Idea {
  const now = new Date().toISOString()
  return normalize({ id: crypto.randomUUID(), createdAt: now, updatedAt: now, ...partial })
}

/** Fill fields an older file may lack, retaining its source and pins as the first Jev file. */
export function normalize(idea: Partial<Idea>): Idea {
  const now = new Date().toISOString()
  const id = idea.id ?? crypto.randomUUID()
  const source = idea.source ?? ''
  const fileName = idea.fileName ?? 'program.jev'
  const initialId = `${id}:main`
  const workspace = idea.workspace ?? {
    files: [{ id: initialId, name: fileName, kind: 'jev' as const, source }],
    activeFileId: initialId,
    entryFileId: initialId,
    annotations: [],
  }
  const files = workspace.files.map(file => file.id === workspace.entryFileId ? { ...file, source, name: fileName } : file)
  if (!files.some(file => file.kind === 'host')) {
    files.push({ id: `${id}:host`, name: 'host.ts', kind: 'host', source: DEFAULT_HOST })
  }
  return {
    id,
    title: idea.title ?? 'Untitled idea',
    fileName,
    source,
    createdAt: idea.createdAt ?? now,
    updatedAt: idea.updatedAt ?? now,
    messages: idea.messages ?? [],
    claudeHistory: idea.claudeHistory ?? [],
    chatAgent: idea.chatAgent ?? null,
    workspace: { ...workspace, files },
    inputs: idea.inputs ?? '{}',
    bindings: idea.bindings ?? {},
    model: idea.model ?? 'jev-latest',
    sample: idea.sample ?? false,
    runs: idea.runs ?? [],
    pins: (idea.pins ?? []).map(pin => ({ ...pin, fileId: pin.fileId ?? workspace.entryFileId })),
  }
}
