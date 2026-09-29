/**
 * Claude, for the two places the toolbox writes Jevscript: drafting a program
 * from an idea in Chat, and answering or editing at a pin in the Machines
 * annotator. The spec goes in the system prompt, cached. Every program Claude
 * writes is compiled before anyone sees it, and compile errors go back to
 * Claude for at most two repairs.
 */
import { randomUUID } from 'node:crypto'

import Anthropic from '@anthropic-ai/sdk'
import type { BetaContentBlock, BetaMessageParam } from '@anthropic-ai/sdk/resources/beta/messages/messages'

import {
  applyEdits,
  EditError,
  lineDiff,
  newWarnings,
  nextPinNumber,
  type Pin,
  type PinReply,
  type PinTarget,
  reachabilityChanges,
  type SourceEdit,
} from '../shared/annotator.ts'
import { machineGraph } from '../shared/graph.ts'
import type { Ir, IrMachine } from '../shared/ir.ts'
import { printExpr, textLiteral } from '../shared/ir.ts'
import type { ChatMessage, ChatOrigin, CheckResult, DraftProgram, Idea } from '../shared/protocol.ts'
import { formatDiagnostic } from '../shared/protocol.ts'

/** One model turn: the raw content to keep in history, and its text. */
export interface Completion {
  content: unknown[]
  text: string
  model?: string
}

/** The model, behind an interface so tests can stand in for the API. */
export interface Model {
  readonly name: string
  complete(system: string, messages: unknown[], options?: { json?: Record<string, unknown> }): Promise<Completion>
}

/** Repairs after the first draft (the brief's "up to 2 repair turns"). */
export const MAX_REPAIRS = 2

export class AnthropicModel implements Model {
  readonly name: string
  readonly #client: Anthropic

  constructor(name: string, apiKey?: string) {
    this.name = name
    this.#client = new Anthropic(apiKey ? { apiKey } : {})
  }

  async complete(system: string, messages: unknown[], options: { json?: Record<string, unknown> } = {}): Promise<Completion> {
    const message = await this.#client.beta.messages
      .stream({
        model: this.name,
        max_tokens: 32_000,
        betas: ['server-side-fallback-2026-07-01'],
        fallbacks: 'default',
        thinking: { type: 'adaptive' },
        output_config: {
          effort: 'high',
          ...(options.json ? { format: { type: 'json_schema', schema: options.json } } : {}),
        },
        system: [{ type: 'text', text: system, cache_control: { type: 'ephemeral' } }],
        messages: messages as BetaMessageParam[],
      })
      .finalMessage()
    if (message.stop_reason === 'refusal') throw new Error('Claude declined this request.')
    const text = message.content
      .filter((block: BetaContentBlock) => block.type === 'text')
      .map((block) => (block as { text: string }).text)
      .join('')
    return { content: message.content, text }
  }
}

/** Compiles a program; the server passes `Jevscript.compile`. */
export type Compile = (fileName: string, source: string) => Promise<CheckResult>

export function systemPrompt(spec: string): string {
  return [
    'You write programs in Jevscript for developers testing ideas in "jevs toolbox".',
    'The language specification below is the only authority on syntax and semantics. Do not use a keyword, verb, label form or builtin it does not define. Reserved words cannot be names. A `pick` needs an `other` (or `none`) label. There is no inline if/else expression.',
    'When you write or change a program, reply with a short explanation and then the complete program in exactly one ```jev fenced block. The toolbox compiles it with `jevscript check` and sends you any errors to fix.',
    '<specification>',
    spec,
    '</specification>',
  ].join('\n\n')
}

/** A fence opens where a line ends in ``` and an optional language, with no ``` before it. */
const OPEN_FENCE = /^((?:(?!```).)*)```[ \t]*([A-Za-z0-9_+-]*)[ \t]*$/
/** A fence closes at the next ```; anything after it on that line is prose again. */
const CLOSE_FENCE = /^(.*?)```(.*)$/

/**
 * The first ```jev (or bare ```) block in a reply is the program; the prose is
 * the reply with every fenced block removed, so Chat never shows code that was
 * not checked. `extra` counts the fenced blocks dropped from view.
 */
export function extractProgram(text: string): { program: string | null; prose: string; extra: number } {
  const blocks: { language: string; lines: string[] }[] = []
  const prose: string[] = []
  let open: { language: string; lines: string[] } | null = null
  for (const line of text.split('\n')) {
    if (open) {
      const close = CLOSE_FENCE.exec(line)
      if (!close) {
        open.lines.push(line)
        continue
      }
      if (close[1]) open.lines.push(close[1])
      if ((close[2] as string).trim()) prose.push((close[2] as string).trim())
      blocks.push(open)
      open = null
      continue
    }
    const start = OPEN_FENCE.exec(line)
    if (!start) {
      prose.push(line)
      continue
    }
    if ((start[1] as string).trim()) prose.push((start[1] as string).trimEnd())
    open = { language: (start[2] as string).toLowerCase(), lines: [] }
  }
  if (open) blocks.push(open)
  const found = blocks.find((block) => ['', 'jev', 'jevscript'].includes(block.language))
  return {
    program: found ? found.lines.join('\n').replace(/\s+$/, '') + '\n' : null,
    prose: prose.join('\n').replace(/\n{3,}/g, '\n\n').trim(),
    extra: blocks.length - (found ? 1 : 0),
  }
}

/** A message that is only a program, pasted: checked, never sent to a model. */
export function pastedProgram(text: string): string | null {
  const { program, prose, extra } = extractProgram(text)
  if (program && extra === 0 && prose.length < 40) return program
  if (!program && extra === 0 && /^(program|task|judgment|machine|needs|in)\s/m.test(text) && /:\s*$/m.test(text)) {
    return text.trimEnd() + '\n'
  }
  return null
}

/** `program <name>` names the file; otherwise the idea keeps its file name. */
export function programName(source: string): string | null {
  return /^program\s+([A-Za-z_][A-Za-z0-9_]*)/m.exec(source)?.[1] ?? null
}

export interface ChatDeps {
  model: Model | null
  compile: Compile
  spec: string
  origin?: ChatOrigin
  unavailable?: string
}

function message(role: ChatMessage['role'], text: string, program?: DraftProgram, origin?: ChatOrigin): ChatMessage {
  return { id: randomUUID(), role, text, at: new Date().toISOString(), ...(program ? { program } : {}), ...(origin ? { origin } : {}) }
}

function adopt(idea: Idea, program: DraftProgram): Idea {
  if (!program.clean) return idea
  const name = programName(program.source)
  return { ...idea, source: program.source, fileName: name ? `${name}.jev` : idea.fileName }
}

async function checked(compile: Compile, fileName: string, source: string, attempts: number): Promise<DraftProgram> {
  const result = await compile(fileName, source)
  const clean = result.ir !== null && !result.diagnostics.some((diagnostic) => diagnostic.severity === 'error')
  return { source, diagnostics: result.diagnostics, clean, attempts }
}

/**
 * One chat turn. A pasted program is checked and adopted if clean. Otherwise
 * Claude drafts; each draft is compiled, and errors go back for at most
 * {@link MAX_REPAIRS} repairs. The history sent to Claude is only ever
 * appended to, so earlier turns (and their thinking) resend unchanged.
 */
export async function chatTurn(idea: Idea, text: string, deps: ChatDeps): Promise<Idea> {
  let next: Idea = {
    ...idea,
    title: idea.title === 'Untitled idea' && idea.messages.length === 0 ? titleFrom(text) : idea.title,
    description: idea.description || (idea.messages.length === 0 ? text : ''),
    messages: [...idea.messages, message('user', text)],
  }

  const pasted = pastedProgram(text)
  if (pasted !== null) {
    const program = await checked(deps.compile, programName(pasted) ? `${programName(pasted)}.jev` : next.fileName, pasted, 0)
    const reply = program.clean
      ? 'Checked. It compiles, so it is now this idea’s program.'
      : 'Checked. It does not compile yet; the diagnostics are below.'
    return adopt({ ...next, messages: [...next.messages, message('assistant', reply, program, { kind: 'check' })] }, program)
  }

  if (!deps.model) {
    return {
      ...next,
      messages: [
        ...next.messages,
        message(
          'assistant',
          deps.unavailable ?? 'Drafting needs a signed-in local agent. Choose Claude Code or Codex, or paste a program to check it. Legacy API drafting needs ANTHROPIC_API_KEY.',
          undefined,
          deps.origin,
        ),
      ],
    }
  }

  const context = next.chatAgent
    ? `\n\nIdea details: ${next.title}\n${next.description}\nThis idea's workspace (entry Jev file: ${next.fileName}). Companion Jev files are available to use; host files are saved alongside the program and run only on an explicit Run pair action:\n${JSON.stringify(next.workspace.files)}`
    : next.source.trim() ? `\n\nThe idea's current program (${next.fileName}):\n\`\`\`jev\n${next.source}\`\`\`` : ''
  const prior = next.chatAgent
    ? idea.messages.filter(item => item.origin?.kind !== 'error').map(item => ({ role: item.role, content: item.text + (item.program ? `\n\`\`\`jev\n${item.program.source}\`\`\`` : '') }))
    : next.claudeHistory
  let history = [...prior, { role: 'user', content: text + context }]
  const system = systemPrompt(deps.spec)
  try {
    let completion = await deps.model.complete(system, history)
    history = [...history, { role: 'assistant', content: completion.content }]
    const { prose, extra } = extractProgram(completion.text)
    let program: DraftProgram | undefined
    for (let attempt = 1; ; attempt++) {
      const draft = extractProgram(completion.text).program
      if (draft === null) break
      program = await checked(deps.compile, programName(draft) ? `${programName(draft)}.jev` : next.fileName, draft, attempt)
      if (program.clean || attempt > MAX_REPAIRS) break
      const report = program.diagnostics.filter((d) => d.severity === 'error').map(formatDiagnostic).join('\n')
      history = [
        ...history,
        {
          role: 'user',
          content: `jevscript check reported:\n${report}\n\nFix the program and reply with the complete corrected program in one \`\`\`jev block.`,
        },
      ]
      completion = await deps.model.complete(system, history)
      history = [...history, { role: 'assistant', content: completion.content }]
    }

    next = { ...next, claudeHistory: next.chatAgent ? next.claudeHistory : history }
    const repaired = program && program.attempts > 1 ? ` (${program.attempts - 1} repair${program.attempts > 2 ? 's' : ''} after check)` : ''
    const hidden = extra > 0 ? `\n\n(${extra === 1 ? 'Another code block' : `${extra} other code blocks`} in the reply went unchecked and ${extra === 1 ? 'is' : 'are'} not shown.)` : ''
    const reply = program
      ? `${prose || 'Here is a draft.'}${program.clean ? repaired : `\n\nIt still does not compile after ${MAX_REPAIRS} repairs; the diagnostics are below.`}${hidden}`
      : `${prose}${hidden}`.trim()
    const origin = deps.origin?.kind === 'agent' ? { ...deps.origin, model: completion.model ?? deps.origin.model } : deps.origin
    return adopt({ ...next, messages: [...next.messages, message('assistant', reply, program, origin)] }, program ?? { source: '', diagnostics: [], clean: false, attempts: 0 })
  } catch (error) {
    if (!next.chatAgent) throw error
    return { ...next, messages: [...next.messages, message('assistant', error instanceof Error ? error.message : 'The local agent failed.', undefined, { kind: 'error', ...next.chatAgent })] }
  }
}

function titleFrom(text: string): string {
  const first = text.trim().split(/[.\n]/)[0] ?? ''
  return first.length > 48 ? `${first.slice(0, 45)}…` : first || 'Untitled idea'
}

/** The machine as the annotator describes it to Claude: the compiled IR, without spans or action trees. */
export function machineFacts(machine: IrMachine): unknown {
  return {
    name: machine.name,
    params: machine.params.map((param) => param.name),
    initial: machine.initial,
    budget: machine.budget,
    thresholds: machine.thresholds,
    goal: textLiteral(machine.goal),
    states: machine.states.map((state) => ({
      name: state.name,
      terminal: state.done,
      events: (state.transitions ?? []).map((transition) => ({
        event: transition.event,
        description: textLiteral(transition.description),
        target: transition.target,
        guard: transition.when ? printExpr(transition.when) : null,
        risky: transition.risky,
        has_actions: (transition.body?.length ?? 0) > 0,
        line: transition.span.start.line,
      })),
    })),
  }
}

const REPLY_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  required: ['kind', 'text', 'edits'],
  properties: {
    kind: { type: 'string', enum: ['answer', 'edit'] },
    text: { type: 'string' },
    edits: {
      type: 'array',
      items: {
        type: 'object',
        additionalProperties: false,
        required: ['find', 'replace'],
        properties: { find: { type: 'string' }, replace: { type: 'string' } },
      },
    },
  },
}

export interface AnnotateDeps extends ChatDeps {
  /** The idea's program as it compiles now; edits are judged against it. */
  current: CheckResult
}

/**
 * Answer a question at a pin, or propose an edit and check it. The prompt
 * carries the source with line numbers and the machine's compiled facts
 * (guards, targets, risky flags), so answers rest on the IR rather than on
 * what the model expects a machine to look like.
 */
export async function annotate(idea: Idea, target: PinTarget, query: string, deps: AnnotateDeps): Promise<Pin> {
  const base: Pin = {
    id: randomUUID(),
    number: target.kind === 'machine' ? null : nextPinNumber(idea.pins, target.machine),
    target,
    query,
    reply: null,
    status: 'open',
    createdAt: new Date().toISOString(),
  }
  const machine = deps.current.ir?.machines.find((candidate) => candidate.name === target.machine)
  if (!machine) return { ...base, reply: { kind: 'error', text: `The program no longer compiles to a machine \`${target.machine}\`.` } }
  if (!deps.model) {
    return { ...base, reply: { kind: 'error', text: 'Annotating needs ANTHROPIC_API_KEY, which this server does not have.' } }
  }

  const numbered = idea.source
    .split('\n')
    .map((line, index) => `${String(index + 1).padStart(3)}  ${line}`)
    .join('\n')
  const where =
    target.kind === 'machine'
      ? `the whole machine \`${machine.name}\``
      : target.kind === 'state'
        ? `the state \`${target.state}\` of machine \`${machine.name}\``
        : `the event \`${target.event}\` out of state \`${target.from}\` in machine \`${machine.name}\``
  const prompt = [
    `The user is looking at ${where} and wrote:\n\n${query}`,
    `Source (${idea.fileName}), with line numbers for reference only:\n${numbered}`,
    `Compiled facts for this machine, from the IR:\n${JSON.stringify(machineFacts(machine), null, 2)}`,
    'If this is a question, answer it from the source and the compiled facts in two or three sentences, with kind "answer" and no edits. Say so if the facts do not settle it.',
    'If it asks for a change, reply with kind "edit", one sentence in text, and edits: exact find/replace pairs against the source (no line numbers). Each find must occur exactly once in the source; include enough surrounding text to make it unique. Change only what the request needs.',
  ].join('\n\n')

  let reply: { kind: 'answer' | 'edit'; text: string; edits: SourceEdit[] }
  try {
    const completion = await deps.model.complete(systemPrompt(deps.spec), [{ role: 'user', content: prompt }], { json: REPLY_SCHEMA })
    reply = JSON.parse(completion.text) as typeof reply
  } catch (error) {
    return { ...base, reply: { kind: 'error', text: error instanceof Error ? error.message : String(error) } }
  }
  if (reply.kind === 'answer' || reply.edits.length === 0) return { ...base, reply: { kind: 'answer', text: reply.text } }
  return { ...base, reply: await checkEdit(idea, reply.text, reply.edits, deps) }
}

/** Patch, compile and compare: errors, only the warnings the edit introduced, and reachability. */
export async function checkEdit(
  idea: Idea,
  text: string,
  edits: SourceEdit[],
  deps: Pick<AnnotateDeps, 'compile' | 'current'>,
): Promise<PinReply> {
  let patched: string
  try {
    patched = applyEdits(idea.source, edits)
  } catch (error) {
    if (!(error instanceof EditError)) throw error
    return { kind: 'edit', text, edits, diff: [], check: null, refused: error.message }
  }
  const after = await deps.compile(idea.fileName, patched)
  const graphOf = (ir: Ir | null, name: string) => {
    const machine = ir?.machines.find((candidate) => candidate.name === name)
    return machine ? machineGraph(machine) : null
  }
  const names = new Set([...(deps.current.ir?.machines ?? []), ...(after.ir?.machines ?? [])].map((machine) => machine.name))
  return {
    kind: 'edit',
    text,
    edits,
    diff: lineDiff(idea.source, patched),
    refused: null,
    check: {
      errors: after.diagnostics.filter((diagnostic) => diagnostic.severity === 'error'),
      newWarnings: newWarnings(deps.current.diagnostics, after.diagnostics),
      reachability: [...names].flatMap((name) =>
        reachabilityChanges(graphOf(deps.current.ir, name), graphOf(after.ir, name)),
      ),
    },
  }
}
