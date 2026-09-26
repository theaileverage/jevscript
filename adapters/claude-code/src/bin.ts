#!/usr/bin/env node
/**
 * `jevscript-adapter-claude-code`: the JSONL subprocess adapter (spec section
 * 11.6) for Claude Code. Defaults to the Herdr backend, like every executable
 * in the suite; `--backend tmux` gives the library's default.
 */
import { claude, defaultBackendName, runAdapterCli } from '@jevscript/adapter-core'

import { ClaudeCodeAdapter } from './index.ts'

process.exitCode = await runAdapterCli(
  claude,
  process.argv.slice(2),
  (options) => new ClaudeCodeAdapter({ backend: defaultBackendName(), ...options }),
  'jevscript-adapter-claude-code',
)
