import { defineConfig } from 'vite'
import { svelte } from '@sveltejs/vite-plugin-svelte'

// PROTOTYPE (wayfinder ticket 09). Nothing here is meant to survive into v1.
export default defineConfig({
  plugins: [svelte()],
  server: { port: 5177 },
})
