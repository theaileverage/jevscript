/**
 * LSP semantic tokens (LSP 3.17, `textDocument/semanticTokens/full`) decoded
 * into absolute ranges and the CSS classes the editor paints them with. The
 * colours follow the Paper design; the legend comes from the server.
 */

export interface SemanticLegend {
  tokenTypes: string[]
  tokenModifiers: string[]
}

export interface SemanticToken {
  line: number
  character: number
  length: number
  type: string
  modifiers: string[]
}

/** Each token is five integers: line delta, start delta (from the previous token on the same line), length, type, modifier bits. */
export function decodeTokens(data: readonly number[], legend: SemanticLegend): SemanticToken[] {
  const tokens: SemanticToken[] = []
  let line = 0
  let character = 0
  for (let index = 0; index + 4 < data.length; index += 5) {
    const deltaLine = data[index] as number
    const deltaStart = data[index + 1] as number
    line += deltaLine
    character = deltaLine === 0 ? character + deltaStart : deltaStart
    const bits = data[index + 4] as number
    tokens.push({
      line,
      character,
      length: data[index + 2] as number,
      type: legend.tokenTypes[data[index + 3] as number] ?? 'unknown',
      modifiers: legend.tokenModifiers.filter((_, bit) => (bits & (1 << bit)) !== 0),
    })
  }
  return tokens
}

/** `tok-<type>` plus one `tok-m-<modifier>` per modifier. */
export function tokenClass(token: Pick<SemanticToken, 'type' | 'modifiers'>): string {
  return [`tok-${token.type}`, ...token.modifiers.map((modifier) => `tok-m-${modifier}`)].join(' ')
}
