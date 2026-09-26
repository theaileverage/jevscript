import { defineConfig } from 'vitest/config'

export default defineConfig({
  test: {
    include: ['test/**/*.test.ts'],
    // Spawning the runtime binary is slower than an in-process test.
    testTimeout: 20_000,
  },
})
