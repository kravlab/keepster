/**
 * PROTOTYPE fixtures (ticket 09): the pathological cases, generated deterministically so that
 * every run of the bake-off compares the same bytes (fixed seed, no Math.random).
 */

/** mulberry32 — small, deterministic, good enough for fixture text. */
function rng(seed: number): () => number {
  let a = seed >>> 0
  return () => {
    a = (a + 0x6d2b79f5) >>> 0
    let t = Math.imul(a ^ (a >>> 15), 1 | a)
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296
  }
}

const LEVELS = ['INFO', 'DEBUG', 'WARN', 'ERROR']
const EVENTS = [
  'watcher: event batch',
  'capture: snapshot written',
  'reconcile: walk complete',
  'retention: segment thinned',
  'maintenance: gc finished',
  'api: GET /v1/roots',
]

/** ~60k lines, ~4 MB: the "large file" case the ticket asks about. */
export function makeLargeLog(): { before: string; after: string } {
  const r = rng(20260910)
  const lines: string[] = []
  for (let i = 0; i < 60_000; i++) {
    const lvl = LEVELS[Math.floor(r() * LEVELS.length)]
    const ev = EVENTS[Math.floor(r() * EVENTS.length)]
    const ms = (r() * 900).toFixed(1)
    lines.push(
      `2026-09-10T08:${String(Math.floor(i / 1000) % 60).padStart(2, '0')}:${String(i % 60).padStart(2, '0')}.${String(i % 1000).padStart(3, '0')}Z ${lvl.padEnd(5)} keepster[${1000 + (i % 7)}] ${ev} root=01K5Z7Q9 files=${i % 512} took=${ms}ms`,
    )
  }
  const before = lines.join('\n')

  // A realistic edit shape for a log: a block appended at the end, plus scattered rewrites.
  const after = lines.slice()
  const r2 = rng(7)
  for (let i = 0; i < 200; i++) {
    const at = Math.floor(r2() * after.length)
    after[at] = after[at].replace('INFO ', 'WARN ').replace('took=', 'took(slow)=')
  }
  after.splice(30_000, 0, ...Array.from({ length: 40 }, (_, i) => `2026-09-10T09:00:${String(i).padStart(2, '0')}.000Z ERROR keepster[1007] capture: store lock contended attempt=${i + 1}`))
  return { before, after: after.join('\n') }
}

/** ~875k characters (~0.85 MB) on a single line: the long-line case every viewport-based editor dreads. */
export function makeMinified(): { before: string; after: string } {
  const r = rng(4242)
  const parts: string[] = ['(function(){"use strict";var _=[];']
  for (let i = 0; i < 14_000; i++) {
    const n = Math.floor(r() * 1e6).toString(36)
    parts.push(`function f${i}(a,b){return a?_[${i}]=b+"${n}":_[${i}]||"${n}"}`)
  }
  parts.push('return _})();')
  const before = parts.join('')
  const after = before.replace('var _=[];', 'var _=[],__k=1;').replace('return _})();', 'return[_,__k]})();')
  return { before, after }
}

/** Bytes with NULs: not a text diff at all. Kept to show what the UI does *instead* of an editor. */
export function makeBinary(): { before: string; after: string } {
  const r = rng(1)
  const bytes = (n: number) => Array.from({ length: n }, () => String.fromCharCode(Math.floor(r() * 256))).join('')
  const header = '\x89PNG\r\n\x1a\n'
  return { before: header + bytes(2048), after: header + bytes(2304) }
}
