#!/usr/bin/env node
/** Muse JSONL subprocess adapter (spec sections 9.1 and 11.6). */
import { muse, runAdapterCli } from '@jevscript/adapter-core'
import { MuseAdapter } from './index.ts'

process.exitCode = await runAdapterCli(muse, process.argv.slice(2), (options) => new MuseAdapter(options))
