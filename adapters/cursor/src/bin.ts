#!/usr/bin/env node
/** Cursor JSONL subprocess adapter (spec sections 9.1 and 11.6). */
import { cursor, runAdapterCli } from '@jevscript/adapter-core'
import { CursorAdapter } from './index.ts'

process.exitCode = await runAdapterCli(cursor, process.argv.slice(2), (options) => new CursorAdapter(options))
