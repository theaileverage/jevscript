#!/usr/bin/env node
import { spawn } from 'node:child_process'
import { packagedBinary } from '../dist/native.js'

let binary
try {
  binary = packagedBinary()
} catch (error) {
  console.error(`jevscript: ${error instanceof Error ? error.message : String(error)}`)
  process.exit(1)
}

const child = spawn(binary, process.argv.slice(2), { stdio: 'inherit' })
for (const signal of ['SIGINT', 'SIGTERM']) {
  process.on(signal, () => child.kill(signal))
}
child.on('error', (error) => {
  console.error(`jevscript: ${error.message}`)
  process.exitCode = 1
})
child.on('exit', (code, signal) => {
  process.exitCode = code ?? (signal ? 128 + (signal === 'SIGINT' ? 2 : 15) : 1)
})
