// Bundle the extension and its client library into one CommonJS file, so the
// package needs no node_modules (vsce --no-dependencies). The esbuild API is
// used rather than its bin, whose install step swaps in a native binary that
// pnpm's Node shim cannot run.

import { build } from 'esbuild'

await build({
  entryPoints: ['src/extension.ts'],
  bundle: true,
  platform: 'node',
  format: 'cjs',
  target: 'node20',
  external: ['vscode'],
  outfile: 'dist/extension.js',
  minify: true,
  logLevel: 'info',
})
