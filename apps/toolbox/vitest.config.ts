import { defineConfig } from 'vitest/config'

export default defineConfig({
  test: {
    include: ['test/**/*.test.ts'],
    exclude: ['test/browser/**'],
    environment: 'node',
    env: { JEVS_TOOLBOX_CLAUDE_BIN: '/nonexistent/claude-fixture', JEVS_TOOLBOX_CODEX_BIN: '/nonexistent/codex-fixture' },
    testTimeout: 60_000,
    hookTimeout: 60_000,
  },
})
