#!/usr/bin/env node
/** Gemini JSONL subprocess adapter (spec sections 9.1 and 11.6). */
import { gemini, runAdapterCli } from '@jevscript/adapter-core'
import { GeminiAdapter } from './index.ts'

process.exitCode = await runAdapterCli(gemini, process.argv.slice(2), (options) => new GeminiAdapter(options))
