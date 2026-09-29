/**
 * A machine graph drawn in SVG from `machineGraph`: guarded edges solid
 * evergreen, Jev's picks grey, risky edges amber dashed, terminal states with
 * a double border, the current state filled.
 */
import type { Pin } from '../../shared/annotator.ts'
import type { GraphEdge, GraphNode, GraphOverlay, MachineGraph } from '../../shared/graph.ts'

const W = 164
const H = 62
const COLUMN = 262
const ROW = 190
const PAD_X = 112
const PAD_Y = 70

export type GraphTarget = { kind: 'state'; state: string } | { kind: 'edge'; from: string; event: string }

interface Point {
  x: number
  y: number
}

function centre(node: GraphNode): Point {
  return { x: PAD_X + node.column * COLUMN + W / 2, y: PAD_Y + node.row * ROW + H / 2 }
}

/** Where the ray from a box's centre towards `toward` leaves the box. */
function boxExit(c: Point, toward: Point): Point {
  const dx = toward.x - c.x
  const dy = toward.y - c.y
  if (dx === 0 && dy === 0) return c
  const scale = Math.min(Math.abs((W / 2 + 4) / (dx || 1e-9)), Math.abs((H / 2 + 4) / (dy || 1e-9)))
  return { x: c.x + dx * scale, y: c.y + dy * scale }
}

interface EdgeShape {
  path: string
  label: Point
  mid: Point
  anchor: 'start' | 'middle' | 'end'
}

function edgeShape(edge: GraphEdge, graph: MachineGraph): EdgeShape {
  const from = graph.nodes.find((node) => node.id === edge.from) as GraphNode
  const to = graph.nodes.find((node) => node.id === edge.to) as GraphNode
  const a = centre(from)
  const b = centre(to)
  if (edge.from === edge.to) {
    const top = { x: a.x, y: a.y - H / 2 }
    const siblings = graph.edges.filter((candidate) => candidate.from === edge.from && candidate.to === edge.to)
    const lift = 46 + siblings.indexOf(edge) * 22
    const path = `M ${top.x - 30} ${top.y} C ${top.x - 40} ${top.y - lift}, ${top.x + 40} ${top.y - lift}, ${top.x + 30} ${top.y}`
    return { path, label: { x: top.x, y: top.y - lift + 8 }, mid: { x: top.x, y: top.y - lift * 0.75 }, anchor: 'middle' }
  }
  const [first, second] = [edge.from, edge.to].sort()
  const pair = graph.edges.filter(
    (candidate) => [candidate.from, candidate.to].sort().join() === [first, second].join() && candidate.from !== candidate.to,
  )
  const index = pair.indexOf(edge)
  const canonA = centre(graph.nodes.find((node) => node.id === first) as GraphNode)
  const canonB = centre(graph.nodes.find((node) => node.id === second) as GraphNode)
  const length = Math.hypot(canonB.x - canonA.x, canonB.y - canonA.y) || 1
  const normal = { x: -(canonB.y - canonA.y) / length, y: (canonB.x - canonA.x) / length }
  // Same-column edges bow out so they do not run through the states between them.
  const skip = from.column === to.column && Math.abs(from.row - to.row) > 1 ? 70 : 0
  const bend = (index - (pair.length - 1) / 2) * 34 + (pair.length === 1 ? skip : 0)
  const control = { x: (a.x + b.x) / 2 + normal.x * bend * 2, y: (a.y + b.y) / 2 + normal.y * bend * 2 }
  const start = boxExit(a, control)
  const end = boxExit(b, control)
  const mid = { x: 0.25 * start.x + 0.5 * control.x + 0.25 * end.x, y: 0.25 * start.y + 0.5 * control.y + 0.25 * end.y }
  const path = `M ${start.x} ${start.y} Q ${control.x} ${control.y} ${end.x} ${end.y}`
  // Labels sit on the outside of the curve, so parallel edges keep theirs apart.
  const chord = { x: (start.x + end.x) / 2, y: (start.y + end.y) / 2 }
  if (Math.abs(b.x - a.x) > Math.abs(b.y - a.y)) {
    const below = mid.y > chord.y + 1
    return { path, mid, anchor: 'middle', label: { x: mid.x, y: below ? mid.y + 16 : mid.y - 8 } }
  }
  const right = mid.x > chord.x + 1 || (Math.abs(mid.x - chord.x) <= 1 && index > 0)
  return { path, mid, anchor: right ? 'start' : 'end', label: { x: mid.x + (right ? 8 : -8), y: mid.y + 4 } }
}

export function Graph({
  graph,
  overlay,
  pins,
  onPick,
}: {
  graph: MachineGraph
  overlay: GraphOverlay
  pins: Pin[]
  onPick: (target: GraphTarget, at: Point) => void
}) {
  const columns = Math.max(...graph.nodes.map((node) => node.column)) + 1
  const rows = Math.max(...graph.nodes.map((node) => node.row)) + 1
  const width = PAD_X * 2 + (columns - 1) * COLUMN + W
  const height = PAD_Y * 2 + (rows - 1) * ROW + H
  const hasRun = Object.keys(overlay.visits).length > 0
  const pinFor = (match: (pin: Pin) => boolean) => pins.find((pin) => pin.number !== null && pin.status !== 'discarded' && match(pin))

  return (
    <svg className="graph" viewBox={`0 0 ${width} ${height}`} style={{ width: '100%', maxWidth: width, height: 'auto' }}>
      <defs>
        {(['guarded', 'picked', 'risky'] as const).map((style) => (
          <marker key={style} id={`arrow-${style}`} viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
            <path d="M 0 0 L 10 5 L 0 10 z" fill={style === 'guarded' ? '#1D5A3F' : style === 'risky' ? '#C2740E' : '#B4BBB7'} />
          </marker>
        ))}
      </defs>
      {graph.edges.map((edge) => {
        const shape = edgeShape(edge, graph)
        const fired = overlay.fired[edge.id] ?? 0
        const pin = pinFor((candidate) => candidate.target.kind === 'edge' && candidate.target.from === edge.from && candidate.target.event === edge.event)
        const pick = (event: React.MouseEvent) => onPick({ kind: 'edge', from: edge.from, event: edge.event }, { x: event.clientX, y: event.clientY })
        return (
          <g key={edge.id} className={`edge ${edge.style} ${fired > 0 ? 'fired' : ''}`} onClick={pick}>
            <title>{`${edge.event}: ${edge.description}${edge.guard ? `\nwhen ${edge.guard}` : ''}${edge.risky ? '\nrisky: the gate asks before it fires' : ''}`}</title>
            <path d={shape.path} markerEnd={`url(#arrow-${edge.style})`} />
            <path d={shape.path} className="hit" />
            <text x={shape.label.x} y={shape.label.y} textAnchor={shape.anchor} fontWeight={edge.style === 'picked' ? 400 : 500}>
              {edge.event}
              {edge.risky ? ' · risky' : ''}
            </text>
            {edge.guard ? (
              <text className="guard" x={shape.label.x} y={shape.label.y + 15} textAnchor={shape.anchor}>
                when {edge.guard}
              </text>
            ) : null}
            {pin ? <PinMark at={shape.mid} n={pin.number as number} /> : null}
          </g>
        )
      })}
      {graph.nodes.map((node) => {
        const x = PAD_X + node.column * COLUMN
        const y = PAD_Y + node.row * ROW
        const visits = overlay.visits[node.id] ?? 0
        const current = overlay.current === node.id
        const sub = [
          node.initial ? 'initial' : null,
          node.terminal ? 'terminal' : null,
          current ? 'now' : hasRun ? (visits > 0 ? `visited ${visits}×` : 'not visited') : null,
          node.orphan ? 'unreachable' : null,
        ]
          .filter(Boolean)
          .join(' · ')
        const pin = pinFor((candidate) => candidate.target.kind === 'state' && candidate.target.state === node.id)
        return (
          <g
            key={node.id}
            className={`node ${current ? 'current' : ''} ${hasRun && visits === 0 && !current ? 'unvisited' : ''}`}
            onClick={(event) => onPick({ kind: 'state', state: node.id }, { x: event.clientX, y: event.clientY })}
          >
            {current ? <rect className="halo" x={x - 4} y={y - 4} width={W + 8} height={H + 8} rx={12} /> : null}
            <rect className="body" x={x} y={y} width={W} height={H} rx={9} />
            {node.terminal ? <rect className="ring" x={x + 3} y={y + 3} width={W - 6} height={H - 6} rx={7} /> : null}
            <text x={x + 17} y={y + 27}>
              {node.id}
            </text>
            <text className="sub" x={x + 17} y={y + 46}>
              {sub}
            </text>
            {pin ? <PinMark at={{ x: x + W - 4, y: y + 4 }} n={pin.number as number} /> : null}
          </g>
        )
      })}
    </svg>
  )
}

function PinMark({ at, n }: { at: Point; n: number }) {
  return (
    <g className="pin">
      <circle cx={at.x} cy={at.y} r={9} />
      <text x={at.x} y={at.y + 3.5} textAnchor="middle">
        {n}
      </text>
    </g>
  )
}
