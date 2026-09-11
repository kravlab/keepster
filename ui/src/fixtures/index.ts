/** PROTOTYPE (wayfinder ticket 09) — the fixture set the two engines are judged on. */
import { RUST_AFTER, RUST_BEFORE } from './rust'
import { MD_AFTER, MD_BEFORE, TOML_AFTER, TOML_BEFORE, TS_AFTER, TS_BEFORE } from './text'
import { makeBinary, makeLargeLog, makeMinified } from './generated'

export interface Fixture {
  id: string
  filename: string
  label: string
  /** what this fixture is here to expose */
  note: string
  /** heavy fixtures are mounted only on an explicit click, so a stray selection cannot hang the tab */
  heavy: boolean
  binary: boolean
  before: string
  after: string
  bytes: number
  lines: number
}

interface FixtureMeta extends Omit<Fixture, 'before' | 'after' | 'bytes' | 'lines'> {
  make: () => { before: string; after: string }
}

export const FIXTURES: FixtureMeta[] = [
  {
    id: 'rust',
    filename: 'crates/store/src/snapshot_writer.rs',
    label: 'snapshot_writer.rs',
    note: 'Dense Rust: lifetimes, attributes, macros, raw strings. Highlight quality, typical diff size.',
    heavy: false,
    binary: false,
    make: () => ({ before: RUST_BEFORE, after: RUST_AFTER }),
  },
  {
    id: 'ts',
    filename: 'ui/src/api/client.ts',
    label: 'client.ts',
    note: 'TypeScript with generics and decorated members — the other half of the highlight test.',
    heavy: false,
    binary: false,
    make: () => ({ before: TS_BEFORE, after: TS_AFTER }),
  },
  {
    id: 'toml',
    filename: '.keepster.toml',
    label: '.keepster.toml',
    note: 'The unsupported language: CodeMirror uses a legacy stream mode, Monaco falls back to `ini`.',
    heavy: false,
    binary: false,
    make: () => ({ before: TOML_BEFORE, after: TOML_AFTER }),
  },
  {
    id: 'md',
    filename: 'docs/notes.md',
    label: 'notes.md',
    note: 'Prose, tables and blockquotes — the "~/Documents/notes" case, not code at all.',
    heavy: false,
    binary: false,
    make: () => ({ before: MD_BEFORE, after: MD_AFTER }),
  },
  {
    id: 'log',
    filename: 'var/keepster.log',
    label: 'keepster.log (60k lines, ~4 MB)',
    note: 'Large file: scroll behaviour, diff computation cost, whether the tab survives at all.',
    heavy: true,
    binary: false,
    make: makeLargeLog,
  },
  {
    id: 'min',
    filename: 'dist/bundle.min.js',
    label: 'bundle.min.js (1 line, ~0.85 MB)',
    note: 'Pathological long line: both engines have a per-line rendering cut-off — where is it?',
    heavy: true,
    binary: false,
    make: makeMinified,
  },
  {
    id: 'bin',
    filename: 'assets/icon.png',
    label: 'icon.png (binary)',
    note: 'Not a text diff. What the UI shows instead is our decision, not the engine’s.',
    heavy: false,
    binary: true,
    make: makeBinary,
  },
]

const cache = new Map<string, Fixture>()

/** Materialises a fixture once per session; generated fixtures cost ~4 MB of string, so never twice. */
export function materialise(meta: FixtureMeta): Fixture {
  const hit = cache.get(meta.id)
  if (hit) return hit
  const { before, after } = meta.make()
  const fixture: Fixture = {
    ...meta,
    before,
    after,
    bytes: after.length,
    lines: after.split('\n').length,
  }
  cache.set(meta.id, fixture)
  return fixture
}
