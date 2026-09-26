#!/usr/bin/env node
/** Kimi JSONL subprocess adapter (spec sections 9.1 and 11.6). */
import { kimi, runAdapterCli } from '@jevscript/adapter-core'
import { KimiAdapter } from './index.ts'

process.exitCode = await runAdapterCli(kimi, process.argv.slice(2), (options) => new KimiAdapter(options))
