import { describe, expect, it } from 'vitest'
import { changeShape, isRoutine } from './summary'
import { strings } from './strings'
import type { Snapshot } from './wire'

/** A watcher Snapshot that modified one file, with `over` changing any field. */
const snapshot = (over: Partial<Snapshot> = {}): Snapshot => ({
  id: '01K7',
  taken_at: '2026-10-12T09:30:00.000Z',
  trigger: 'watcher',
  incomplete: false,
  pinned: false,
  changed_paths: 1,
  added_paths: 0,
  deleted_paths: 0,
  ...over,
})

describe('changeShape', () => {
  it('names a Snapshot that only deleted files, in words', () => {
    const shape = changeShape(snapshot({ changed_paths: 3, deleted_paths: 3 }))

    expect(shape).toEqual({ kind: 'onlyDeleted', count: 3 })
    expect(strings.shape(shape)).toBe('3 deleted, nothing else')
  })

  it('names a Snapshot that only added files, in words', () => {
    const shape = changeShape(snapshot({ changed_paths: 1, added_paths: 1 }))

    expect(shape).toEqual({ kind: 'onlyAdded', count: 1 })
    expect(strings.shape(shape)).toBe('1 added, nothing else')
  })

  it('counts each kind of a mixed Snapshot, leaving out the kinds it has none of', () => {
    const shape = changeShape(snapshot({ changed_paths: 4, added_paths: 1, deleted_paths: 2 }))

    expect(shape).toEqual({ kind: 'mixed', added: 1, modified: 1, deleted: 2 })
    expect(strings.shape(shape)).toBe('1 added, 1 modified, 2 deleted')
    expect(strings.shape(changeShape(snapshot({ changed_paths: 2 })))).toBe('2 modified')
  })

  it('says a Snapshot that changed nothing changed nothing', () => {
    const shape = changeShape(snapshot({ changed_paths: 0 }))

    expect(shape).toEqual({ kind: 'none' })
    expect(strings.shape(shape)).toBe('nothing changed')
  })

  it('says a Snapshot the index has not counted yet is not counted', () => {
    const shape = changeShape(snapshot({ changed_paths: undefined, added_paths: undefined, deleted_paths: undefined }))

    expect(shape).toEqual({ kind: 'uncounted' })
    expect(strings.shape(shape)).toBe(strings.notCounted)
  })

  it('falls back to the total when the counts by kind are missing', () => {
    const shape = changeShape(snapshot({ changed_paths: 5, added_paths: undefined, deleted_paths: undefined }))

    expect(shape).toEqual({ kind: 'changed', count: 5 })
    expect(strings.shape(shape)).toBe('5 changed')
  })
})

describe('isRoutine', () => {
  it('counts an ordinary watcher Snapshot as routine', () => {
    expect(isRoutine(snapshot())).toBe(true)
    expect(isRoutine(snapshot({ changed_paths: 3, added_paths: 2 }))).toBe(true)
  })

  it('keeps every Snapshot that deleted anything', () => {
    expect(isRoutine(snapshot({ changed_paths: 3, deleted_paths: 1 }))).toBe(false)
  })

  it('keeps pinned and incomplete Snapshots', () => {
    expect(isRoutine(snapshot({ pinned: true }))).toBe(false)
    expect(isRoutine(snapshot({ incomplete: true }))).toBe(false)
  })

  it('keeps every Trigger but the watcher, an unknown one included', () => {
    for (const trigger of ['manual', 'restore', 'export', 'reconcile', 'periodic', 'retry', 'someday']) {
      expect(isRoutine(snapshot({ trigger }))).toBe(false)
    }
  })

  it('keeps a Snapshot whose deletions are not counted, rather than hide one it cannot judge', () => {
    expect(isRoutine(snapshot({ deleted_paths: undefined }))).toBe(false)
  })
})
