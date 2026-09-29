/** Highlighted companion host editing over the section 11.2 SDK languages. */
import { defaultKeymap, history, historyKeymap } from '@codemirror/commands'
import { javascript } from '@codemirror/lang-javascript'
import { json } from '@codemirror/lang-json'
import { python } from '@codemirror/lang-python'
import { defaultHighlightStyle, syntaxHighlighting } from '@codemirror/language'
import { EditorState } from '@codemirror/state'
import { EditorView, keymap, lineNumbers } from '@codemirror/view'
import { useEffect, useRef } from 'react'

export function HostEditor(props: { id: string; name: string; value: string; onChange: (source: string) => void; onSelection: (selection: { from: number; to: number }) => void }) {
  const root = useRef<HTMLDivElement>(null)
  const view = useRef<EditorView | null>(null)
  const latest = useRef(props)
  latest.current = props
  useEffect(() => {
    const editor = new EditorView({
      parent: root.current!,
      state: EditorState.create({ doc: latest.current.value, extensions: [
        lineNumbers(), history(), keymap.of([...defaultKeymap, ...historyKeymap]),
        props.name.endsWith('.json') ? json() : props.name.endsWith('.py') ? python() : javascript({ typescript: /\.m?ts$/.test(props.name) }),
        syntaxHighlighting(defaultHighlightStyle),
        EditorView.contentAttributes.of({ 'aria-label': 'Host file source' }),
        EditorView.updateListener.of(update => {
          if (update.docChanged) latest.current.onChange(update.state.doc.toString())
          if (update.selectionSet) {
            const { from, to } = update.state.selection.main
            latest.current.onSelection({ from, to })
          }
        }),
      ] }),
    })
    view.current = editor
    return () => { editor.destroy(); view.current = null }
  }, [props.id, props.name])
  useEffect(() => {
    const editor = view.current
    if (editor && editor.state.doc.toString() !== props.value) editor.dispatch({ changes: { from: 0, to: editor.state.doc.length, insert: props.value } })
  }, [props.value])
  return <div ref={root} className="host-source workspace-editor" />
}
