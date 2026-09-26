import { defineConfig } from 'vitest/config'

export default defineConfig({
  test: {
    include: ['test/**/*.test.ts'],
    // The tmux integration and JSONL executable tests drive real processes.
    testTimeout: 30_000,
  },
})
