/**
 * Search as the page holds it (ticket 66): where a result leads, results grouped by Root, and a content
 * search's stream folded into what the overlay shows.
 */
import type { ContentMatch, ContentRecord, PathHit, SearchCost, SearchSummary } from './wire'

/** How long typing pauses before a search runs, or before the header's field opens the overlay. */
export const SEARCH_PAUSE_MS = 250

/** Where picking a result takes the History screen: a file in a Snapshot of a Root. */
export interface SearchTarget {
  root: string
  path: string
  snapshot: string
}

/**
 * A path leads to the last Snapshot that changed it: its newest change, which for a file already gone is
 * the Snapshot that deleted it.
 */
export function pathTarget(hit: PathHit): SearchTarget {
  return { root: hit.root, path: hit.path, snapshot: hit.last }
}

/** A match leads to the first Snapshot that held its content, so the diff shows that version arriving. */
export function matchTarget(match: ContentMatch): SearchTarget {
  return { root: match.root, path: match.path, snapshot: match.first }
}

/** `items` grouped by Root, the Roots in the order they first appear and each group in arrival order. */
export function byRoot<T extends { root: string }>(items: readonly T[]): { root: string; items: T[] }[] {
  const groups = new Map<string, T[]>()
  for (const item of items) {
    const group = groups.get(item.root)
    if (group) group.push(item)
    else groups.set(item.root, [item])
  }
  return [...groups].map(([root, grouped]) => ({ root, items: grouped }))
}

/**
 * How many matches the overlay holds. A short query can match hundreds of thousands of lines across a
 * history, more than a page can draw; the rest are counted, and the search still runs to its end so that
 * its progress and summary stay true.
 */
export const SHOWN_MATCHES = 1000

/** A content search as far as its stream has come. */
export interface ContentSearch {
  /** What each Root will read, once the plan has come. */
  costs: SearchCost[] | null
  /** Each Root's latest progress, by Root. */
  progress: Record<string, { done: number; total: number }>
  /** The first [`SHOWN_MATCHES`] matches. */
  matches: ContentMatch[]
  /** The matches past those, counted only. */
  unshown: number
  /** How many paths had no content to search. */
  withheld: number
  /** The Roots that could not be searched, and why. */
  errors: { root: string; detail: string }[]
  /** Sent last: what the whole walk looked at. */
  summary: SearchSummary | null
}

/** A content search before its first record. */
export function emptyContentSearch(): ContentSearch {
  return { costs: null, progress: {}, matches: [], unshown: 0, withheld: 0, errors: [], summary: null }
}

/**
 * Adds one record to `state`, in place: a stream brings thousands, and copying the matches for each would
 * cost the square of their number. A record of a kind this page does not know is passed over, since the
 * stream may grow inside `/v1`.
 */
export function foldContent(state: ContentSearch, record: ContentRecord): void {
  switch (record.type) {
    case 'plan':
      state.costs = record.roots
      break
    case 'match':
      if (state.matches.length < SHOWN_MATCHES) state.matches.push(record)
      else state.unshown += 1
      break
    case 'withheld':
      state.withheld += 1
      break
    case 'progress':
      state.progress[record.root] = { done: record.done, total: record.total }
      break
    case 'error':
      state.errors.push({ root: record.root, detail: record.detail })
      break
    case 'summary':
      state.summary = record
      break
  }
}

/**
 * How many versions each planned Root has had looked at of how many, in plan order: a Root not yet begun
 * has done none of what the plan gave it. `null` before the plan.
 */
export function rootProgress(state: ContentSearch): { root: string; done: number; total: number }[] | null {
  if (state.costs === null) return null
  return state.costs.map((cost) => {
    const progress = state.progress[cost.root]
    return { root: cost.root, done: progress?.done ?? 0, total: progress?.total ?? cost.versions }
  })
}

/** How many versions the search has looked at of how many, over every Root; `null` before the plan. */
export function searchProgress(state: ContentSearch): { done: number; total: number } | null {
  const roots = rootProgress(state)
  if (roots === null) return null
  return roots.reduce((sum, root) => ({ done: sum.done + root.done, total: sum.total + root.total }), {
    done: 0,
    total: 0,
  })
}
