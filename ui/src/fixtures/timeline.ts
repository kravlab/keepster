/**
 * PROTOTYPE (wayfinder ticket 15) — the Snapshot timeline the three UX variants are judged on.
 *
 * Shapes follow ticket 07's timeline item (SnapshotId, timestamp, Trigger, incomplete flag, changed-path
 * count, and per-path kind of change) so that a variant cannot quietly depend on a field the API will not
 * return. Deterministic: one fixed seed, never `Math.random`. `NOW` is read once at module load so that
 * relative labels ("4 min ago") stay honest while the shape of the ladder stays fixed.
 */

/** Seeded PRNG (mulberry32) — the fixture must look identical on every reload of a given session. */
function rng(seed: number): () => number {
  let a = seed >>> 0
  return () => {
    a = (a + 0x6d2b79f5) >>> 0
    let t = Math.imul(a ^ (a >>> 15), 1 | a)
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296
  }
}

const rand = rng(0x5eed15)

/** Ticket 07: the Trigger that fired, carried on every timeline item. */
export type Trigger = 'watcher' | 'periodic' | 'reconciliation' | 'safety' | 'manual'

/**
 * How one path changed in one Snapshot. `withheld` is ticket 10's term and is deliberately *not* a failure:
 * the Snapshot holds the path but refuses its content, with a reason.
 */
export type ChangeKind = 'added' | 'modified' | 'deleted' | 'withheld'

export interface Change {
  path: string
  kind: ChangeKind
  /** only for `withheld` — ticket 10's reason, shown verbatim so a variant cannot invent a friendlier lie */
  reason?: string
}

export interface Snapshot {
  /** ULID in v1; here a ULID-shaped string, lexicographically ordered like the real one */
  id: string
  at: number
  trigger: Trigger
  /** ticket 05: meant to capture, could not read — distinct from `withheld` */
  incomplete: boolean
  /** ADR 0002: a Pin is a self-contained copy, so Retention cannot collect it */
  pinned: boolean
  changes: Change[]
}

const CROCKFORD = '0123456789ABCDEFGHJKMNPQRSTVWXYZ'

/** ULID-shaped: 10 chars of timestamp then 16 of entropy, so sorting by id sorts by time. */
function ulid(at: number): string {
  let ts = ''
  let n = at
  for (let i = 0; i < 10; i++) {
    ts = CROCKFORD[n % 32] + ts
    n = Math.floor(n / 32)
  }
  let tail = ''
  for (let i = 0; i < 16; i++) tail += CROCKFORD[Math.floor(rand() * 32)]
  return ts + tail
}

export const NOW = Date.now()

const MIN = 60_000
const HOUR = 60 * MIN
const DAY = 24 * HOUR

/**
 * The Working Tree the fixture pretends to watch. The first seven paths are the editor fixtures from ticket
 * 09, so selecting one shows a real diff instead of a placeholder; the rest exist to give the file column a
 * realistic length.
 */
export const TREE = [
  'crates/store/src/snapshot_writer.rs',
  'ui/src/api/client.ts',
  '.keepster.toml',
  'docs/notes.md',
  'var/keepster.log',
  'dist/bundle.min.js',
  'assets/icon.png',
  'crates/core/src/retention.rs',
  'crates/core/src/rules.rs',
  'crates/core/src/policy.rs',
  'crates/store/src/index.rs',
  'crates/store/src/segment.rs',
  'crates/daemon/src/main.rs',
  'crates/daemon/src/routes.rs',
  'crates/cli/src/main.rs',
  'crates/cli/src/exit.rs',
  'ui/src/App.svelte',
  'ui/src/lib/timeline.ts',
  'ui/src/lib/format.ts',
  'docs/adr/0001-segmented-snapshot-chains.md',
  'docs/adr/0002-pins-as-parentless-commits.md',
  'Cargo.toml',
  'README.md',
  'Makefile',
]

/** Paths that never change on their own — they move with the file that imports them. */
const HOT = TREE.slice(0, 4).concat(['crates/core/src/retention.rs', 'crates/daemon/src/routes.rs'])

function pick<T>(xs: T[]): T {
  return xs[Math.floor(rand() * xs.length)]
}

function ordinaryChanges(): Change[] {
  const n = 1 + Math.floor(rand() * 4)
  const out = new Map<string, Change>()
  for (let i = 0; i < n; i++) {
    const path = rand() < 0.55 ? pick(HOT) : pick(TREE)
    if (path === 'assets/icon.png' && rand() < 0.8) continue
    out.set(path, { path, kind: 'modified' })
  }
  return [...out.values()]
}

/**
 * Builds the ladder ticket 06 settled — everything for 24 h, hourly to 7 d, daily to 30 d — so the timeline
 * is dense at the top and sparse at the bottom, which is the legibility problem this prototype exists to test.
 */
function build(): Snapshot[] {
  const out: Snapshot[] = []
  const add = (at: number, trigger: Trigger, changes: Change[], extra: Partial<Snapshot> = {}) => {
    out.push({ id: ulid(at), at, trigger, incomplete: false, pinned: false, changes, ...extra })
  }

  // Last 24 h: the watcher's own cadence, 3-14 minutes apart.
  for (let t = NOW - 30 * MIN; t > NOW - DAY; t -= (3 + Math.floor(rand() * 11)) * MIN) {
    add(t, rand() < 0.85 ? 'watcher' : 'periodic', ordinaryChanges())
  }
  // 24 h - 7 d: one survivor per hour.
  for (let t = NOW - DAY - HOUR; t > NOW - 7 * DAY; t -= HOUR) {
    add(t, rand() < 0.7 ? 'watcher' : 'periodic', ordinaryChanges())
  }
  // 7 - 30 d: one survivor per day.
  for (let t = NOW - 7 * DAY - DAY; t > NOW - 30 * DAY; t -= DAY) {
    add(t, 'periodic', ordinaryChanges())
  }

  out.sort((a, b) => b.at - a.at)
  return out
}

const timeline = build()

/** Replaces the Snapshot nearest a given age, so the special cases sit at plausible places on the ladder. */
function at(ageMs: number, mutate: (s: Snapshot) => void): Snapshot {
  const target = NOW - ageMs
  let best = timeline[0]
  for (const s of timeline) if (Math.abs(s.at - target) < Math.abs(best.at - target)) best = s
  mutate(best)
  return best
}

/* ---- The cases the variants must survive, planted deliberately ---------------------------------- */

/** The scenario the whole tool exists for: an agent deleted a directory and nothing else. */
export const DELETION_ONLY = at(2 * HOUR + 20 * MIN, (s) => {
  s.trigger = 'watcher'
  s.changes = [
    { path: 'crates/core/src/retention.rs', kind: 'deleted' },
    { path: 'crates/core/src/rules.rs', kind: 'deleted' },
    { path: 'crates/core/src/policy.rs', kind: 'deleted' },
  ]
})

/** Its mirror: a Snapshot that only added files, which reads very differently in a diff-shaped UI. */
export const ADDITION_ONLY = at(9 * HOUR, (s) => {
  s.trigger = 'watcher'
  s.changes = [
    { path: 'crates/store/src/segment.rs', kind: 'added' },
    { path: 'crates/store/src/index.rs', kind: 'added' },
  ]
})

/** Ticket 05: meant to capture, could not read. The Snapshot exists and says so. */
export const INCOMPLETE = at(5 * HOUR, (s) => {
  s.incomplete = true
  s.changes = [...s.changes, { path: 'crates/daemon/src/main.rs', kind: 'modified' }]
})

/** Ticket 10: policy, not failure — the path is recorded, the content deliberately is not. */
export const WITHHELD = at(30 * HOUR, (s) => {
  s.changes = [
    ...s.changes,
    { path: 'var/keepster.log', kind: 'withheld', reason: 'over the 100 MiB size cap' },
    { path: 'dist/bundle.min.js', kind: 'withheld', reason: 'matched a secret denylist rule' },
  ]
})

/** The Safety Snapshot a Restore takes first — the reason a Restore is itself undoable. */
export const SAFETY = at(45 * MIN, (s) => {
  s.trigger = 'safety'
  s.changes = [
    { path: 'crates/core/src/retention.rs', kind: 'modified' },
    { path: 'crates/core/src/rules.rs', kind: 'modified' },
  ]
})

/** ADR 0002: pinned Snapshots survive Retention unconditionally. */
export const PINNED = [
  at(4 * DAY, (s) => {
    s.pinned = true
    s.trigger = 'manual'
  }),
  at(18 * DAY, (s) => {
    s.pinned = true
  }),
]

export const SNAPSHOTS: Snapshot[] = timeline

/** Ticket 07's per-path filter: the same list, narrowed to Snapshots that touched one path. */
export function touching(path: string): Snapshot[] {
  return SNAPSHOTS.filter((s) => s.changes.some((c) => c.path === path))
}

export function changeIn(s: Snapshot, path: string): Change | undefined {
  return s.changes.find((c) => c.path === path)
}

/** The Root the fixture describes. `state` is ticket 05's vocabulary. */
export const ROOT = {
  id: '01K4Z9QWERTY7HJ3MNPQRS',
  name: 'keepster',
  path: '/home/vk/projects/keepster',
  state: 'Watching' as 'Watching' | 'Degraded' | 'Unavailable',
  storeBytes: 418_000_000,
}
