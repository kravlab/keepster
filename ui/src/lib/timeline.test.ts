import { describe, expect, it } from 'vitest'
import { loadedPredecessor, withNewer } from './timeline'

const s = (id: string) => ({ id })

describe('withNewer', () => {
  it('puts Snapshots newer than the shown ones on top, keeping the older pages already loaded', () => {
    expect(withNewer([s('3'), s('2'), s('1')], [s('5'), s('4'), s('3'), s('2')])?.map((x) => x.id)).toEqual([
      '5',
      '4',
      '3',
      '2',
      '1',
    ])
  })

  it('changes nothing when nothing is newer', () => {
    expect(withNewer([s('2'), s('1')], [s('2'), s('1')])?.map((x) => x.id)).toEqual(['2', '1'])
  })

  it('gives up when the fresh page no longer reaches the shown Snapshots', () => {
    // More arrived than one page holds: what lies between is unknown, so the list has to be loaded afresh.
    expect(withNewer([s('2'), s('1')], [s('9'), s('8')])).toBeNull()
  })

  it('gives up when nothing was shown', () => {
    expect(withNewer([], [s('1')])).toBeNull()
  })
})

describe('loadedPredecessor', () => {
  const list = [s('3'), s('2'), s('1')]

  it('is the next row of the list', () => {
    expect(loadedPredecessor(list, 0, true)).toBe('2')
  })

  it("is none past the Root's first Snapshot, once the oldest page is loaded", () => {
    expect(loadedPredecessor(list, 2, false)).toBeNull()
  })

  it('is unknown past the last loaded row while older pages remain', () => {
    expect(loadedPredecessor(list, 2, true)).toBeUndefined()
  })

  it('is unknown for a Snapshot the list does not hold', () => {
    expect(loadedPredecessor(list, -1, false)).toBeUndefined()
  })
})
