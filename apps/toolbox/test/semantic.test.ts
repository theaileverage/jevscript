import { describe, expect, it } from 'vitest'

import { decodeTokens, tokenClass } from '../shared/semantic.ts'

/** The legend the jevscript language server announces. */
const legend = {
  tokenTypes: ['keyword', 'comment', 'string', 'number', 'operator', 'function', 'method', 'namespace', 'interface', 'parameter', 'variable', 'property', 'enumMember', 'event', 'type'],
  tokenModifiers: ['declaration', 'readonly', 'defaultLibrary', 'unitKind', 'judgmentVerb'],
}

describe('semantic tokens (LSP 3.17 relative encoding)', () => {
  it('decodes the first line of review_loop.jev as the server sends it', () => {
    // `program review_loop` then `needs claude: agent` on line 2.
    const data = [0, 0, 7, 0, 0, 0, 8, 11, 7, 1, 2, 0, 5, 0, 0, 0, 6, 6, 8, 3]
    expect(decodeTokens(data, legend)).toEqual([
      { line: 0, character: 0, length: 7, type: 'keyword', modifiers: [] },
      { line: 0, character: 8, length: 11, type: 'namespace', modifiers: ['declaration'] },
      { line: 2, character: 0, length: 5, type: 'keyword', modifiers: [] },
      { line: 2, character: 6, length: 6, type: 'interface', modifiers: ['declaration', 'readonly'] },
    ])
  })

  it('names classes by type and modifier', () => {
    expect(tokenClass({ type: 'event', modifiers: ['declaration'] })).toBe('tok-event tok-m-declaration')
  })
})
