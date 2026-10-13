import { describe, expect, it } from 'vitest'
import { arrivingSnapshot, keptSelection, versionAt } from './selection'

const list = (...ids: string[]) => ids.map((id) => ({ id }))

describe('keptSelection', () => {
  it('keeps a selection the list still contains', () => {
    expect(keptSelection(list('c', 'b', 'a'), 'b')).toBe('b')
  })

  it('moves a selection that fell out of the list to the newest Snapshot it still contains', () => {
    expect(keptSelection(list('c', 'a'), 'b')).toBe('c')
  })

  it('selects the newest when nothing was selected', () => {
    expect(keptSelection(list('c', 'a'), null)).toBe('c')
  })

  it('selects nothing in an empty list', () => {
    expect(keptSelection([], 'b')).toBeNull()
  })
})

describe('arrivingSnapshot', () => {
  const newest = (found: Record<string, string>) => (path: string) => Promise.resolve(found[path] ?? null)

  it('keeps the Snapshot carried in', async () => {
    await expect(arrivingSnapshot({ path: 'a.txt', snapshotId: 'S2' }, newest({ 'a.txt': 'S9' }))).resolves.toBe('S2')
  })

  it('resolves a file carried in without a Snapshot to the newest Snapshot that changed it', async () => {
    await expect(arrivingSnapshot({ path: 'a.txt', snapshotId: null }, newest({ 'a.txt': 'S9' }))).resolves.toBe('S9')
  })

  it('arrives at nothing in particular when nothing was carried, or the file was never changed', async () => {
    await expect(arrivingSnapshot({ path: null, snapshotId: null }, newest({}))).resolves.toBeNull()
    await expect(arrivingSnapshot({ path: 'b.txt', snapshotId: null }, newest({}))).resolves.toBeNull()
  })
})

describe('versionAt', () => {
  // A file's versions, newest first; ids order as Snapshot ids do, by when they were taken.
  const versions = list('s7', 's4', 's2')

  it('is the selected Snapshot when it changed the file', () => {
    expect(versionAt(versions, 's4')).toBe('s4')
  })

  it('is the version the file had in a Snapshot that did not change it: the newest one not newer', () => {
    expect(versionAt(versions, 's5')).toBe('s4')
  })

  it('is the newest version with no Snapshot selected', () => {
    expect(versionAt(versions, null)).toBe('s7')
  })

  it('is the oldest version for a Snapshot taken before the file existed', () => {
    expect(versionAt(versions, 's1')).toBe('s2')
  })

  it('is none for a file with no versions', () => {
    expect(versionAt([], 's4')).toBeNull()
  })
})
