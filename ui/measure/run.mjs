/*
 * Holds the built SPA to its weight budget (ticket 28), reading the Vite manifest of the real build in
 * `dist/`; run it after `pnpm build`. Exits non-zero when a limit is crossed.
 *
 * Eager is the entry's static import closure plus its CSS: what the browser must download before the page
 * shows anything. Total is every script and stylesheet, the lazy chunks (the editor, each grammar)
 * included. A dynamically imported chunk that lands in the eager closure fails the check too: that is how
 * a grammar stops being lazy without anyone noticing.
 */
import { readdirSync, readFileSync } from 'node:fs'
import { join } from 'node:path'
import { gzipSync } from 'node:zlib'

const DIST = 'dist'
const EAGER_LIMIT_KIB = 150
const TOTAL_LIMIT_KIB = 400

const manifest = JSON.parse(readFileSync(join(DIST, '.vite/manifest.json'), 'utf8'))

/** The manifest's chunks closed over their static imports, starting from `keys`, as output file names. */
function closure(keys) {
  const files = new Set()
  const seen = new Set()
  const queue = [...keys]
  while (queue.length > 0) {
    const key = queue.pop()
    if (seen.has(key)) continue
    seen.add(key)
    const chunk = manifest[key]
    files.add(chunk.file)
    for (const css of chunk.css ?? []) files.add(css)
    queue.push(...(chunk.imports ?? []))
  }
  return files
}

/** A built file's size gzipped at the highest level, as a server would send it at best. */
const gzipped = (file) => gzipSync(readFileSync(join(DIST, file)), { level: 9 }).length
/** Bytes in KiB. */
const kib = (bytes) => bytes / 1024

const eager = closure(Object.keys(manifest).filter((key) => manifest[key].isEntry))
const all = readdirSync(join(DIST, 'assets'))
  .filter((name) => name.endsWith('.js') || name.endsWith('.css'))
  .map((name) => `assets/${name}`)

const eagerKib = kib([...eager].reduce((sum, file) => sum + gzipped(file), 0))
const totalKib = kib(all.reduce((sum, file) => sum + gzipped(file), 0))
const notLazy = Object.entries(manifest)
  .filter(([, chunk]) => chunk.isDynamicEntry && eager.has(chunk.file))
  .map(([key]) => key)

const failures = []
if (eagerKib > EAGER_LIMIT_KIB) failures.push(`eager ${eagerKib.toFixed(1)} KiB is over ${EAGER_LIMIT_KIB} KiB`)
if (totalKib > TOTAL_LIMIT_KIB) failures.push(`total ${totalKib.toFixed(1)} KiB is over ${TOTAL_LIMIT_KIB} KiB`)
for (const key of notLazy) failures.push(`${key} is imported lazily but loaded eagerly`)

process.stdout.write(
  `eager ${eagerKib.toFixed(1)} KiB of ${EAGER_LIMIT_KIB} KiB gzip (${eager.size} files)\n` +
    `total ${totalKib.toFixed(1)} KiB of ${TOTAL_LIMIT_KIB} KiB gzip (${all.length} files)\n`,
)
for (const failure of failures) process.stderr.write(`budget: ${failure}\n`)
process.exit(failures.length === 0 ? 0 : 1)
