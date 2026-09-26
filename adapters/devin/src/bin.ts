#!/usr/bin/env node
/** Devin JSONL subprocess adapter (spec sections 9.1 and 11.6). */
import { devin, runAdapterCli } from '@jevscript/adapter-core'
import { DevinAdapter } from './index.ts'

process.exitCode = await runAdapterCli(devin, process.argv.slice(2), (options) => new DevinAdapter(options))
