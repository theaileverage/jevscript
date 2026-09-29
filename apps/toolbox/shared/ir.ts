/**
 * The slice of the compiled IR the toolbox reads (spec section 11.1).
 *
 * The SDKs deliberately never expose the IR; the toolbox is a developer tool
 * and reads the `jevscript compile` JSON (and the copy embedded in a
 * recording's `start` event, spec section 10.3) for exactly what it draws:
 * needs, budgets, thresholds and machines. The authoritative shape is
 * `crates/jevscript-ir/ir.schema.json`.
 */

export interface Position {
  line: number
  column: number
  offset: number
}

export interface Span {
  start: Position
  end: Position
}

/** Section 9: the four capability kinds. */
export type CapabilityKind = 'agent' | 'person' | 'llm' | 'tool'

/** Section 9.4: a declared tool verb. */
export interface IrSignature {
  name: string
  params?: string[]
  returns: 'text' | 'number' | 'bool' | 'list' | 'record' | 'handle' | 'none'
}

/** Section 3.4. */
export interface IrNeed {
  name: string
  kind: CapabilityKind
  signatures?: IrSignature[]
}

/** Section 7.1. */
export type BudgetKey = 'calls' | 'minutes' | 'usd' | 'steps'
export type Budget = Partial<Record<BudgetKey, number>>

/** Section 7.6. */
export type ThresholdKey = 'risk_confirm' | 'min_confidence' | 'stop_confidence' | 'done' | 'other'
export type Thresholds = Partial<Record<ThresholdKey, number>>

/** An IR expression, typed only as far as the guard printer needs. */
export interface Expr {
  node: string
  [field: string]: unknown
}

/** Section 7.8: one `on` line. */
export interface IrTransition {
  event: string
  description: Expr
  target: string
  when?: Expr | null
  risky: boolean
  body?: unknown[]
  span: Span
}

export interface IrState {
  name: string
  done: boolean
  transitions?: IrTransition[]
  span?: Span
}

/** Section 7.8. */
export interface IrMachine {
  name: string
  params: { name: string }[]
  budget: Budget
  thresholds: Thresholds
  goal?: Expr
  initial: string
  states: IrState[]
  shape_hash: string
  span: Span
}

/** Section 7. */
export interface IrTask {
  name: string
  params?: { name: string }[]
  budget: Budget
  thresholds: Thresholds
  span: Span
}

/** Section 3.2. */
export interface IrInput {
  name: string
  shape: unknown
}

export interface Ir {
  ir_version: string
  program: string
  inputs: IrInput[]
  needs: IrNeed[]
  tasks: IrTask[]
  machines: IrMachine[]
  judgments: { name: string; params: string[] }[]
  file?: string | null
}

/**
 * The IR as the toolbox reads it. The schema lets the compiler leave out
 * empty lists (a program with no machines has no `machines`, a machine with
 * no parameters no `params`), so they are filled in here, where JSON from
 * `jevscript compile` or a recording's `start` event first arrives.
 */
export function readIr(json: unknown): Ir {
  const raw = json as Ir & { machines?: (Omit<IrMachine, 'params'> & { params?: IrMachine['params'] })[] }
  return { ...raw, machines: (raw.machines ?? []).map((machine) => ({ ...machine, params: machine.params ?? [] })) }
}

const BINARY: Record<string, string> = {
  or: 'or',
  and: 'and',
  eq: '==',
  not_eq: '!=',
  lt: '<',
  lt_eq: '<=',
  gt: '>',
  gt_eq: '>=',
  add: '+',
  sub: '-',
  mul: '*',
  div: '/',
  rem: '%',
}

/**
 * Print an expression back as Jevscript source, for guard labels. Covers the
 * forms a `when` guard is written in; anything else prints as `…` rather than
 * as a guess.
 */
export function printExpr(expr: Expr | null | undefined): string {
  if (!expr) return ''
  switch (expr.node) {
    case 'number':
      return String(expr['value'])
    case 'bool':
      return expr['value'] ? 'true' : 'false'
    case 'none':
      return 'none'
    case 'name':
      return String(expr['name'])
    case 'text':
      return JSON.stringify(textLiteral(expr))
    case 'field':
      return `${printExpr(expr['target'] as Expr)}.${String(expr['name'])}`
    case 'index':
      return `${printExpr(expr['target'] as Expr)}[${printExpr(expr['index'] as Expr)}]`
    case 'unary': {
      const operand = printExpr(expr['operand'] as Expr)
      return expr['op'] === 'not' ? `not ${operand}` : `-${operand}`
    }
    case 'binary':
      return `${printExpr(expr['left'] as Expr)} ${BINARY[String(expr['op'])] ?? String(expr['op'])} ${printExpr(expr['right'] as Expr)}`
    case 'is':
      return `${printExpr(expr['target'] as Expr)} is ${String(expr['label'])}`
    case 'call': {
      const args = (expr['args'] as { name?: string | null; value: Expr }[])
        .map((arg) => (arg.name ? `${arg.name} ${printExpr(arg.value)}` : printExpr(arg.value)))
        .join(', ')
      return `${printExpr(expr['callee'] as Expr)}(${args})`
    }
    default:
      return '…'
  }
}

/** The literal text of a `text` node, with interpolations shown as `{…}`. */
export function textLiteral(expr: Expr | null | undefined): string {
  if (!expr || expr.node !== 'text') return ''
  return (expr['parts'] as { part: string; value?: string; expr?: Expr }[])
    .map((part) => (part.part === 'literal' ? (part.value ?? '') : `{${printExpr(part.expr)}}`))
    .join('')
}
