#!/usr/bin/env node
/** Antigravity JSONL subprocess adapter (spec sections 9.1 and 11.6). */
import { agy, runAdapterCli } from '@jevscript/adapter-core'
import { AntigravityAdapter } from './index.ts'

process.exitCode = await runAdapterCli(agy, process.argv.slice(2), (options) => new AntigravityAdapter(options))
