// The manifest and language configuration VS Code, Cursor and Windsurf read,
// and the fields Open VSX requires to accept the package.

import { readFileSync } from 'node:fs'
import { describe, expect, it } from 'vitest'

const root = new URL('..', import.meta.url)
const manifest = JSON.parse(readFileSync(new URL('package.json', root), 'utf8'))
const language = JSON.parse(readFileSync(new URL('language-configuration.json', root), 'utf8'))

describe('package.json', () => {
  it('has every field Open VSX and vsce require', () => {
    for (const field of ['name', 'publisher', 'version', 'license', 'repository', 'engines', 'displayName']) {
      expect(manifest[field], field).toBeTruthy()
    }
    expect(manifest.engines.vscode).toMatch(/^\^1\.\d+\.\d+$/)
    expect(manifest.main).toBe('./dist/extension.js')
  })

  it('registers .jev as jevscript and activates on it', () => {
    const [jev] = manifest.contributes.languages
    expect(jev.id).toBe('jevscript')
    expect(jev.extensions).toEqual(['.jev'])
    expect(jev.configuration).toBe('./language-configuration.json')
    expect(manifest.activationEvents).toContain('onLanguage:jevscript')
  })

  it('declares the server modifiers', () => {
    // The server's legend carries `unitKind` and `judgmentVerb` modifiers.
    const ids = manifest.contributes.semanticTokenModifiers.map((m: { id: string }) => m.id)
    expect(ids).toEqual(['unitKind', 'judgmentVerb'])
  })

  it('pins every dependency exactly', () => {
    const all = { ...manifest.dependencies, ...manifest.devDependencies }
    for (const [name, version] of Object.entries(all)) {
      expect(version, name).toMatch(/^\d+\.\d+\.\d+$/)
    }
  })

  it('keeps @types/vscode within the engine it claims', () => {
    const engine = manifest.engines.vscode.replace('^', '')
    expect(manifest.devDependencies['@types/vscode']).toBe(engine)
  })
})

describe('language-configuration.json', () => {
  const increase = new RegExp(language.indentationRules.increaseIndentPattern)
  const decrease = new RegExp(language.indentationRules.decreaseIndentPattern)

  it('comments with # (spec section 2.3)', () => {
    expect(language.comments.lineComment).toBe('#')
  })

  it('indents after a colon or a gate arrow (spec section 2.2)', () => {
    expect(increase.test('task main budget calls 40:')).toBe(true)
    expect(increase.test('  obs = shape:  # capped')).toBe(true)
    expect(increase.test('    proceed  ->')).toBe(true)
    expect(increase.test('  x = "a: b"')).toBe(false)
    expect(increase.test('# note:')).toBe(false)
  })

  it('dedents elif and else', () => {
    expect(decrease.test('  else:')).toBe(true)
    expect(decrease.test('  elif j.next is stuck:')).toBe(true)
    expect(decrease.test('  elsewhere = 1')).toBe(false)
  })

  it('pairs brackets and quotes', () => {
    expect(language.brackets).toEqual([['{', '}'], ['[', ']'], ['(', ')']])
    expect(language.autoClosingPairs.some((p: { open: string }) => p.open === '"')).toBe(true)
  })

  it('reads identifiers as the lexer does (spec section 2.4)', () => {
    const word = new RegExp(`^${language.wordPattern}$`)
    expect(word.test('read_agent')).toBe(true)
    expect(word.test('x2')).toBe(true)
  })
})
