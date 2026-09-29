/** Line icons for the rail, 16px, drawn on a 16 grid. */
const common = { fill: 'none', stroke: 'currentColor', strokeWidth: 1.4, strokeLinecap: 'round' as const, strokeLinejoin: 'round' as const }

export const ChatIcon = () => (
  <svg viewBox="0 0 16 16" {...common}><path d="M2.5 3.5h11v7h-6l-3 2.5v-2.5h-2z" /></svg>
)
export const CodeIcon = () => (
  <svg viewBox="0 0 16 16" {...common}><path d="M5.5 4.5 2 8l3.5 3.5M10.5 4.5 14 8l-3.5 3.5" /></svg>
)
export const MachineIcon = () => (
  <svg viewBox="0 0 16 16" {...common}><circle cx="4" cy="4" r="1.8" /><circle cx="12" cy="12" r="1.8" /><path d="M5.8 4h3.2a2 2 0 0 1 2 2v4.2" /></svg>
)
export const PlugIcon = () => (
  <svg viewBox="0 0 16 16" {...common}><path d="M6 2v3M10 2v3M4.5 5h7v2.5a3.5 3.5 0 0 1-7 0zM8 11v3" /></svg>
)
export const SendIcon = () => (
  <svg viewBox="0 0 16 16" width="16" height="16" {...common} strokeWidth={1.8}><path d="M8 13V3M3.5 7.5 8 3l4.5 4.5" /></svg>
)
export const PlayIcon = () => (
  <svg viewBox="0 0 16 16" width="10" height="10"><path d="M4 2.5v11l9-5.5z" fill="currentColor" /></svg>
)
