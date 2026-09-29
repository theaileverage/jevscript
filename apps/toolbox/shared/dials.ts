/**
 * Tuning dials over the program text.
 *
 * The source is the single source of truth: a dial is a numeric literal in a
 * `task` or `machine` header's `budget` or `thresholds` clause (spec sections
 * 7.1, 7.6 and 7.8), found by its character range, and moving a dial rewrites
 * exactly that range. Only what the program declares gets a dial; adding a
 * clause is an edit, not a tuning.
 */

export type DialClause = 'budget' | 'thresholds'

export interface Dial {
  /** Stable across rewrites: unit, clause and key. */
  id: string
  unit: 'task' | 'machine'
  unitName: string
  clause: DialClause
  key: string
  value: number
  /** 1-based line of the header. */
  line: number
  /** The literal's range in the source, end exclusive. */
  start: number
  end: number
}

const HEADER = /^(task|machine)\s+([A-Za-z_][A-Za-z0-9_]*)/
const CLAUSE = /\b(budget|thresholds)\b/g
const PAIR = /([A-Za-z_][A-Za-z0-9_]*)\s+(\d+(?:\.\d+)?k?)/g

export function findDials(source: string): Dial[] {
  const dials: Dial[] = []
  let offset = 0
  const lines = source.split('\n')
  for (const [index, line] of lines.entries()) {
    const header = HEADER.exec(line)
    if (header) {
      const code = stripComment(line)
      const colon = code.lastIndexOf(':')
      const body = colon === -1 ? code : code.slice(0, colon)
      const clauses = [...body.matchAll(CLAUSE)]
      for (const [position, clause] of clauses.entries()) {
        const from = (clause.index ?? 0) + clause[0].length
        const to = clauses[position + 1]?.index ?? body.length
        for (const pair of body.slice(from, to).matchAll(PAIR)) {
          const [, key, raw] = pair as unknown as [string, string, string]
          const literalAt = from + (pair.index ?? 0) + pair[0].length - raw.length
          dials.push({
            id: `${header[1]}:${header[2]}:${clause[1]}:${key}`,
            unit: header[1] as Dial['unit'],
            unitName: header[2] as string,
            clause: clause[1] as DialClause,
            key,
            value: parseNumber(raw),
            line: index + 1,
            start: offset + literalAt,
            end: offset + literalAt + raw.length,
          })
        }
      }
    }
    offset += line.length + 1
  }
  return dials
}

/** Write a new value for one dial into the source, touching nothing else. */
export function setDial(source: string, dial: Dial, value: number): string {
  return source.slice(0, dial.start) + formatDial(dial, value) + source.slice(dial.end)
}

/** How a dial value is written back: thresholds and `usd` to two places, counts whole. */
export function formatDial(dial: Pick<Dial, 'clause' | 'key'>, value: number): string {
  if (dial.clause === 'thresholds' || dial.key === 'usd') {
    const text = (Math.round(value * 100) / 100).toFixed(2).replace(/0+$/, '').replace(/\.$/, '')
    return text === '' ? '0' : text
  }
  return String(Math.max(0, Math.round(value)))
}

/** A slider's range for a dial. Thresholds are probabilities; budgets grow with the value. */
export function dialRange(dial: Pick<Dial, 'clause' | 'key' | 'value'>): { min: number; max: number; step: number } {
  if (dial.clause === 'thresholds') return { min: 0, max: 1, step: 0.01 }
  if (dial.key === 'usd') return { min: 0, max: Math.max(1, dial.value * 4), step: 0.01 }
  return { min: 1, max: Math.max(10, Math.ceil(dial.value * 4)), step: 1 }
}

function parseNumber(raw: string): number {
  return raw.endsWith('k') ? Number(raw.slice(0, -1)) * 1000 : Number(raw)
}

function stripComment(line: string): string {
  let inString = false
  for (let index = 0; index < line.length; index++) {
    const char = line[index]
    if (char === '"' && line[index - 1] !== '\\') inString = !inString
    if (char === '#' && !inString) return line.slice(0, index)
  }
  return line
}
