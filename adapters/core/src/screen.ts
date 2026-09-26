/**
 * What adapters read off a captured screen, and what they may type into one.
 *
 * The screen is the `tail` of an observation (spec section 9.1): agent-written
 * text, bounded and otherwise untouched. An adapter decides only two things
 * from it, both ingredients of `status` and never messages: whether a harness's
 * busy marker is showing, and whether its input box is empty.
 */

/**
 * Roughly 4k tokens (spec section 9.1 says "the last screen or ~4k tokens"),
 * at the usual four characters a token.
 */
export const TAIL_LIMIT = 16_000

/** Trim a capture to its meaningful lines and bound it from the end. */
export function boundTail(screen: string, limit = TAIL_LIMIT): string {
  const lines = screen.split('\n').map((line) => line.replace(/[\s ]+$/, ''))
  while (lines.length > 0 && lines.at(-1) === '') lines.pop()
  const joined = lines.join('\n')
  return joined.length > limit ? joined.slice(joined.length - limit) : joined
}

// CSI, OSC (BEL- or ST-terminated) and the two-byte escapes.
// eslint-disable-next-line no-control-regex
const ANSI = /\x1b\[[0-?]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\x1b[@-Z\\-_]/g

/** Remove terminal escape sequences, for backends that return styled text. */
export function stripAnsi(text: string): string {
  return text.replace(ANSI, '')
}

/** The last `count` lines that are not blank, oldest first. */
export function lastLines(screen: string, count: number): string[] {
  const lines = screen.split('\n').filter((line) => line.trim() !== '')
  return lines.slice(Math.max(0, lines.length - count))
}

/** True when any pattern matches any of the screen's last `count` non-blank lines. */
export function matchesNear(screen: string, patterns: readonly RegExp[], count: number): boolean {
  if (patterns.length === 0) return false
  const lines = lastLines(screen, count)
  return lines.some((line) => patterns.some((pattern) => pattern.test(line)))
}

/**
 * Make program-supplied text safe to type (spec section 9.1 `send`).
 *
 * The text reaches a terminal, where a control character is a key: an Escape
 * could cancel the agent's turn and a `^C` kill it. So every C0 control except
 * newline and tab is dropped, carriage returns become newlines, and DEL goes
 * too. What is left is exactly the characters a person could have typed.
 */
export function safeText(text: string): string {
  // eslint-disable-next-line no-control-regex
  return text.replace(/\r\n?/g, '\n').replace(/[\x00-\x08\x0b-\x1f\x7f]/g, '')
}
