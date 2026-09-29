import { execFileSync } from 'node:child_process'
import { join } from 'node:path'

import { describe, expect, it } from 'vitest'

import { machineGraph, overlay } from '../shared/graph.ts'
import type { Ir } from '../shared/ir.ts'
import type { MachineStepView } from '../shared/recording.ts'
import { paths, REPO_ROOT } from '../server/env.ts'

const ir = JSON.parse(
  execFileSync(paths().bin, ['compile', join(REPO_ROOT, 'examples/review_loop.jev')], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore'] }),
) as Ir
const graph = machineGraph(ir.machines[0]!)

describe('IR to graph (spec 7.8)', () => {
  it('has one edge per `on` line with its target', () => {
    expect(graph.edges.map((edge) => `${edge.from} -${edge.event}-> ${edge.to}`)).toEqual([
      'working -finished-> reviewing',
      'working -claims_done-> nudging',
      'working -stuck-> nudging',
      'working -asks-> waiting_on_me',
      'nudging -resumed-> working',
      'waiting_on_me -answered-> working',
      'reviewing -approved-> approved',
      'reviewing -rejected-> working',
    ])
  })

  it('styles guarded edges, Jev picks and risky edges apart', () => {
    const styles = Object.fromEntries(graph.edges.map((edge) => [edge.id, edge.style]))
    expect(styles).toMatchObject({
      'working.finished': 'guarded',
      'working.claims_done': 'guarded',
      'working.stuck': 'picked',
      'reviewing.approved': 'guarded',
      'reviewing.rejected': 'risky',
    })
    expect(graph.edges.find((edge) => edge.id === 'working.claims_done')?.guard).toBe('not tree.tests_pass')
  })

  it('marks the initial and terminal states and lays the main line out on one row', () => {
    const node = (id: string) => graph.nodes.find((candidate) => candidate.id === id)!
    expect(node('working')).toMatchObject({ initial: true, terminal: false })
    expect(node('approved')).toMatchObject({ terminal: true })
    const mainRow = node('working').row
    expect(['working', 'reviewing', 'approved'].map((id) => [node(id).column, node(id).row])).toEqual([
      [0, mainRow],
      [1, mainRow],
      [2, mainRow],
    ])
    expect([node('nudging').column, node('waiting_on_me').column]).toEqual([0, 0])
    expect(new Set(graph.nodes.map((candidate) => `${candidate.column}:${candidate.row}`)).size).toBe(graph.nodes.length)
    expect(graph.nodes.some((candidate) => candidate.orphan)).toBe(false)
  })

  it('overlays visits, fired edges and the current state from recorded steps', () => {
    const step = (from: string, chosen: string, to: string) => ({ from, chosen, to }) as MachineStepView
    const steps = [step('working', 'stuck', 'nudging'), step('nudging', 'resumed', 'working'), step('working', 'stay', 'working')]
    expect(overlay(graph, steps, 2)).toEqual({
      current: 'working',
      visits: { working: 2, nudging: 1 },
      fired: { 'working.stuck': 1, 'nudging.resumed': 1 },
    })
    expect(overlay(graph, steps, 0).current).toBe('working')
  })
})
