#!/usr/bin/env node
/** `jevscript-agent <cli> [options]` and `jevscript-agent discover`; see `cli.ts`. */
import { runAgentCli } from './cli.ts'

process.exitCode = await runAgentCli()
