/**
 * PROTOTYPE (wayfinder ticket 15) — the little that all three variants may share.
 *
 * Formatting and lookups only. Anything that decides *layout* belongs to a variant: a shared layout would
 * defeat the point of putting three structurally different answers side by side.
 */
import { FIXTURES, type Fixture, materialise } from '../fixtures'
import type { Change, ChangeKind, Snapshot, Trigger } from '../fixtures/timeline'

/**
 * The three modes ticket 09 named. `current` is the default: a file's history is almost always read as
 * "how does this differ from what is on disk right now".
 */
export type Mode = 'current' | 'prev' | 'full'

export function ago(at: number, now: number): string {
  const s = Math.max(1, Math.round((now - at) / 1000))
  if (s < 60) return `${s}s ago`
  const m = Math.round(s / 60)
  if (m < 60) return `${m} min ago`
  const h = Math.round(m / 60)
  if (h < 48) return `${h} h ago`
  return `${Math.round(h / 24)} d ago`
}

export function clock(at: number): string {
  return new Date(at).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })
}

export function day(at: number): string {
  return new Date(at).toLocaleDateString([], { weekday: 'short', day: 'numeric', month: 'short' })
}

export function shortId(id: string): string {
  return id.slice(-6)
}

export const KIND: Record<ChangeKind, { glyph: string; label: string; colour: string }> = {
  added: { glyph: '+', label: 'added', colour: '#7ee787' },
  modified: { glyph: '~', label: 'modified', colour: '#7aa2f7' },
  deleted: { glyph: '−', label: 'deleted', colour: '#ff7b72' },
  withheld: { glyph: '⦸', label: 'withheld', colour: '#d29922' },
}

export const TRIGGER: Record<Trigger, string> = {
  watcher: 'watcher',
  periodic: 'periodic',
  reconciliation: 'reconciliation',
  safety: 'safety snapshot',
  manual: 'manual',
}

/** True when a Snapshot changed nothing but deletions — the case a diff-shaped UI renders worst. */
export function deletionOnly(s: Snapshot): boolean {
  return s.changes.length > 0 && s.changes.every((c) => c.kind === 'deleted')
}

export function additionOnly(s: Snapshot): boolean {
  return s.changes.length > 0 && s.changes.every((c) => c.kind === 'added')
}

/** Ticket 09's editor fixtures, keyed by the path they pretend to be. */
const BY_PATH = new Map(FIXTURES.map((f) => [f.filename, f]))

export interface Content {
  /** null when this path has no editor fixture behind it — the variant shows a placeholder */
  fixture: Fixture | null
  /** heavy fixtures are armed by an explicit click: a 4 MB diff mounted on selection is how a tab dies */
  heavy: boolean
  label: string
}

export function contentFor(path: string | null, armed: Set<string>): Content {
  if (!path) return { fixture: null, heavy: false, label: 'no file selected' }
  const meta = BY_PATH.get(path)
  if (!meta) return { fixture: null, heavy: false, label: 'no fixture behind this path' }
  if (meta.heavy && !armed.has(meta.id)) return { fixture: null, heavy: true, label: meta.label }
  return { fixture: materialise(meta), heavy: false, label: meta.label }
}

export function fixtureId(path: string | null): string | null {
  return path ? (BY_PATH.get(path)?.id ?? null) : null
}

export function summarise(changes: Change[]): string {
  const n = { added: 0, modified: 0, deleted: 0, withheld: 0 }
  for (const c of changes) n[c.kind]++
  return (['added', 'modified', 'deleted', 'withheld'] as ChangeKind[])
    .filter((k) => n[k] > 0)
    .map((k) => `${n[k]} ${KIND[k].label}`)
    .join(', ')
}
