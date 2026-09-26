#!/usr/bin/env node
/** Codex JSONL subprocess adapter (spec sections 9.1 and 11.6). */
import { codex, runAdapterCli } from '@jevscript/adapter-core'
import { CodexAdapter } from './index.ts'

process.exitCode = await runAdapterCli(codex, process.argv.slice(2), (options) => new CodexAdapter(options))
