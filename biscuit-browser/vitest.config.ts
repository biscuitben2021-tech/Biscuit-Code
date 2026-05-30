import { resolve } from 'path'
import { defineConfig } from 'vitest/config'

// Unit tests run the project's PURE logic (gate decisions, contract parsing, URL
// normalization, verification, the page-world extractor against jsdom) in plain
// Node — no Electron required. The `@shared` alias mirrors the app's tsconfig.
export default defineConfig({
  resolve: {
    alias: {
      '@shared': resolve('src/shared')
    }
  },
  test: {
    include: ['test/**/*.test.ts'],
    environment: 'node'
  }
})
