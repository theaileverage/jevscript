/**
 * The machine annotator: pins on a machine's states and edges, the questions
 * asked at them, and the source edits Claude proposes.
 *
 * An edit is a list of exact find/replace pairs against the `.jev` text. Each
 * `find` must occur exactly once, so an edit either lands where the model
 * meant it or is refused; nothing is fuzzily patched. The toolbox checks the
 * patched source before it offers Apply, and Apply is reversible while the
 * replaced text is still in the source.
 */
import type { Diagnostic } from './protocol.ts'
import type { MachineGraph } from './graph.ts'

export interface SourceEdit {
  find: string
  replace: string
}

export type PinTarget =
  | { kind: 'state'; machine: string; state: string }
  | { kind: 'edge'; machine: string; from: string; event: string }
  | { kind: 'machine'; machine: string }

/** What a check of the patched source found, relative to the source it patched. */
export interface EditCheck {
  errors: Diagnostic[]
  /** Warnings the edit introduced; ones the program already had are not repeated. */
  newWarnings: Diagnostic[]
  /** Plain statements about states whose incoming events or reachability changed. */
  reachability: string[]
}

export type PinReply =
  | { kind: 'answer'; text: string }
  | {
      kind: 'edit'
      text: string
      edits: SourceEdit[]
      /** Line diff for display. */
      diff: DiffLine[]
      check: EditCheck | null
      /** Why the edit could not be applied to the current source, if it could not. */
      refused: string | null
    }
  | { kind: 'error'; text: string }

export type PinStatus = 'open' | 'applied' | 'discarded'

export interface Pin {
  id: string
  /** Shown on the graph; whole-machine questions have none. */
  number: number | null
  target: PinTarget
  query: string
  reply: PinReply | null
  status: PinStatus
  createdAt: string
}

export class EditError extends Error {}

/** Apply every edit in order. Throws {@link EditError} unless each `find` occurs exactly once. */
export function applyEdits(source: string, edits: readonly SourceEdit[]): string {
  let text = source
  for (const [index, edit] of edits.entries()) {
    if (edit.find === '') throw new EditError(`edit ${index + 1} has nothing to find`)
    const first = text.indexOf(edit.find)
    if (first === -1) throw new EditError(`edit ${index + 1}: the text to replace is not in the source`)
    if (text.indexOf(edit.find, first + 1) !== -1) {
      throw new EditError(`edit ${index + 1}: the text to replace occurs more than once`)
    }
    text = text.slice(0, first) + edit.replace + text.slice(first + edit.find.length)
  }
  return text
}

/** Undo {@link applyEdits}: the inverse edits, applied last first, under the same uniqueness rule. */
export function revertEdits(source: string, edits: readonly SourceEdit[]): string {
  return applyEdits(
    source,
    [...edits].reverse().map((edit) => ({ find: edit.replace, replace: edit.find })),
  )
}

export interface DiffLine {
  op: ' ' | '-' | '+'
  text: string
}

/**
 * A line diff trimmed to the changed region with one line of context, from a
 * longest-common-subsequence table. Programs are small, so quadratic is fine.
 */
export function lineDiff(before: string, after: string): DiffLine[] {
  const a = before.split('\n')
  const b = after.split('\n')
  const table: number[][] = Array.from({ length: a.length + 1 }, () => new Array<number>(b.length + 1).fill(0))
  for (let i = a.length - 1; i >= 0; i--) {
    for (let j = b.length - 1; j >= 0; j--) {
      const row = table[i] as number[]
      row[j] = a[i] === b[j] ? (table[i + 1]?.[j + 1] ?? 0) + 1 : Math.max(table[i + 1]?.[j] ?? 0, row[j + 1] ?? 0)
    }
  }
  const lines: DiffLine[] = []
  let i = 0
  let j = 0
  while (i < a.length || j < b.length) {
    if (i < a.length && j < b.length && a[i] === b[j]) {
      lines.push({ op: ' ', text: a[i] as string })
      i++
      j++
    } else if (i < a.length && (j >= b.length || (table[i + 1]?.[j] ?? 0) >= (table[i]?.[j + 1] ?? 0))) {
      lines.push({ op: '-', text: a[i] as string })
      i++
    } else {
      lines.push({ op: '+', text: b[j] as string })
      j++
    }
  }
  const changed = lines.flatMap((line, index) => (line.op === ' ' ? [] : [index]))
  if (changed.length === 0) return []
  const from = Math.max(0, (changed[0] as number) - 1)
  const to = Math.min(lines.length, (changed.at(-1) as number) + 2)
  return lines.slice(from, to)
}

/**
 * Warnings in `after` that `before` did not have. Line numbers move when an
 * edit adds or removes lines, so a warning is identified by its code and
 * message, counted, not by where it sits.
 */
export function newWarnings(before: readonly Diagnostic[], after: readonly Diagnostic[]): Diagnostic[] {
  const seen = new Map<string, number>()
  for (const diagnostic of before) {
    if (diagnostic.severity !== 'warning') continue
    const key = `${diagnostic.code}\u0000${diagnostic.message}`
    seen.set(key, (seen.get(key) ?? 0) + 1)
  }
  return after.filter((diagnostic) => {
    if (diagnostic.severity !== 'warning') return false
    const key = `${diagnostic.code}\u0000${diagnostic.message}`
    const left = seen.get(key) ?? 0
    if (left > 0) {
      seen.set(key, left - 1)
      return false
    }
    return true
  })
}

/** How reaching each state changed between two versions of a machine. */
export function reachabilityChanges(before: MachineGraph | null, after: MachineGraph | null): string[] {
  if (!before || !after) return []
  const incoming = (graph: MachineGraph, state: string) =>
    graph.edges
      .filter((edge) => edge.to === state && edge.from !== state)
      .map((edge) => edge.event)
      .sort()
  const changes: string[] = []
  const names = new Set([...before.nodes.map((node) => node.id), ...after.nodes.map((node) => node.id)])
  for (const name of names) {
    const was = before.nodes.find((node) => node.id === name)
    const now = after.nodes.find((node) => node.id === name)
    if (!now) {
      changes.push(`${name} was removed.`)
      continue
    }
    if (!was) {
      changes.push(`${name} is new.`)
      continue
    }
    if (was.orphan !== now.orphan) {
      changes.push(now.orphan ? `${name} is no longer reachable.` : `${name} is now reachable.`)
    }
    const a = incoming(before, name)
    const b = incoming(after, name)
    if (a.join() !== b.join() && !now.orphan) {
      changes.push(
        b.length === 0 ? `${name} has no incoming events.` : `${name} is now reached by ${joinWords(b)}.`,
      )
    }
  }
  return changes
}

function joinWords(words: readonly string[]): string {
  if (words.length <= 1) return words.join('')
  return `${words.slice(0, -1).join(', ')} and ${words.at(-1) as string}`
}

/** The next pin number for a machine: one more than the highest drawn so far. */
export function nextPinNumber(pins: readonly Pin[], machine: string): number {
  return (
    Math.max(
      0,
      ...pins
        .filter((pin) => pin.target.machine === machine && pin.number !== null)
        .map((pin) => pin.number as number),
    ) + 1
  )
}

export function describeTarget(target: PinTarget): string {
  switch (target.kind) {
    case 'state':
      return `state ${target.state}`
    case 'edge':
      return `${target.from} · ${target.event}`
    case 'machine':
      return `machine ${target.machine}`
  }
}
