import { describe, expect, it } from 'vitest'
import { TOO_LARGE_BYTES, fromDiff, fromEntries, unchangedFile } from './files'
import type { DiffFile } from './wire'

const diffFile = (over: Partial<DiffFile>): DiffFile => ({
  path: 'a.txt',
  change: 'modified',
  binary: false,
  too_large: false,
  hunks: [],
  ...over,
})

describe('fromDiff', () => {
  it('keeps each changed file with how it changed', () => {
    expect(
      fromDiff([diffFile({ path: 'a.txt', change: 'added' }), diffFile({ path: 'b.txt', change: 'deleted' })]),
    ).toEqual([
      { path: 'a.txt', oldPath: null, change: 'added', card: null },
      { path: 'b.txt', oldPath: null, change: 'deleted', card: null },
    ])
  })

  it('keeps where a renamed file came from', () => {
    expect(fromDiff([diffFile({ path: 'new.txt', old_path: 'old.txt', change: 'renamed' })])).toEqual([
      { path: 'new.txt', oldPath: 'old.txt', change: 'renamed', card: null },
    ])
  })

  it('puts a card in place of the editor for a file with no hunks to show', () => {
    expect(
      fromDiff([
        diffFile({ path: 'b', binary: true }),
        diffFile({ path: 'l', too_large: true }),
        diffFile({ path: 'w', withheld: 'too_large' }),
      ]).map((f) => f.card),
    ).toEqual(['binary', 'too_large', 'withheld'])
  })

  it('names a file too large for the daemon to diff as such even when it is also binary', () => {
    expect(fromDiff([diffFile({ binary: true, too_large: true })])[0]?.card).toBe('too_large')
  })
})

describe('fromEntries', () => {
  it('lists what the first Snapshot holds as added, leaving out directories', () => {
    expect(
      fromEntries([
        { path: 'src', kind: 'dir' },
        { path: 'src/a.rs', kind: 'file', binary: false, size: 10 },
      ]),
    ).toEqual([{ path: 'src/a.rs', oldPath: null, change: 'added', card: null }])
  })

  it('puts a card in place of the editor for what cannot be shown as text', () => {
    expect(
      fromEntries([
        { path: 'b', kind: 'file', binary: true, size: 3 },
        { path: 'l', kind: 'file', binary: false, size: TOO_LARGE_BYTES + 1 },
        { path: 'w', kind: 'withheld' },
        { path: 'o', kind: 'omitted' },
        { path: 's', kind: 'symlink' },
      ]).map((f) => f.card),
    ).toEqual(['binary', 'too_large', 'withheld', 'omitted', 'symlink'])
  })

  it('shows a file exactly at the size limit', () => {
    expect(fromEntries([{ path: 'x', kind: 'file', binary: false, size: TOO_LARGE_BYTES }])[0]?.card).toBeNull()
  })
})

describe('unchangedFile', () => {
  it('shows a file the Snapshot holds but did not change, as it holds it', () => {
    expect(unchangedFile('a.txt', { path: 'a.txt', kind: 'file', size: 3, binary: false })).toEqual({
      path: 'a.txt',
      oldPath: null,
      change: 'unchanged',
      card: null,
    })
  })

  it('gives a card to what the editor cannot show, as for a first Snapshot', () => {
    expect(unchangedFile('b.bin', { path: 'b.bin', kind: 'file', size: 3, binary: true })?.card).toBe('binary')
    expect(unchangedFile('l', { path: 'l', kind: 'symlink' })?.card).toBe('symlink')
  })

  it('says the Snapshot does not hold a path it has no entry for', () => {
    expect(unchangedFile('gone.txt', null)).toEqual({
      path: 'gone.txt',
      oldPath: null,
      change: 'unchanged',
      card: 'absent',
    })
  })
})
