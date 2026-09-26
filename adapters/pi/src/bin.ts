#!/usr/bin/env node
/** Pi JSONL subprocess adapter (spec sections 9.1 and 11.6). */
import { pi, runAdapterCli } from '@jevscript/adapter-core'
import { PiAdapter } from './index.ts'

process.exitCode = await runAdapterCli(pi, process.argv.slice(2), (options) => new PiAdapter(options))
