/**
 * A machine as a drawable graph (spec section 7.8), built only from the
 * compiled IR: states, their events, targets, `when` guards, `risky` and
 * terminal states. The layout puts the main line from the initial state to a
 * terminal one on a single row and hangs the other states above and below
 * the state they are first reached from.
 */
import type { IrMachine } from './ir.ts'
import { printExpr, textLiteral } from './ir.ts'
import type { MachineStepView } from './recording.ts'

/**
 * How an edge is drawn. A guarded edge is proved by code; an unguarded one is
 * Jev's pick; a risky one makes the gate ask before it fires. Risky wins over
 * guarded because the pause is what the user needs to see.
 */
export type EdgeStyle = 'guarded' | 'picked' | 'risky'

export interface GraphNode {
  id: string
  initial: boolean
  terminal: boolean
  column: number
  row: number
  /** Unreachable from the initial state. */
  orphan: boolean
}

export interface GraphEdge {
  id: string
  from: string
  to: string
  event: string
  description: string
  guard: string | null
  risky: boolean
  style: EdgeStyle
  hasActions: boolean
}

export interface MachineGraph {
  name: string
  params: string[]
  initial: string
  nodes: GraphNode[]
  edges: GraphEdge[]
}

export function machineGraph(machine: IrMachine): MachineGraph {
  const initial = machine.initial || machine.states[0]?.name || ''
  const edges: GraphEdge[] = machine.states.flatMap((state) =>
    (state.transitions ?? []).map((transition) => {
      const guard = transition.when ? printExpr(transition.when) : null
      return {
        id: `${state.name}.${transition.event}`,
        from: state.name,
        to: transition.target,
        event: transition.event,
        description: textLiteral(transition.description),
        guard,
        risky: transition.risky,
        style: transition.risky ? 'risky' : guard ? 'guarded' : 'picked',
        hasActions: (transition.body?.length ?? 0) > 0,
      }
    }),
  )

  const names = machine.states.map((state) => state.name)
  const terminals = new Set(machine.states.filter((state) => state.done).map((state) => state.name))
  const spine = mainPath(initial, names, edges, terminals)
  const place = new Map<string, { column: number; row: number }>()
  spine.forEach((name, column) => place.set(name, { column, row: 0 }))
  const taken = (column: number, row: number) => [...place.values()].some((at) => at.column === column && at.row === row)

  // Every other reachable state hangs off the placed state it is first reached from, above or below it.
  const reachable = new Set<string>([initial])
  const queue = [initial]
  while (queue.length > 0) {
    const current = queue.shift() as string
    for (const edge of edges) {
      if (edge.from !== current || reachable.has(edge.to)) continue
      reachable.add(edge.to)
      queue.push(edge.to)
    }
  }
  for (const name of names.filter((candidate) => reachable.has(candidate) && !place.has(candidate))) {
    const anchor =
      edges.find((edge) => edge.to === name && place.has(edge.from))?.from ??
      edges.find((edge) => edge.from === name && place.has(edge.to))?.to ??
      initial
    const column = place.get(anchor)?.column ?? 0
    let row = 1
    for (let step = 1; taken(column, row); step++) row = step % 2 === 1 ? -Math.ceil(step / 2) : Math.ceil(step / 2) + 1
    place.set(name, { column, row })
  }
  const lowest = Math.max(0, ...[...place.values()].map((at) => at.row))
  names
    .filter((name) => !place.has(name))
    .forEach((name, index) => place.set(name, { column: index, row: lowest + 1 }))
  const top = Math.min(...[...place.values()].map((at) => at.row))

  const nodes: GraphNode[] = machine.states.map((state) => {
    const at = place.get(state.name) as { column: number; row: number }
    return {
      id: state.name,
      initial: state.name === initial,
      terminal: state.done,
      column: at.column,
      row: at.row - top,
      orphan: !reachable.has(state.name),
    }
  })

  return { name: machine.name, params: machine.params.map((param) => param.name), initial, nodes, edges }
}

/**
 * The longest simple path from the initial state, preferring one that ends in
 * a terminal state: the machine's main line, drawn left to right. Machines are
 * small, but the search is bounded anyway.
 */
function mainPath(initial: string, names: string[], edges: GraphEdge[], terminals: Set<string>): string[] {
  let best: string[] = [initial]
  let bestEndsDone = terminals.has(initial)
  let budget = 50_000
  const walk = (path: string[]) => {
    if (budget-- <= 0) return
    const last = path.at(-1) as string
    const endsDone = terminals.has(last)
    if ((endsDone && !bestEndsDone) || (endsDone === bestEndsDone && path.length > best.length)) {
      best = [...path]
      bestEndsDone = endsDone
    }
    for (const edge of edges) {
      if (edge.from === last && !path.includes(edge.to) && names.includes(edge.to)) walk([...path, edge.to])
    }
  }
  walk([initial])
  return best
}

/** What a run did to the graph up to a point in its timeline. */
export interface GraphOverlay {
  current: string | null
  visits: Record<string, number>
  /** Edge ids that fired, with how often. */
  fired: Record<string, number>
}

/**
 * Overlay the first `upTo` recorded steps. A state is visited once for being
 * started in and once per step that moved into it; `current` is where the last
 * included step left the machine.
 */
export function overlay(graph: MachineGraph, steps: readonly MachineStepView[], upTo: number): GraphOverlay {
  const included = steps.slice(0, upTo)
  const visits: Record<string, number> = {}
  const fired: Record<string, number> = {}
  if (steps.length > 0) visits[steps[0]?.from ?? graph.initial] = 1
  for (const step of included) {
    if (step.to !== step.from) visits[step.to] = (visits[step.to] ?? 0) + 1
    const edge = graph.edges.find((candidate) => candidate.from === step.from && candidate.event === step.chosen)
    if (edge && step.to === edge.to) fired[edge.id] = (fired[edge.id] ?? 0) + 1
  }
  const current = included.at(-1)?.to ?? (steps.length > 0 ? (steps[0]?.from ?? null) : null)
  return { current, visits, fired }
}
