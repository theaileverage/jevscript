/**
 * Test configuration every package in the suite shares: `@jevscript/adapter-core`
 * resolves to its source, so tests and typechecks never need a build.
 */
import { fileURLToPath } from 'node:url'

import type { ViteUserConfig } from 'vitest/config'

const core = (path: string) => fileURLToPath(new URL(`./src/${path}`, import.meta.url))

export const shared: ViteUserConfig = {
  resolve: {
    alias: [
      { find: /^@jevscript\/adapter-core\/testing$/, replacement: core('testing.ts') },
      { find: /^@jevscript\/adapter-core$/, replacement: core('index.ts') },
    ],
  },
  test: {
    include: ['test/**/*.test.ts'],
    // Integration tests drive a real tmux server and real subprocesses.
    testTimeout: 30_000,
  },
}
