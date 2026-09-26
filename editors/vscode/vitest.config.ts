import { defineConfig } from 'vitest/config'

export default defineConfig({
  test: {
    include: ['test/**/*.test.ts'],
    // The handshake test starts the real `jevscript lsp`.
    testTimeout: 20_000,
  },
})
