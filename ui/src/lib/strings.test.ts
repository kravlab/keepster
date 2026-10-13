import { describe, expect, it } from 'vitest'
import { strings } from './strings'

describe('the words for what a Snapshot changed', () => {
  it('says a Snapshot that only deleted files did nothing else', () => {
    expect(strings.shape({ kind: 'onlyDeleted', count: 3 })).toBe('3 deleted, nothing else')
  })

  it('says a Snapshot that only added files did nothing else', () => {
    expect(strings.shape({ kind: 'onlyAdded', count: 2 })).toBe('2 added, nothing else')
  })

  it('names each kind of change a mixed Snapshot made, leaving out the kinds it did not', () => {
    expect(strings.shape({ kind: 'mixed', added: 1, modified: 0, deleted: 4 })).toBe('1 added, 4 deleted')
  })

  it('gives only the total when the kinds are not known', () => {
    expect(strings.shape({ kind: 'changed', count: 7 })).toBe('7 changed')
  })

  it('says when nothing changed, and when the changes are not counted yet', () => {
    expect(strings.shape({ kind: 'none' })).toBe('nothing changed')
    expect(strings.shape({ kind: 'uncounted' })).toBe('not counted yet')
  })
})

describe('the words for a file’s versions', () => {
  it('counts the Snapshots that changed a file, as a lower bound while older ones are not loaded', () => {
    expect(strings.versions(1, false)).toBe('1 Snapshot changed it')
    expect(strings.versions(100, true)).toBe('At least 100 Snapshots changed it')
  })

  it("labels a scrubber tick with the version's moment and how it changed the file", () => {
    const moment = new Date('2026-10-12T23:45:01Z')

    expect(strings.versionTick(moment, 'deleted')).toBe(`${strings.at(moment)}, deleted`)
  })
})

describe('the Restore dialog’s words', () => {
  it('lists paths one after another', () => {
    expect(strings.list(['a.txt', 'b/c.txt'])).toBe('a.txt, b/c.txt')
  })

  it('sums up what happened to the files, kind by kind', () => {
    expect(strings.restore.summary({ restored: 2, created: 1 })).toBe('2 restored, 1 created')
  })

  it('says why content cannot go into the Safety Snapshot, and shows an unknown reason as the daemon names it', () => {
    expect(strings.restore.unrecoverable('too_large')).toBe('too large to capture')
    expect(strings.restore.unrecoverable('a_new_reason')).toBe('a_new_reason')
  })

  it('writes permission bits as four octal digits', () => {
    expect(strings.restore.modes(0o755, 0o644)).toBe('Its permission bits are 0755 on disk and 0644 in the Snapshot.')
  })

  it('names the other paths that are one file with the one to write', () => {
    expect(strings.restore.write(['A.txt', 'a.TXT'])).toBe(
      'This path and A.txt, a.TXT are one file here. Write this one.',
    )
  })
})

describe('the words of a search', () => {
  it('says how far a content search has got', () => {
    expect(strings.search.progress(2, 5)).toBe('Searched 2 of 5 versions')
  })

  it('counts the matches not shown', () => {
    expect(strings.search.unshown(3)).toBe('3 more matches not shown: narrow the search.')
  })

  it('sums up what a content search looked at', () => {
    expect(strings.search.summary(4, 1, 2)).toBe('Searched 4 versions; skipped 1 binary, 2 without content.')
  })

  it('places a match by its line and the moment its version was first captured', () => {
    const since = new Date('2026-10-12T10:00:00Z')

    expect(strings.search.match(12, since)).toBe(`line 12, since ${strings.at(since)}`)
  })

  it('says how often a path changed and whether it is gone, in one phrase', () => {
    expect(strings.search.pathDetail(1, true)).toBe('changed once')
    expect(strings.search.pathDetail(3, false)).toBe('changed 3 times, gone now')
  })

  it('says how far one Root of a content search has got', () => {
    expect(strings.search.rootProgress('/home/u/app', 2, 5)).toBe('/home/u/app: 2 of 5 versions')
  })

  it('names a Root the search could not read', () => {
    expect(strings.search.notSearched('/home/u/app', 'unreadable')).toBe('/home/u/app was not searched: unreadable')
  })
})
