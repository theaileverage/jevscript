/**
 * Resending an edited Jev request as a separate live call. It reads the
 * recording and never writes it, and it runs no task, so the run, its
 * recording and its replay are untouched. History is kept per request under
 * the idea's folder.
 *
 * Every resend posts the shown state and questions to the recorded profile's
 * endpoint. `judgment.run` would rebuild the questions from the idea's current
 * source and the current profiles, and a matching shape hash does not prove
 * those equal the recorded ones: the hash covers the answer shape (spec section
 * 11.4), not descriptions, conditions or detail blocks.
 */
import { createHash } from 'node:crypto'
import { mkdir, readFile, writeFile } from 'node:fs/promises'
import { basename, join } from 'node:path'

import { type Profile, type JevAnswer, parseRecording, startInfo } from '../shared/recording.ts'
import { editedPaths, requestEntries, type ResendRecord } from '../shared/requests.ts'
import { errorMessage, type JevRequest, parseResponse, requestBody } from '../shared/wire.ts'

export interface ResendInput {
  ideaId: string
  recording: string
  requestId: string
  request: JevRequest
}

export interface ResendDeps {
  home: string
  env: NodeJS.ProcessEnv
  fetch?: typeof fetch
}

export async function resend(input: ResendInput, deps: ResendDeps): Promise<ResendRecord> {
  const events = parseRecording(await readFile(input.recording, 'utf8'))
  const start = startInfo(events)
  const entry = requestEntries(events, start?.ir ?? null).find((candidate) => candidate.requestId === input.requestId)
  if (!entry) throw new Error(`the recording has no request \`${input.requestId}\``)
  const profile = start?.profile
  if (!profile?.endpoint) throw new Error('the recording has no resolved profile to resend against')
  const edited = editedPaths(entry.request, input.request)

  const began = Date.now()
  let answers: JevAnswer[] | null = null
  let error: string | null = null
  try {
    answers = await viaEndpoint(input.request, profile, deps)
  } catch (failure) {
    error = failure instanceof Error ? failure.message : String(failure)
  }
  const record: ResendRecord = {
    at: new Date().toISOString(),
    recording: input.recording,
    requestId: input.requestId,
    via: 'endpoint',
    edited,
    request: input.request,
    answers,
    error,
    latencyMs: Date.now() - began,
  }
  await appendHistory(deps.home, input.ideaId, input.recording, input.requestId, record)
  return record
}

/** Post the ported wire body to the recorded profile's endpoint, as `HttpJevClient::send` does. */
async function viaEndpoint(request: JevRequest, profile: Profile, deps: ResendDeps): Promise<JevAnswer[]> {
  const key = deps.env['TYPESAFE_API_KEY']
  if (!key) throw new Error('TYPESAFE_API_KEY is not set')
  const response = await (deps.fetch ?? fetch)(profile.endpoint as string, {
    method: 'POST',
    headers: { authorization: `Bearer ${key}`, 'content-type': 'application/json' },
    body: JSON.stringify(requestBody(request, profile.model)),
  })
  const text = await response.text()
  if (response.status >= 400) throw new Error(errorMessage(response.status, text))
  let json: unknown
  try {
    json = JSON.parse(text)
  } catch (error) {
    throw new Error(`HTTP ${response.status}: the response is not JSON: ${String(error)}`)
  }
  return parseResponse(json, request)
}

function historyPath(home: string, ideaId: string, recording: string, requestId: string): string {
  if (!/^[A-Za-z0-9_-]+$/.test(ideaId) || !/^[A-Za-z0-9_-]+$/.test(requestId)) throw new Error('invalid id')
  const run = `${basename(recording, '.jsonl')}-${createHash('sha256').update(recording).digest('hex').slice(0, 8)}`
  return join(home, 'ideas', ideaId, 'resends', run, `${requestId}.json`)
}

export async function resendHistory(home: string, ideaId: string, recording: string, requestId: string): Promise<ResendRecord[]> {
  try {
    return JSON.parse(await readFile(historyPath(home, ideaId, recording, requestId), 'utf8')) as ResendRecord[]
  } catch {
    return []
  }
}

async function appendHistory(home: string, ideaId: string, recording: string, requestId: string, record: ResendRecord): Promise<void> {
  const path = historyPath(home, ideaId, recording, requestId)
  const history = await resendHistory(home, ideaId, recording, requestId)
  await mkdir(join(path, '..'), { recursive: true })
  await writeFile(path, JSON.stringify([...history, record], null, 2))
}
