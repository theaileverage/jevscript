#!/usr/bin/env node
/** Omp JSONL subprocess adapter (spec sections 9.1 and 11.6). */
import { omp, runAdapterCli } from '@jevscript/adapter-core'
import { OmpAdapter } from './index.ts'

process.exitCode = await runAdapterCli(omp, process.argv.slice(2), (options) => new OmpAdapter(options))
