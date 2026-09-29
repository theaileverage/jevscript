/**
 * A `.jev` editor. With the language server it gets semantic colours,
 * diagnostics with hover cards, hover and completion; without it, plain text
 * and the `jevscrypt check` diagnostics. The text passed in is the source of
 * truth: when it changes from outside (a dial, an applied edit), the editor
 * takes it.
 */
import { defaultKeymap, history, historyKeymap } from '@codemirror/commands'
import { Compartment, EditorState, Prec } from '@codemirror/state'
import { EditorView, keymap, lineNumbers } from '@codemirror/view'
import { useEffect, useRef } from 'react'

import type { Diagnostic } from '../../shared/protocol.ts'
import { checkDiagnostics, lspClient, lspDocument, staticLint } from '../lsp.ts'
import { actions, getState, useStore } from '../store.ts'

export interface JevEditorProps {
  value: string
  uri: string
  fileName: string
  onChange?: (value: string) => void
  readOnly?: boolean
  /** `jevscrypt check` diagnostics, used when the language server is unavailable. */
  fallback?: readonly Diagnostic[]
  onDiagnostics?: (diagnostics: Diagnostic[]) => void
  onRun?: () => void
  className?: string
}

export function JevEditor(props: JevEditorProps) {
  const host = useRef<HTMLDivElement>(null)
  const view = useRef<EditorView | null>(null)
  const latest = useRef(props)
  latest.current = props
  const language = useRef(new Compartment())
  const lsp = useStore((s) => s.lsp)
  const available = useStore((s) => s.status?.lsp.available ?? false)

  useEffect(() => {
    if (!host.current) return
    const editor = new EditorView({
      parent: host.current,
      state: EditorState.create({
        doc: props.value,
        extensions: [
          ...(props.readOnly ? [EditorState.readOnly.of(true), EditorView.editable.of(false)] : [lineNumbers(), history()]),
          Prec.highest(
            keymap.of([
              {
                key: 'Mod-Enter',
                run: () => {
                  latest.current.onRun?.()
                  return Boolean(latest.current.onRun)
                },
              },
            ]),
          ),
          keymap.of([...defaultKeymap, ...historyKeymap]),
          language.current.of([]),
          EditorView.updateListener.of((update) => {
            if (update.docChanged) latest.current.onChange?.(update.state.doc.toString())
          }),
        ],
      }),
    })
    view.current = editor
    return () => {
      editor.destroy()
      view.current = null
    }
    // The editor is created once per document; later props flow in through the effects below.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [props.uri])

  useEffect(() => {
    const editor = view.current
    if (!editor) return
    if (available && lsp !== 'disconnected') {
      const client = lspClient((state) => actions.setLsp(state))
      editor.dispatch({
        effects: language.current.reconfigure(
          lspDocument({
            client,
            uri: props.uri,
            fileName: props.fileName,
            readOnly: props.readOnly ?? false,
            reference: () => getState().errorReference,
            onDiagnostics: (diagnostics) => latest.current.onDiagnostics?.(diagnostics),
          }),
        ),
      })
    } else {
      editor.dispatch({
        effects: language.current.reconfigure(
          props.readOnly
            ? []
            : staticLint(() => checkDiagnostics(editor.state.doc, latest.current.fallback ?? [], getState().errorReference)),
        ),
      })
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [available, lsp === 'disconnected', props.uri])

  useEffect(() => {
    const editor = view.current
    if (!editor) return
    const current = editor.state.doc.toString()
    if (current !== props.value) editor.dispatch({ changes: { from: 0, to: current.length, insert: props.value } })
  }, [props.value])

  return <div ref={host} className={`jev-editor ${props.readOnly ? 'read-only' : ''} ${props.className ?? ''}`} />
}
