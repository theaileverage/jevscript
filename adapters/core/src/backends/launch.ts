/**
 * Launch scripts, for backends that start a pane with a shell in it.
 *
 * Herdr, cmux, Orca and Zellij open a pane running the user's shell and have
 * no way to say "run this argv and tell me how it ended". So the adapter writes
 * a small owner-only script that sets the environment, runs the agent and
 * records its exit status in a file beside it, and types one short line to run
 * that script. The exit file is how `observe` reports `exited` and `exit_code`
 * (spec section 9.1) on those backends; tmux has `pane_dead_status` instead.
 *
 * A script rather than a typed command line keeps a long prompt off the
 * terminal's input line, which has a length limit and echoes everything.
 */
import { randomBytes } from 'node:crypto'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import type { Files } from '../process.ts'
import { shellQuote } from '../process.ts'
import type { LaunchSpec } from './types.ts'

/** Where launch scripts go unless a backend is told otherwise. */
export function defaultStateDir(): string {
  const uid = typeof process.getuid === 'function' ? process.getuid() : 'user'
  return join(tmpdir(), `jevscript-agents-${uid}`)
}

/** What {@link writeLaunchScript} made. */
export interface LaunchScript {
  /** The line to type into the pane's shell. */
  command: string
  /** Where the agent's exit status will appear. */
  exitFile: string
}

/** The POSIX script that runs `spec` and records its exit status. */
export function launchScript(spec: LaunchSpec, exitFile: string): string {
  const lines = [
    '#!/bin/sh',
    '# Written by a Jevscript agent adapter: runs one agent and records how it ended.',
    `cd ${shellQuote(spec.cwd)} || { printf '%s\\n' 126 > ${shellQuote(exitFile)}; exit 126; }`,
  ]
  if (spec.unset.length > 0) lines.push(`unset ${spec.unset.map(shellQuote).join(' ')}`)
  for (const [key, value] of Object.entries(spec.env)) {
    if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(key)) continue
    lines.push(`export ${key}=${shellQuote(value)}`)
  }
  lines.push(spec.argv.map(shellQuote).join(' '))
  lines.push(`printf '%s\\n' "$?" > ${shellQuote(exitFile)}`)
  return `${lines.join('\n')}\n`
}

/** Write a fresh private launch script for `spec`. */
export async function writeLaunchScript(files: Files, stateDir: string, spec: LaunchSpec): Promise<LaunchScript> {
  const dir = join(stateDir, randomBytes(8).toString('hex'))
  await files.mkdirPrivate(dir)
  const script = join(dir, 'launch.sh')
  const exitFile = join(dir, 'exit')
  await files.write(script, launchScript(spec, exitFile), 0o700)
  return { command: `sh ${shellQuote(script)}`, exitFile }
}

/** The exit status recorded in `exitFile`, or `undefined` while the agent runs. */
export async function readExitFile(files: Files, exitFile: unknown): Promise<number | null | undefined> {
  if (typeof exitFile !== 'string' || exitFile === '') return undefined
  const text = await files.read(exitFile)
  if (text === undefined) return undefined
  const code = Number.parseInt(text.trim(), 10)
  return Number.isNaN(code) ? null : code
}
