/**
 * Reading the arguments of one capability call (spec section 9.1).
 *
 * The runtime sends `{ positional, named }`. A verb on a handle has the handle
 * as its first positional argument (AGENTS.md "Decisions the interpreter
 * made"); a host may also pass it as `named.handle`. Handles arrive in the
 * SDK boundary's JSON form, which tags them `"$jev": "handle"`; the tag is
 * ignored here.
 */
import type { CallArgs, Handle } from 'jevscript'

import { AdapterError } from './errors.ts'

export function handleOf(args: CallArgs): Handle {
  const handle = args.named?.['handle'] ?? args.positional?.[0]
  if (!handle || typeof handle !== 'object') throw new AdapterError('this verb needs a handle', false)
  return handle as Handle
}

export function textOf(args: CallArgs): string {
  const text = args.named?.['text'] ?? args.positional?.[1] ?? ''
  return String(text)
}

export function minutesOf(args: CallArgs): number {
  const minutes = Number(args.named?.['minutes'] ?? 5)
  if (!Number.isFinite(minutes) || minutes < 0) throw new AdapterError('`minutes` must be a number', false)
  return minutes
}

/** `wait idle`: the evaluator passes the mode word as text (AGENTS.md "`wait`'s mode word"). */
export function conditionOf(args: CallArgs): string {
  const condition = args.positional?.[1] ?? args.named?.['condition'] ?? 'idle'
  return String(condition)
}

export function stringOr(value: unknown, fallback: string): string {
  return typeof value === 'string' && value !== '' ? value : fallback
}

export function optionalString(value: unknown): string | undefined {
  return typeof value === 'string' && value !== '' ? value : undefined
}

/** A boolean argument: `true`, `"true"`, `"yes"` and `1` count. */
export function flag(value: unknown): boolean | undefined {
  if (value === undefined || value === null) return undefined
  if (typeof value === 'boolean') return value
  if (typeof value === 'number') return value !== 0
  if (typeof value === 'string') return ['true', 'yes', '1', 'on'].includes(value.toLowerCase())
  return undefined
}

/** A list argument: a list of anything, or one word. */
export function list(value: unknown): string[] {
  if (Array.isArray(value)) return value.map(String)
  if (typeof value === 'string' && value !== '') return [value]
  return []
}

/** The working directory an `in` handle names, whichever field its adapter used. */
export function cwdOf(handle: unknown): string | undefined {
  if (typeof handle === 'string' && handle !== '') return handle
  if (!handle || typeof handle !== 'object') return undefined
  const record = handle as Record<string, unknown>
  for (const key of ['cwd', 'path', 'dir']) {
    const value = record[key]
    if (typeof value === 'string' && value !== '') return value
  }
  return undefined
}
