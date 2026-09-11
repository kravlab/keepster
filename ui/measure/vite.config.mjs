import { defineConfig } from 'vite'
import { writeFileSync } from 'node:fs'
import { join } from 'node:path'

/* PROTOTYPE (ticket 09): one build per engine, into its own outDir, so the size table is not
   confused by chunks shared between the two. A tiny plugin records the static/dynamic import graph,
   so "what must be downloaded before anything renders" is read off the graph rather than guessed
   from file names. */
const target = process.env.MEASURE_TARGET
if (!target) throw new Error('MEASURE_TARGET must be cm6, monaco or monaco-core')

const outDir = `measure/out/${target}`

/** Names a chunk after the npm package (or language module) it came from, so the table is readable. */
function nameChunk(chunk) {
  const id = chunk.facadeModuleId ?? chunk.moduleIds?.[chunk.moduleIds.length - 1] ?? ''
  const pkg = id.match(/node_modules\/(?:\.pnpm\/)?(?:@[^/]+\+)?(@[^/]+\/[^/]+|[^/@]+)/)
  const tail = id.split('/').filter(Boolean).slice(-2).join('-').replace(/\.[jt]s$/, '')
  const base = (pkg?.[1] ?? tail).replace(/[^a-zA-Z0-9._-]/g, '_')
  return `chunks/${base}-[hash].js`
}

export default defineConfig({
  logLevel: 'warn',
  build: {
    outDir,
    emptyOutDir: true,
    target: 'es2022',
    minify: 'oxc', // Vite 8 is rolldown-based; esbuild is no longer bundled
    reportCompressedSize: false,
    rollupOptions: {
      input: `measure/${target}.ts`,
      output: {
        entryFileNames: '[name].js',
        chunkFileNames: nameChunk,
        assetFileNames: 'assets/[name][extname]',
      },
      plugins: [
        {
          name: 'keepster-chunk-graph',
          generateBundle(_opts, bundle) {
            const meta = Object.values(bundle)
              .filter((c) => c.type === 'chunk')
              .map((c) => ({
                file: c.fileName,
                isEntry: c.isEntry,
                isDynamicEntry: c.isDynamicEntry,
                imports: c.imports,
                dynamicImports: c.dynamicImports,
                css: c.viteMetadata ? [...(c.viteMetadata.importedCss ?? [])] : [],
              }))
            writeFileSync(join(process.cwd(), outDir, 'meta.json'), JSON.stringify(meta, null, 2))
          },
        },
      ],
    },
  },
})
