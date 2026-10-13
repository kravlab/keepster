import { svelte } from '@sveltejs/vite-plugin-svelte'
import { defineConfig } from 'vitest/config'

// The build is what the daemon embeds (`keepster-ui`), committed under `dist/` so that a plain checkout builds
// without node. ES2022 is the target the size budget was measured against: the two move together (ticket 28).
export default defineConfig({
  plugins: [svelte()],
  build: {
    target: 'es2022',
    // `measure/run.mjs` reads the import graph off the manifest; the daemon does not embed it.
    manifest: true,
    reportCompressedSize: false,
    // Every browser at the Baseline floor supports `modulepreload`, so the polyfill is dead weight.
    modulePreload: { polyfill: false },
  },
  test: {
    include: ['src/**/*.test.ts'],
  },
})
