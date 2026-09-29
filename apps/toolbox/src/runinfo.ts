/** Small readings of a run and its program that several screens share. */
import type { Ir } from '../shared/ir.ts'
import type { RequestOrigin } from '../shared/requests.ts'

/**
 * The threshold a confidence is judged against: the asking machine's
 * `min_confidence`, else `main`'s. Null when the program declares none, in
 * which case nothing is marked low.
 */
export function confidenceThreshold(ir: Ir | null | undefined, origin: RequestOrigin | null): { value: number; owner: string } | null {
  if (!ir) return null
  if (origin?.kind === 'machine') {
    if (origin.machine === null) return null
    const machine = ir.machines.find((candidate) => candidate.name === origin.machine)
    if (machine?.thresholds.min_confidence !== undefined) return { value: machine.thresholds.min_confidence, owner: `machine ${machine.name}` }
  }
  const main = ir.tasks.find((task) => task.name === 'main')
  return main?.thresholds.min_confidence !== undefined ? { value: main.thresholds.min_confidence, owner: 'task main' } : null
}

/** The main task's `calls` budget, paired with the run's total usage (spec section 7.1). */
export function callsLimit(ir: Ir | null | undefined): number | null {
  if (!ir) return null
  return ir.tasks.find((task) => task.name === 'main')?.budget.calls ?? null
}

export function usd(value: number): string {
  if (value === 0) return '$0'
  if (value < 0.0001) return '<$0.0001'
  return value < 0.01 ? `$${value.toFixed(4)}` : `$${value.toFixed(3)}`
}

export function short(value: unknown, max = 80): string {
  const text = typeof value === 'string' ? value : JSON.stringify(value)
  return text.length > max ? `${text.slice(0, max - 1)}…` : text
}
