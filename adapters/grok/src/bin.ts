#!/usr/bin/env node
/** Grok JSONL subprocess adapter (spec sections 9.1 and 11.6). */
import { grok, runAdapterCli } from '@jevscript/adapter-core'
import { GrokAdapter } from './index.ts'

process.exitCode = await runAdapterCli(grok, process.argv.slice(2), (options) => new GrokAdapter(options))
