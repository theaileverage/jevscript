#!/usr/bin/env node
/** OpenCode JSONL subprocess adapter (spec sections 9.1 and 11.6). */
import { opencode, runAdapterCli } from '@jevscript/adapter-core'
import { OpenCodeAdapter } from './index.ts'

process.exitCode = await runAdapterCli(opencode, process.argv.slice(2), (options) => new OpenCodeAdapter(options))
