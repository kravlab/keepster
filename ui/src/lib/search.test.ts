import { describe, expect, it } from 'vitest'
import {
  SHOWN_MATCHES,
  byRoot,
  emptyContentSearch,
  foldContent,
  matchTarget,
  pathTarget,
  rootProgress,
  searchProgress,
} from './search'
import type { ContentMatch, ContentRecord } from './wire'

const match = (path: string, line = 1, root = 'r1'): ContentMatch => ({
  type: 'match',
  root,
  path,
  content: 'c',
  first: 'S1',
  last: 'S9',
  line,
  snippet: 'needle',
})

const fold = (...records: ContentRecord[]) => {
  const state = emptyContentSearch()
  for (const record of records) foldContent(state, record)
  return state
}

describe('where a search result leads', () => {
  it('takes a path to the last Snapshot that changed it', () => {
    const hit = { root: 'r1', path: 'a.txt', snapshots: 3, first: 'S1', last: 'S3', exists: false }

    expect(pathTarget(hit)).toEqual({ root: 'r1', path: 'a.txt', snapshot: 'S3' })
  })

  it('takes a match to the first Snapshot that held its content, where that version appeared', () => {
    expect(matchTarget(match('a.txt'))).toEqual({ root: 'r1', path: 'a.txt', snapshot: 'S1' })
  })
})

describe('grouping results by Root', () => {
  it('keeps the Roots and the results in the order they came', () => {
    const items = [
      { root: 'r2', n: 1 },
      { root: 'r1', n: 2 },
      { root: 'r2', n: 3 },
    ]

    expect(byRoot(items)).toEqual([
      { root: 'r2', items: [items[0], items[2]] },
      { root: 'r1', items: [items[1]] },
    ])
  })

  it('makes no groups of nothing', () => {
    expect(byRoot([])).toEqual([])
  })
})

describe('folding a content search', () => {
  it('starts with nothing planned, found or summed', () => {
    expect(emptyContentSearch()).toEqual({
      costs: null,
      progress: {},
      matches: [],
      unshown: 0,
      withheld: 0,
      errors: [],
      summary: null,
    })
  })

  it('keeps the plan, the matches, the Roots that failed and the summary', () => {
    const summary = { type: 'summary', versions_scanned: 4, binaries_skipped: 1, withheld: 1 } as const

    const state = fold(
      { type: 'plan', roots: [{ root: 'r1', versions: 4, bytes: 100 }] },
      match('a.txt', 3),
      { type: 'withheld', root: 'r1', path: 'big.bin', reason: 'too_large', snapshot: 'S2' },
      { type: 'error', root: 'r2', detail: 'unreadable' },
      summary,
    )

    expect(state.costs).toEqual([{ root: 'r1', versions: 4, bytes: 100 }])
    expect(state.matches).toEqual([match('a.txt', 3)])
    expect(state.withheld).toBe(1)
    expect(state.errors).toEqual([{ root: 'r2', detail: 'unreadable' }])
    expect(state.summary).toEqual(summary)
  })

  it(`shows the first ${SHOWN_MATCHES} matches and only counts the rest`, () => {
    const records = Array.from({ length: SHOWN_MATCHES + 3 }, (_, i) => match('a.txt', i + 1))

    const state = fold(...records)

    expect(state.matches).toHaveLength(SHOWN_MATCHES)
    expect(state.matches.at(-1)?.line).toBe(SHOWN_MATCHES)
    expect(state.unshown).toBe(3)
  })

  it('ignores a record of a kind this page does not know', () => {
    const state = fold({ type: 'from_the_future' } as unknown as ContentRecord)

    expect(state).toEqual(emptyContentSearch())
  })
})

describe('the progress of a content search', () => {
  it('is nothing before the plan', () => {
    expect(searchProgress(emptyContentSearch())).toBeNull()
  })

  it('counts every Root’s planned versions, none done before any progress', () => {
    const state = fold({
      type: 'plan',
      roots: [
        { root: 'r1', versions: 10, bytes: 0 },
        { root: 'r2', versions: 5, bytes: 0 },
      ],
    })

    expect(searchProgress(state)).toEqual({ done: 0, total: 15 })
  })

  it('sums the latest progress of each Root', () => {
    const state = fold(
      {
        type: 'plan',
        roots: [
          { root: 'r1', versions: 10, bytes: 0 },
          { root: 'r2', versions: 5, bytes: 0 },
        ],
      },
      { type: 'progress', root: 'r1', done: 3, total: 10 },
      { type: 'progress', root: 'r1', done: 7, total: 10 },
      { type: 'progress', root: 'r2', done: 5, total: 5 },
    )

    expect(searchProgress(state)).toEqual({ done: 12, total: 15 })
  })
})

describe('the progress of each Root of a content search', () => {
  it('is nothing before the plan', () => {
    expect(rootProgress(emptyContentSearch())).toBeNull()
  })

  it('lists every planned Root in plan order, with its latest progress or none done yet', () => {
    const state = fold(
      {
        type: 'plan',
        roots: [
          { root: 'r1', versions: 10, bytes: 0 },
          { root: 'r2', versions: 5, bytes: 0 },
        ],
      },
      { type: 'progress', root: 'r2', done: 4, total: 5 },
    )

    expect(rootProgress(state)).toEqual([
      { root: 'r1', done: 0, total: 10 },
      { root: 'r2', done: 4, total: 5 },
    ])
  })
})
