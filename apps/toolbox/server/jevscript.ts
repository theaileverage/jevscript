/**
 * The `jevscript` CLI, for what the JSON-RPC surface does not carry:
 * `compile` (the IR and every warning, spec section 11.6), `check --tools`
 * (manifest comparison, section 9.4) and `replay` (section 10.4).
 */
import { spawn } from 'node:child_process'
import { mkdir, mkdtemp, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import type { CheckResult, Diagnostic, ReplayResult } from '../shared/protocol.ts'
import { parseDiagnostics } from '../shared/protocol.ts'
import { readIr } from '../shared/ir.ts'
import type { Pause } from '../shared/pauses.ts'
import { parseRecording } from '../shared/recording.ts'
import { readFile } from 'node:fs/promises'

export interface Exited {
  code: number | null
  stdout: string
  stderr: string
}

export function exec(bin: string, args: string[], env: NodeJS.ProcessEnv = process.env): Promise<Exited> {
  return new Promise((resolve, reject) => {
    const child = spawn(bin, args, { env, stdio: ['ignore', 'pipe', 'pipe'] })
    let stdout = ''
    let stderr = ''
    child.stdout.setEncoding('utf8').on('data', (chunk: string) => (stdout += chunk))
    child.stderr.setEncoding('utf8').on('data', (chunk: string) => (stderr += chunk))
    child.on('error', reject)
    child.on('close', (code) => resolve({ code, stdout, stderr }))
  })
}

/** A safe file name for a program, so diagnostics read `inbox_triage.jev:3:1`. */
export function programFileName(fileName: string): string {
  const base = fileName.replace(/[^A-Za-z0-9_.-]/g, '_')
  return base.endsWith('.jev') ? base : `${base || 'program'}.jev`
}

export class Jevscript {
  readonly bin: string
  readonly workDir: string

  constructor(bin: string, workDir: string) {
    this.bin = bin
    this.workDir = workDir
  }

  /** Write the source where the CLI can read it. Each call gets its own directory. */
  async materialize(fileName: string, source: string): Promise<string> {
    await mkdir(this.workDir, { recursive: true })
    const dir = await mkdtemp(join(this.workDir, 'src-'))
    const path = join(dir, programFileName(fileName))
    await writeFile(path, source)
    return path
  }

  /** `jevscript compile`: the IR when it compiles, and every diagnostic, warnings included. */
  async compile(fileName: string, source: string): Promise<CheckResult> {
    const path = await this.materialize(fileName, source)
    const result = await exec(this.bin, ['compile', path])
    const diagnostics = relabel(parseDiagnostics(result.stderr), path, programFileName(fileName))
    if (result.code === 0) return { diagnostics, ir: readIr(JSON.parse(result.stdout)) }
    if (diagnostics.length === 0) {
      throw new Error(`jevscript compile failed: ${result.stderr.trim() || `exit ${String(result.code)}`}`)
    }
    return { diagnostics, ir: null }
  }

  /** `jevscript check --tools`: each tool verb the program uses that a manifest lacks. */
  async checkTools(
    fileName: string,
    source: string,
    manifests: Record<string, unknown>,
  ): Promise<{ missing: string[]; diagnostics: Diagnostic[] }> {
    const path = await this.materialize(fileName, source)
    const manifestPath = join(await mkdtemp(join(tmpdir(), 'jevs-tools-')), 'manifests.json')
    await writeFile(manifestPath, JSON.stringify(manifests))
    const result = await exec(this.bin, ['check', path, '--tools', manifestPath])
    const diagnostics = relabel(parseDiagnostics(result.stderr), path, programFileName(fileName))
    return {
      missing: diagnostics.filter((diagnostic) => diagnostic.code === 'verb_missing').map((diagnostic) => diagnostic.message),
      diagnostics: diagnostics.filter((diagnostic) => diagnostic.code !== 'verb_missing'),
    }
  }

  /**
   * `jevscript replay`: the recording and nothing else. The child gets no
   * TypeSafe key and no profiles overlay, so any attempt to call Jev would
   * fail instead of spending; the CLI binds no adapters at all.
   */
  async replay(recording: string, env: NodeJS.ProcessEnv = process.env): Promise<ReplayResult> {
    const sealed: NodeJS.ProcessEnv = { ...env }
    delete sealed['TYPESAFE_API_KEY']
    delete sealed['JEVSCRIPT_PROFILES']
    const result = await exec(this.bin, ['replay', recording], sealed)
    const pauses = result.stdout
      .split('\n')
      .filter((line) => line.startsWith('{'))
      .map((line) => JSON.parse(line) as Pause)
    const events = parseRecording(await readFile(recording, 'utf8'))
    return { pauses, events, exitCode: result.code, stderr: result.stderr }
  }
}

function relabel(diagnostics: Diagnostic[], path: string, fileName: string): Diagnostic[] {
  return diagnostics.map((diagnostic) => (diagnostic.file === path ? { ...diagnostic, file: fileName } : diagnostic))
}
