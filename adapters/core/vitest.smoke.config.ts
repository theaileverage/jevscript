import { defineConfig } from 'vitest/config'

// Live smoke tests: they start real agent CLIs, so they never run in CI.
export default defineConfig({
  test: {
    include: ['smoke/**/*.smoke.ts'],
    testTimeout: 600_000,
    hookTimeout: 120_000,
    fileParallelism: false,
  },
})
