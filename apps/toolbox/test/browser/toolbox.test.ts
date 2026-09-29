/**
 * The four screens in a real browser, against the production server started
 * exactly as `pnpm start` starts it, the real `jevscript` binary and language
 * server, and the fixture TypeSafe and Anthropic endpoints from
 * `demo/services.ts`. No live service is called.
 *
 * Needs `pnpm build` first (`pnpm test:browser` does it) and Chrome; set
 * `JEVS_BROWSER` to a Chrome or Chromium executable to use another one.
 */
import { type ChildProcess, spawn } from 'node:child_process'
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { createServer } from 'node:net'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { type Browser, chromium, type Page } from 'playwright-core'
import { afterAll, beforeAll, describe, expect, it } from 'vitest'

import { APP_ROOT } from '../../server/env.ts'
import type { Idea } from '../../shared/protocol.ts'
import { type Fixtures, startFixtures } from '../../demo/services.ts'
import { connect } from '../harness.ts'
import { agentCalls, fakeAgents } from '../agent-fixture.ts'

/** Pauses on a person and makes no Jev call. */
const ASK = `program ask_me

needs me: person

task main:
  a = me.ask "Go ahead?"
  me.notify "answered {a.answer}"
`

/** Every pause card kind in one run: a retryable adapter error, the budget, then a confirm with declared options. */
const TUNED = `program tuned

needs t: tool:
  ping() -> bool
needs me: person

judgment urgent(message):
  now = message feels "needs a reply today"

task main budget calls 1, steps 2k thresholds min_confidence 0.9:  # tuned
  t.ping
  first = urgent("Invoice overdue")
  second = urgent("Lunch on Friday?")
  lanes = ["today", "week", "ignore"]
  lane = me.ask "Which lane?", options lanes
  me.notify "lane {lane.answer}"
`

/** A JSONL adapter whose first call fails retryably. */
const FLAKY = `n=0
while read line; do n=$((n+1)); if [ $n -eq 1 ]; then echo '{"error":{"message":"warming up","retryable":true}}'; else echo '{"result": true}'; fi; done
`

let fixtures: Fixtures
let server: ChildProcess
let browser: Browser
let page: Page
let home: string
let url: string
let port: number
let agentEnv: NodeJS.ProcessEnv
const shots: string[] = []

async function freePort(): Promise<number> {
  const probe = createServer()
  await new Promise<void>((done) => probe.listen(0, '127.0.0.1', done))
  const { port } = probe.address() as { port: number }
  await new Promise((done) => probe.close(done))
  return port
}

/** Screenshots go to `JEVS_EVIDENCE` when set (the verify-toolbox skill sets it), else the run's temporary folder. */
async function shot(name: string): Promise<void> {
  const dir = process.env['JEVS_EVIDENCE'] ?? home
  await mkdir(dir, { recursive: true })
  const path = join(dir, `${shots.length + 1}-${name}.png`)
  await page.screenshot({ path, fullPage: true })
  shots.push(path)
}

const screen = (name: string) => page.locator('.rail .rail-item', { hasText: new RegExp(`^${name}`) }).click()
const idea = (title: string) => page.locator('.rail .idea-item', { hasText: title }).click()
const editorText = () => page.locator('.editor-area .cm-content').innerText()

/** `pnpm start`'s server on the fixtures; its state lives in `<home>/toolbox`, so a restart keeps it. */
async function startServer(): Promise<void> {
  server = spawn(process.execPath, ['server/main.ts'], {
    cwd: APP_ROOT,
    env: { ...process.env, ...fixtures.env, ...agentEnv, NODE_ENV: 'production', JEVS_TOOLBOX_HOME: join(home, 'toolbox'), JEVS_TOOLBOX_PORT: String(port) },
    stdio: ['ignore', 'pipe', 'pipe'],
  })
  let log = ''
  server.stdout?.on('data', (chunk: Buffer) => (log += chunk.toString()))
  server.stderr?.on('data', (chunk: Buffer) => (log += chunk.toString()))
  const deadline = Date.now() + 20_000
  while (!log.includes('  Jev: ')) {
    if (Date.now() > deadline || server.exitCode !== null) throw new Error(`the toolbox did not start:\n${log}`)
    await new Promise((resolve) => setTimeout(resolve, 50))
  }
  expect(log).toContain(`state: ${join(home, 'toolbox', 'toolbox.sqlite')}`)
  expect(log).toContain('chat drafting: signed-in Claude Code or Codex')
  expect(log).toContain('Jev: TYPESAFE_API_KEY set')
}

beforeAll(async () => {
  home = await mkdtemp(join(tmpdir(), 'jevs-browser-'))
  fixtures = await startFixtures(home)
  agentEnv = await fakeAgents(home, await readFile(join(APP_ROOT, 'fixtures/inbox_triage.jev'), 'utf8'))
  await writeFile(join(home, 'flaky.sh'), FLAKY)
  port = await freePort()
  url = `http://127.0.0.1:${port}`
  await startServer()
  const executablePath = process.env['JEVS_BROWSER']
  browser = await chromium.launch(executablePath ? { executablePath } : { channel: 'chrome' })
  page = await browser.newPage({ viewport: { width: 1440, height: 1000 } })
  page.setDefaultTimeout(20_000)
  await page.goto(url)
  await page.locator('.rail .idea-item', { hasText: 'Review loop for Claude' }).waitFor()
})

afterAll(async () => {
  await browser?.close()
  server?.kill()
  await fixtures?.close()
  if (shots.length > 0) console.log(`screenshots:\n${shots.join('\n')}`)
})

describe('the toolbox in a browser', () => {
  it('Chat: drafts a checked program, runs it, and answers slash commands', async () => {
    await page.getByRole('button', { name: /New idea/ }).click()
    const composer = page.getByPlaceholder('Describe an idea, or ask the agent to change the program…')
    await page.getByRole('combobox', { name: 'Harness model' }).selectOption('claude-code:sonnet')
    await composer.fill('Sort my inbox by urgency')
    await composer.press('Enter')
    const block = page.locator('.codeblock').last()
    await expect.poll(() => block.locator('.bar-top').innerText()).toContain('jevscript check: 0 errors, 0 warnings')
    await expect.poll(() => page.locator('.msg-jev .text').last().innerText()).toContain('sorts each message into a lane')
    expect(fixtures.claude.requests).toHaveLength(0)
    expect((await agentCalls(home)).at(-1)?.model).toBe('sonnet')
    await expect.poll(() => page.locator('.msg-jev .who').last().innerText()).toContain('Claude Code · resolved-sonnet · local CLI response')
    await page.getByRole('combobox', { name: 'Harness model' }).selectOption('codex:codex-second')
    await composer.fill('Keep the urgency lanes')
    await composer.press('Enter')
    await expect.poll(() => page.locator('.msg-jev .who').last().innerText()).toContain('Codex · codex-second · local CLI response')
    await page.reload()
    await expect.poll(() => page.getByRole('combobox', { name: 'Harness model' }).inputValue()).toBe('codex:codex-second')
    expect(await page.getByRole('combobox', { name: 'Harness model' }).inputValue()).toBe('codex:codex-second')
    await expect.poll(() => page.locator('.rail .idea-item.active').innerText()).toBe('Sort my inbox by urgency')

    await page.locator('.panel textarea.inputs').fill('{ "message": "Quick one on our March invoice" }')
    await block.getByRole('button', { name: /Run/ }).click()
    await expect.poll(() => block.locator('.result-card .head').innerText()).toMatch(/^done/)
    const answered = page.locator('.panel section', { hasText: 'Jev answered' })
    await expect.poll(() => answered.innerText()).toContain('week')

    await composer.fill('/judge triage {"message": "Invoice question"}')
    await composer.press('Enter')
    await expect.poll(() => page.locator('.msg-jev .text').last().innerText()).toContain('"label": "week"')
    await composer.fill('/check')
    await composer.press('Enter')
    await expect.poll(() => page.locator('.msg-jev .text').last().innerText()).toBe('jevscript check: 0 errors, 0 warnings')
    await shot('chat')
  })

  it('Chat and the pause stack: a pasted program asks the person, and the card resumes it', async () => {
    await page.getByRole('button', { name: /New idea/ }).click()
    const composer = page.getByPlaceholder('Describe an idea, or ask the agent to change the program…')
    const drafts = fixtures.claude.requests.length
    await composer.fill(ASK)
    await composer.press('Enter')
    const block = page.locator('.codeblock').last()
    await expect.poll(() => block.locator('.bar-top').innerText()).toContain('0 errors')
    expect(fixtures.claude.requests).toHaveLength(drafts)
    await block.getByRole('button', { name: /Run/ }).click()
    const stack = page.locator('.stack')
    await expect.poll(() => stack.innerText()).toMatch(/1 pause waiting/i)
    await shot('pause-stack')
    await stack.getByRole('button', { name: 'yes' }).click()
    await expect.poll(() => page.locator('.panel section', { hasText: 'Told you' }).innerText()).toContain('answered yes')
    await expect.poll(() => stack.count()).toBe(0)
  })

  it('Chat: saves CLI errors and unavailable states without substituting an API fixture response', async () => {
    await page.getByRole('button', { name: /New idea/ }).click()
    const composer = page.getByPlaceholder('Describe an idea, or ask the agent to change the program…')
    await page.getByRole('combobox', { name: 'Harness model' }).selectOption('codex:codex-first')
    await writeFile(join(home, 'fake-agents/failure'), '')
    const apiCalls = fixtures.claude.requests.length
    await composer.fill('A failed CLI idea')
    await composer.press('Enter')
    await expect.poll(() => page.locator('.msg-jev .who').last().innerText()).toContain('Codex · codex-first · error')
    expect(await page.locator('.chat').innerText()).not.toContain('PRIVATE_PROVIDER_SECRET')
    expect(await page.locator('.codeblock').count()).toBe(0)
    expect(fixtures.claude.requests).toHaveLength(apiCalls)
    await page.reload()
    await page.locator('.rail .idea-item', { hasText: 'A failed CLI idea' }).click()
    await expect.poll(() => page.locator('.msg-jev .who').last().innerText()).toContain('error')
    await rm(join(home, 'fake-agents/failure'))
    await writeFile(join(home, 'fake-agents/signed-out'), '')
    await page.getByRole('button', { name: 'Refresh agents' }).click()
    await expect.poll(() => page.locator('.agent-state').innerText()).toContain('Codex unavailable')
    expect(await page.getByRole('combobox', { name: 'Harness model' }).locator('option').allTextContents()).toEqual(['Codex · codex-first · unavailable', 'Claude Code · unavailable'])
    await composer.fill('Still here after sign-out')
    await composer.press('Enter')
    await expect.poll(() => page.locator('.msg-jev .text').last().innerText()).toContain('unavailable')
    await rm(join(home, 'fake-agents/signed-out'))
    await page.getByRole('button', { name: 'Refresh agents' }).click()
    await expect.poll(() => page.getByRole('combobox', { name: 'Harness model' }).locator('option').count()).toBeGreaterThan(1)
  })

  it('Workspace: edits and annotates Jev/host files, checks and explicitly runs the pair, and restores it', async () => {
    await page.getByRole('button', { name: /New idea/ }).click()
    const composer = page.getByPlaceholder('Describe an idea, or ask the agent to change the program…')
    await page.getByRole('combobox', { name: 'Harness model' }).selectOption('codex:codex-second')
    await composer.fill(`program workspace_pair
in name: text
out greeting
task main:
  greeting = "Hello, {name}!"
`)
    await composer.press('Enter')
    const files = page.locator('.idea-files')
    await files.locator('summary').click()
    await page.getByRole('textbox', { name: 'New file name' }).fill('lib/message.jev')
    await files.getByRole('button', { name: 'Add file' }).click()
    const editor = files.locator('.cm-content')
    await editor.click()
    await page.keyboard.insertText('program helper\ndef greet(name):\n  return "Hello, {name}!"\n')
    await files.getByRole('tab', { name: /workspace_pair.jev/ }).click()
    await editor.click()
    await editor.press('ControlOrMeta+A')
    await page.keyboard.insertText(`program workspace_pair
use "./lib/message.jev" as helper
in name: text
out greeting
task main:
  greeting = helper.greet(name)
`)
    await editor.press('ControlOrMeta+A')
    await page.getByRole('textbox', { name: 'File annotation question' }).fill('Explain this Jev source')
    await files.getByRole('button', { name: 'Annotate selection' }).click()
    await expect.poll(() => files.locator('.file-note').count()).toBe(1)
    await files.getByRole('tab', { name: 'host.ts · Host' }).click()
    const host = page.getByRole('textbox', { name: 'Host file source' })
    await host.fill(`import { runIdea } from './.toolbox/host.ts'
const result = await runIdea({ inputs: { name: 'Browser' } })
if (result.outputs.greeting !== 'Hello, Browser!') throw new Error('unexpected output')
`)
    await host.press('ControlOrMeta+A')
    await page.getByRole('textbox', { name: 'File annotation question' }).fill('Explain this host source')
    await files.getByRole('button', { name: 'Annotate selection' }).click()
    await expect.poll(() => files.locator('.file-note').count()).toBe(1)
    await files.getByRole('button', { name: 'Check pair' }).click()
    await expect.poll(() => files.locator('.pair-check').innerText()).toContain('Jev compile passed. Node syntax check passed')
    expect(await page.locator('.result-card').count()).toBe(0)
    await files.getByRole('button', { name: 'Run pair' }).click()
    await expect.poll(() => files.innerText()).toContain('Host finished')
    await expect.poll(() => page.locator('.result-card').innerText()).toContain('Hello, Browser!')
    await host.fill('throw new Error("private host stderr")')
    await files.getByRole('button', { name: 'Run pair' }).click()
    await expect.poll(() => page.locator('.toast').innerText()).toContain('host exited unsuccessfully')
    await page.reload()
    await expect.poll(() => page.locator('.rail .idea-item.active').innerText()).toBe('program workspace_pair')
    await page.locator('.idea-files summary').click()
    await expect.poll(() => page.getByRole('textbox', { name: 'Host file source' }).inputValue()).toContain('private host stderr')
    expect(await page.locator('.idea-files .file-note').count()).toBe(1)
    await page.getByRole('tab', { name: /workspace_pair.jev/ }).click()
    expect(await page.locator('.idea-files .file-note').count()).toBe(1)
    await shot('workspace-pair')
  })

  it('the pause stack: cards stack one per run, oldest first, and each resumes its run', async () => {
    await page.getByRole('button', { name: /New idea/ }).click()
    const composer = page.getByPlaceholder('Describe an idea, or ask the agent to change the program…')
    await composer.fill(TUNED)
    await composer.press('Enter')
    await expect.poll(() => page.locator('.codeblock').last().locator('.bar-top').innerText()).toContain('0 errors')

    await screen('Adapters')
    await page.locator('table.caps tbody tr').first().click()
    await page.locator('.bind-card', { hasText: 'Subprocess' }).click()
    await page.getByPlaceholder('node adapters/tree.js').fill(`sh ${join(home, 'flaky.sh')}`)

    await screen('Playground')
    const tune = page.locator('.panel section', { hasText: 'Tune · task main' })
    await expect.poll(() => tune.locator('.dial input[type=number]').count()).toBe(3)
    const steps = tune.locator('.dial', { hasText: 'budget steps' }).locator('input[type=number]')
    await expect.poll(() => steps.inputValue()).toBe('2000')
    await steps.fill('3000')
    await steps.press('Enter')
    await expect.poll(editorText).toMatch(/task main budget calls 1, steps 3(000|k) thresholds min_confidence 0\.9: {2}# tuned/)

    await page.locator('.header .actions').getByRole('button', { name: /Run/ }).click()
    const stack = page.locator('.stack')
    await expect.poll(() => stack.innerText()).toContain('warming up')
    await idea('program ask_me')
    await page.locator('.header .actions').getByRole('button', { name: /Run/ }).click()
    await expect.poll(() => stack.innerText()).toMatch(/2 pauses waiting/i)
    await expect.poll(() => stack.locator('.card').innerText()).toContain('warming up')
    await shot('pause-stack-two')

    // An answered card leaves; the run's next pause queues behind the other run's.
    await stack.getByRole('button', { name: 'Retry' }).click()
    await expect.poll(() => stack.locator('.card').innerText()).toContain('Go ahead?')
    await stack.getByRole('button', { name: 'no' }).click()
    await expect.poll(() => stack.locator('.card').innerText()).toContain('Raise calls to 2')
    await stack.getByRole('button', { name: 'Raise calls to 2' }).click()
    await expect.poll(() => stack.locator('.card').getByRole('button').allInnerTexts()).toEqual(['today', 'week', 'ignore'])
    await stack.getByRole('button', { name: 'week' }).click()
    await expect.poll(() => stack.count()).toBe(0)
    await screen('Chat')
    await expect.poll(() => page.locator('.panel section', { hasText: 'Told you' }).innerText()).toContain('answered no')
    await idea('program tuned')
    await expect.poll(() => page.locator('.panel section', { hasText: 'Told you' }).innerText()).toContain('lane week')
  })

  it('Playground: language server colours and diagnostics, tuning, run, requests and replay', async () => {
    await idea('Review loop for Claude')
    await screen('Playground')
    await expect.poll(() => page.locator('.status-line').innerText()).toContain('jevscript lsp · diagnostics, hover, completion')
    await expect.poll(() => page.locator('.editor-area .tok-keyword').count()).toBeGreaterThan(5)
    await expect.poll(() => page.locator('.console .tabs').innerText()).toContain('Diagnostics 1')

    const dial = page.locator('.dial', { hasText: 'min_confidence' })
    await dial.locator('input[type=number]').fill('0.7')
    await dial.locator('input[type=number]').press('Enter')
    await expect.poll(editorText).toContain('min_confidence 0.7')
    await page.getByRole('button', { name: 'sample' }).click()
    await page.getByRole('button', { name: 'sample' }).click()

    await page.locator('.header .actions').getByRole('button', { name: /Run/ }).click()
    await expect.poll(() => page.locator('.console-body').innerText()).toContain('done')
    await page.locator('.console .tab', { hasText: 'Requests' }).click()
    const list = page.locator('.requests .list')
    await expect.poll(() => list.innerText()).toContain('working')
    await expect.poll(() => list.getByRole('button').count()).toBe(2)
    await shot('playground-requests')

    const calls = fixtures.jev.bodies.length
    await page.getByRole('button', { name: 'Replay last' }).click()
    await expect.poll(() => page.locator('.console .status').innerText()).toContain('replay · no live calls')
    expect(fixtures.jev.bodies).toHaveLength(calls)
  })

  it('Machines: graph from the IR, steps from the recording, and a checked edit applied and reverted at a pin', async () => {
    await screen('Machines')
    const graph = page.locator('svg.graph')
    await expect.poll(() => graph.locator('g.node').count()).toBe(5)
    await expect.poll(() => graph.locator('g.edge.guarded').count()).not.toBe(0)
    await expect.poll(() => graph.locator('g.edge.risky').count()).toBe(1)
    await expect.poll(() => graph.locator('g.node.current').count()).toBe(1)
    await expect.poll(() => page.locator('.timeline .step').count()).toBe(2)
    await expect.poll(() => page.locator('.panel').first().innerText()).toMatch(/proceed/)

    await graph.locator('g.edge', { hasText: 'stuck' }).locator('text').first().click()
    const popover = page.locator('.popover')
    await popover.getByPlaceholder('Ask about this, or ask for a change…').fill('When it is stuck, ask me instead of nudging.')
    await popover.getByRole('button', { name: 'Ask' }).click()
    await expect.poll(() => popover.innerText()).toContain('waiting_on_me is now reached by asks and stuck.')
    await expect.poll(() => popover.locator('.diff .del').count()).toBe(2)
    await shot('machines-pin')
    await popover.getByRole('button', { name: 'Apply to source' }).click()
    await screen('Playground')
    await expect.poll(editorText).not.toContain('Stop. In three lines')
    await screen('Machines')
    await page.locator('g.edge', { hasText: 'stuck' }).locator('text').first().click()
    await expect.poll(() => page.locator('.popover').innerText()).toContain('Applied to source.')
    await page.locator('.popover').getByRole('button', { name: 'Revert' }).click()
    await screen('Playground')
    await expect.poll(editorText).toContain('Stop. In three lines')
  })

  it('Adapters: bindings per capability and the manifest check (spec 9.4)', async () => {
    await screen('Adapters')
    const rows = page.locator('table.caps tbody tr')
    await expect.poll(() => rows.count()).toBe(3)
    await rows.filter({ hasText: 'tree' }).click()
    await page.locator('.bind-card', { hasText: 'Subprocess' }).click()
    await page.getByPlaceholder('node adapters/tree.js').fill('sh adapters/tree.sh')
    await page.locator('label.field', { hasText: 'manifest' }).locator('textarea').fill('{ "verbs": { "test_summary": { "returns": "text" } } }')
    await page.getByRole('button', { name: 'Check manifests' }).click()
    await expect.poll(() => rows.filter({ hasText: 'tree' }).innerText()).toContain('verb_missing: tree.tests_pass')
    await shot('adapters')
  })

  it('keeps ideas, pins and the run index in SQLite across a server restart', async () => {
    // The page saves an idea 400 ms after its last edit; restart only once the server holds those edits.
    const observer = await connect({ url })
    const review = async () => (await observer.request<{ ideas: Idea[] }>('ideas.list')).ideas.find((candidate) => candidate.title === 'Review loop for Claude')
    await expect.poll(async () => (await review())?.pins.length, { timeout: 5_000 }).toBe(1)
    await expect.poll(async () => (await review())?.bindings['tree'], { timeout: 5_000 }).toMatchObject({ kind: 'subprocess' })
    observer.close()
    const exited = new Promise((resolve) => server.once('exit', resolve))
    server.kill('SIGTERM')
    await exited
    await startServer()
    await page.reload()
    await expect.poll(() => page.locator('.rail .idea-item').allInnerTexts()).toEqual(
      expect.arrayContaining(['program tuned', 'program ask_me', 'Sort my inbox by urgency', 'Review loop for Claude']),
    )
    await idea('Review loop for Claude')
    await screen('Machines')
    await expect.poll(() => page.locator('svg.graph g.pin').count()).toBe(1)
    await expect.poll(() => page.locator('.timeline').innerText()).toContain('2 steps')
    await screen('Adapters')
    await expect.poll(() => page.locator('table.caps tbody tr', { hasText: 'tree' }).innerText()).toContain('sh adapters/tree.sh')
    await shot('after-restart')
  })
})
