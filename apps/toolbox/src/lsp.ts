/**
 * A small LSP client over the server's `/lsp` WebSocket, and the CodeMirror
 * extensions that use it: semantic-token colouring, diagnostics with a hover
 * card, hover and completion. The server is the `jevscript` language server,
 * started by the toolbox server; when it is not there, editors fall back to
 * plain text and `jevscrypt check` diagnostics.
 */
import { autocompletion, type Completion, type CompletionContext, type CompletionResult } from '@codemirror/autocomplete'
import { type Diagnostic as CmDiagnostic, forEachDiagnostic, linter, lintGutter, setDiagnostics } from '@codemirror/lint'
import { type Extension, RangeSetBuilder, StateEffect, StateField, type Text } from '@codemirror/state'
import { Decoration, type DecorationSet, EditorView, hoverTooltip, ViewPlugin, type ViewUpdate } from '@codemirror/view'

import type { Diagnostic } from '../shared/protocol.ts'
import { decodeTokens, type SemanticLegend, tokenClass } from '../shared/semantic.ts'

interface LspDiagnostic {
  range: { start: { line: number; character: number }; end: { line: number; character: number } }
  severity?: number
  code?: string | number
  message: string
  data?: { specSection?: string }
}

type Listener = (params: unknown) => void

export class LspClient {
  #socket: WebSocket | null = null
  #next = 1
  #pending = new Map<number, (result: unknown) => void>()
  #notifications = new Map<string, Set<Listener>>()
  #ready: Promise<boolean>
  legend: SemanticLegend | null = null
  onState: (state: 'connected' | 'disconnected') => void = () => {}

  constructor() {
    this.#ready = new Promise((resolve) => {
      const socket = new WebSocket(`${location.protocol === 'https:' ? 'wss' : 'ws'}://${location.host}/lsp`)
      this.#socket = socket
      socket.onmessage = (event) => this.#receive(JSON.parse(String(event.data)) as Record<string, unknown>)
      socket.onclose = () => {
        this.onState('disconnected')
        resolve(false)
      }
      socket.onopen = async () => {
        const result = (await this.request('initialize', {
          processId: null,
          rootUri: null,
          capabilities: {
            textDocument: {
              semanticTokens: {
                requests: { full: true },
                formats: ['relative'],
                tokenTypes: ['keyword', 'comment', 'string', 'number', 'operator', 'function', 'method', 'namespace', 'interface', 'parameter', 'variable', 'property', 'enumMember', 'event', 'type'],
                tokenModifiers: ['declaration', 'readonly', 'defaultLibrary', 'unitKind', 'judgmentVerb'],
              },
              hover: { contentFormat: ['markdown', 'plaintext'] },
              completion: { completionItem: { snippetSupport: false } },
              publishDiagnostics: {},
            },
            general: { positionEncodings: ['utf-16'] },
          },
        })) as { capabilities?: { semanticTokensProvider?: { legend?: SemanticLegend } } }
        this.legend = result.capabilities?.semanticTokensProvider?.legend ?? null
        this.notify('initialized', {})
        this.onState('connected')
        resolve(true)
      }
    })
  }

  ready(): Promise<boolean> {
    return this.#ready
  }

  request(method: string, params: unknown): Promise<unknown> {
    const id = this.#next++
    return new Promise((resolve) => {
      this.#pending.set(id, resolve)
      this.#socket?.send(JSON.stringify({ jsonrpc: '2.0', id, method, params }))
    })
  }

  notify(method: string, params: unknown): void {
    if (this.#socket?.readyState === WebSocket.OPEN) this.#socket.send(JSON.stringify({ jsonrpc: '2.0', method, params }))
  }

  on(method: string, listener: Listener): () => void {
    const listeners = this.#notifications.get(method) ?? new Set()
    listeners.add(listener)
    this.#notifications.set(method, listeners)
    return () => listeners.delete(listener)
  }

  #receive(message: Record<string, unknown>): void {
    if (typeof message['id'] === 'number' && !('method' in message)) {
      const resolve = this.#pending.get(message['id'])
      this.#pending.delete(message['id'])
      resolve?.(message['result'] ?? null)
      return
    }
    if (typeof message['method'] === 'string') {
      for (const listener of this.#notifications.get(message['method']) ?? []) listener(message['params'])
    }
  }
}

let shared: LspClient | null = null

/** One connection for the page, opened on first use. */
export function lspClient(onState: (state: 'connected' | 'disconnected') => void): LspClient {
  if (!shared) {
    shared = new LspClient()
    shared.onState = onState
  }
  return shared
}

function offset(doc: Text, position: { line: number; character: number }): number {
  const line = doc.line(Math.min(position.line + 1, doc.lines))
  return Math.min(line.from + position.character, line.to)
}

function position(doc: Text, pos: number): { line: number; character: number } {
  const line = doc.lineAt(pos)
  return { line: line.number - 1, character: pos - line.from }
}

const setTokens = StateEffect.define<DecorationSet>()

const tokenField = StateField.define<DecorationSet>({
  create: () => Decoration.none,
  update(value, transaction) {
    let next = value.map(transaction.changes)
    for (const effect of transaction.effects) if (effect.is(setTokens)) next = effect.value
    return next
  },
  provide: (field) => EditorView.decorations.from(field),
})

/** Plain `jevscrypt check` diagnostics, for when there is no language server. */
export function checkDiagnostics(doc: Text, diagnostics: readonly Diagnostic[], reference: ErrorReference): CmDiagnostic[] {
  return diagnostics.map((diagnostic) => {
    const line = doc.line(Math.max(1, Math.min(diagnostic.line, doc.lines)))
    const from = Math.min(line.from + diagnostic.column, line.to)
    return {
      from,
      to: Math.max(from + 1, line.to),
      severity: diagnostic.severity,
      message: diagnostic.message,
      renderMessage: () => diagnosticCard(diagnostic.code, diagnostic.message, null, reference, 'jevscrypt check'),
    }
  })
}

export type ErrorReference = Record<string, { meaning: string; correction: string }>

/** The hover card: code, source, spec section, message and the error reference's cause. */
export function diagnosticCard(code: string, message: string, spec: string | null, reference: ErrorReference, source: string): HTMLElement {
  const card = document.createElement('div')
  card.className = 'diag-card'
  const head = document.createElement('div')
  head.className = 'diag-head'
  const name = document.createElement('span')
  name.className = 'diag-code'
  name.textContent = code
  const origin = document.createElement('span')
  origin.className = 'diag-origin'
  origin.textContent = spec ? `${source} · spec ${spec}` : source
  head.append(name, origin)
  const body = document.createElement('div')
  body.className = 'diag-message'
  body.textContent = message.replace(/\s*\(spec section [\d.a-z]+\)$/, '')
  card.append(head, body)
  const cause = reference[code]?.meaning
  if (cause) {
    const note = document.createElement('div')
    note.className = 'diag-cause'
    note.textContent = cause
    card.append(note)
  }
  return card
}

export interface LspDocOptions {
  client: LspClient
  uri: string
  readOnly?: boolean
  reference: () => ErrorReference
  /** Receives the server's diagnostics, converted to `file:line:col` form, for the console. */
  onDiagnostics?: (diagnostics: Diagnostic[]) => void
  fileName: string
}

/** Everything an editor needs from the language server for one document. */
export function lspDocument(options: LspDocOptions): Extension {
  const { client, uri } = options
  const plugin = ViewPlugin.fromClass(
    class {
      version = 1
      timer: ReturnType<typeof setTimeout> | null = null
      stop: () => void
      readonly view: EditorView
      constructor(view: EditorView) {
        this.view = view
        void client.ready().then((ok) => {
          if (!ok) return
          client.notify('textDocument/didOpen', {
            textDocument: { uri, languageId: 'jevscrypt', version: this.version, text: view.state.doc.toString() },
          })
          void this.tokens()
        })
        this.stop = client.on('textDocument/publishDiagnostics', (params) => this.diagnostics(params as { uri: string; diagnostics: LspDiagnostic[] }))
      }
      update(update: ViewUpdate) {
        if (!update.docChanged) return
        if (this.timer) clearTimeout(this.timer)
        this.timer = setTimeout(() => {
          this.version += 1
          client.notify('textDocument/didChange', {
            textDocument: { uri, version: this.version },
            contentChanges: [{ text: this.view.state.doc.toString() }],
          })
          void this.tokens()
        }, 150)
      }
      async tokens() {
        const legend = client.legend
        if (!legend) return
        const result = (await client.request('textDocument/semanticTokens/full', { textDocument: { uri } })) as { data?: number[] } | null
        const doc = this.view.state.doc
        const builder = new RangeSetBuilder<Decoration>()
        for (const token of decodeTokens(result?.data ?? [], legend)) {
          if (token.line >= doc.lines) continue
          const from = offset(doc, { line: token.line, character: token.character })
          const to = Math.min(from + token.length, doc.line(token.line + 1).to)
          if (to > from) builder.add(from, to, Decoration.mark({ class: tokenClass(token) }))
        }
        this.view.dispatch({ effects: setTokens.of(builder.finish()) })
      }
      diagnostics(params: { uri: string; diagnostics: LspDiagnostic[] }) {
        if (params.uri !== uri) return
        const doc = this.view.state.doc
        const reference = options.reference()
        const converted: CmDiagnostic[] = params.diagnostics.map((diagnostic) => {
          const from = offset(doc, diagnostic.range.start)
          const to = Math.max(offset(doc, diagnostic.range.end), from + 1)
          const code = String(diagnostic.code ?? 'diagnostic')
          return {
            from,
            to: Math.min(to, doc.length),
            severity: diagnostic.severity === 1 ? 'error' : diagnostic.severity === 2 ? 'warning' : 'info',
            message: diagnostic.message,
            renderMessage: () => diagnosticCard(code, diagnostic.message, diagnostic.data?.specSection ?? null, reference, 'jevscript lsp'),
          }
        })
        this.view.dispatch(setDiagnostics(this.view.state, converted))
        options.onDiagnostics?.(
          params.diagnostics.map((diagnostic) => ({
            file: options.fileName,
            line: diagnostic.range.start.line + 1,
            column: diagnostic.range.start.character,
            severity: diagnostic.severity === 1 ? 'error' : 'warning',
            code: String(diagnostic.code ?? 'diagnostic'),
            message: diagnostic.message,
          })),
        )
      }
      destroy() {
        this.stop()
        if (this.timer) clearTimeout(this.timer)
        client.notify('textDocument/didClose', { textDocument: { uri } })
      }
    },
  )

  const hover = hoverTooltip(async (view, pos) => {
    let covered = false
    forEachDiagnostic(view.state, (_diagnostic, from, to) => {
      if (from <= pos && pos <= to) covered = true
    })
    if (covered) return null
    const result = (await client.request('textDocument/hover', { textDocument: { uri }, position: position(view.state.doc, pos) })) as {
      contents?: { value?: string } | string
    } | null
    const text = typeof result?.contents === 'string' ? result.contents : result?.contents?.value
    if (!text) return null
    return {
      pos,
      create: () => {
        const dom = document.createElement('div')
        dom.className = 'hover-card'
        renderMarkdown(dom, text)
        return { dom }
      },
    }
  })

  const completion = autocompletion({
    override: [
      async (context: CompletionContext): Promise<CompletionResult | null> => {
        const word = context.matchBefore(/[A-Za-z_][A-Za-z0-9_]*/)
        if (!word && !context.explicit && context.matchBefore(/\.$/) === null) return null
        const items = (await client.request('textDocument/completion', {
          textDocument: { uri },
          position: position(context.state.doc, context.pos),
        })) as { label: string; detail?: string; kind?: number; documentation?: { value?: string } | string }[] | { items: [] } | null
        const list = Array.isArray(items) ? items : (items?.items ?? [])
        return {
          from: word?.from ?? context.pos,
          options: list.map(
            (item): Completion => ({
              label: item.label,
              ...(item.detail ? { detail: item.detail } : {}),
              type: COMPLETION_KINDS[item.kind ?? 0] ?? 'text',
              ...(item.documentation
                ? { info: typeof item.documentation === 'string' ? item.documentation : (item.documentation.value ?? '') }
                : {}),
            }),
          ),
        }
      },
    ],
  })

  return [tokenField, plugin, lintGutter(), ...(options.readOnly ? [] : [hover, completion])]
}

/** Plain diagnostics with no language server. */
export function staticLint(get: () => CmDiagnostic[]): Extension {
  return [linter(() => get(), { delay: 100 }), lintGutter()]
}

const COMPLETION_KINDS: Record<number, string> = { 3: 'function', 6: 'variable', 7: 'class', 8: 'interface', 9: 'namespace', 14: 'keyword' }

/** Enough markdown for hover text: fenced code, `code`, *emphasis*. Text only; never HTML from the server. */
function renderMarkdown(into: HTMLElement, text: string): void {
  const parts = text.split(/```[a-z]*\n?/)
  parts.forEach((part, index) => {
    if (!part.trim()) return
    if (index % 2 === 1) {
      const pre = document.createElement('pre')
      pre.textContent = part.replace(/\n$/, '')
      into.append(pre)
      return
    }
    const paragraph = document.createElement('p')
    for (const piece of part.trim().split(/(`[^`]+`|\*[^*]+\*)/)) {
      if (piece.startsWith('`') && piece.endsWith('`')) {
        const code = document.createElement('code')
        code.textContent = piece.slice(1, -1)
        paragraph.append(code)
      } else if (piece.startsWith('*') && piece.endsWith('*') && piece.length > 2) {
        const em = document.createElement('em')
        em.textContent = piece.slice(1, -1)
        paragraph.append(em)
      } else paragraph.append(piece)
    }
    into.append(paragraph)
  })
}
