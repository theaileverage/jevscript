#!/usr/bin/env node
/** Rovo JSONL subprocess adapter (spec sections 9.1 and 11.6). */
import { rovo, runAdapterCli } from '@jevscript/adapter-core'
import { RovoAdapter } from './index.ts'

process.exitCode = await runAdapterCli(rovo, process.argv.slice(2), (options) => new RovoAdapter(options))
