import { describe, expect, it } from 'vitest'
import { Chunk } from '@codemirror/merge'
import { Text } from '@codemirror/state'
import { daemonChanges } from './hunks'
import { HunkMismatch } from './mismatch'
import type { DiffHunk, DiffLine } from './wire'

/** A document split the way the editor splits it: on `\n` only. */
const doc = (s: string) => Text.of(s.split('\n'))

/** The chunks MergeView shows for `a` → `b` when its hunks come from the daemon. */
const shown = (a: string, b: string, hunks: DiffHunk[]) =>
  Chunk.build(doc(a), doc(b), { override: daemonChanges(hunks) }).map(({ fromA, toA, fromB, toB }) => ({
    fromA,
    toA,
    fromB,
    toB,
  }))

/** The chunks MergeView's own diff finds, for edits where only one answer is possible. */
const ownDiff = (a: string, b: string) =>
  Chunk.build(doc(a), doc(b)).map(({ fromA, toA, fromB, toB }) => ({ fromA, toA, fromB, toB }))

const c = (text: string): DiffLine => ({ kind: 'context', text })
const add = (text: string, newline = true): DiffLine => ({ kind: 'added', text, ...(newline ? {} : { newline }) })
const del = (text: string, newline = true): DiffLine => ({ kind: 'deleted', text, ...(newline ? {} : { newline }) })
const hunk = (old_start: number, old_lines: number, new_start: number, new_lines: number, lines: DiffLine[]) => ({
  old_start,
  old_lines,
  new_start,
  new_lines,
  lines,
})

describe('daemonChanges', () => {
  it('shows a modified line where the daemon put it', () => {
    const a = 'a\nb\nc\nd\ne\n'
    const b = 'a\nb\nC\nd\ne\n'
    const hunks = [hunk(1, 5, 1, 5, [c('a'), c('b'), del('c'), add('C'), c('d'), c('e')])]

    expect(shown(a, b, hunks)).toEqual(ownDiff(a, b))
  })

  it('shows each of several hunks', () => {
    const a = '1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n11\n12\n'
    const b = '1\nX\n3\n4\n5\n6\n7\n8\n9\n10\nY\n12\n'
    const hunks = [
      hunk(1, 5, 1, 5, [c('1'), del('2'), add('X'), c('3'), c('4'), c('5')]),
      hunk(8, 5, 8, 5, [c('8'), c('9'), c('10'), del('11'), add('Y'), c('12')]),
    ]

    expect(shown(a, b, hunks)).toEqual(ownDiff(a, b))
  })

  it('shows an added file as one insertion into an empty document', () => {
    const b = 'x\ny\n'

    expect(shown('', b, [hunk(0, 0, 1, 2, [add('x'), add('y')])])).toEqual(ownDiff('', b))
  })

  it('shows a deleted file as one deletion down to an empty document', () => {
    const a = 'x\ny\n'

    expect(shown(a, '', [hunk(1, 2, 0, 0, [del('x'), del('y')])])).toEqual(ownDiff(a, ''))
  })

  it('shows a line gaining its missing final newline', () => {
    const a = 'one\ntwo'
    const b = 'one\ntwo\n'

    expect(shown(a, b, [hunk(1, 2, 1, 2, [c('one'), del('two', false), add('two')])])).toEqual(ownDiff(a, b))
  })

  it('shows a line losing its final newline', () => {
    const a = 'a\nb\n'
    const b = 'a\nb'

    expect(shown(a, b, [hunk(1, 2, 1, 2, [c('a'), del('b'), add('b', false)])])).toEqual(ownDiff(a, b))
  })

  it('shows a line inserted before the first line', () => {
    const a = 'b\nc\n'
    const b = 'a\nb\nc\n'

    expect(shown(a, b, [hunk(1, 2, 1, 3, [add('a'), c('b'), c('c')])])).toEqual(ownDiff(a, b))
  })

  it("keeps the daemon's choice of which blank line was deleted", () => {
    // Deleting line 2 or line 3 gives the same text; the daemon said line 2, so line 2 is what is shown.
    const a = 'p\n\n\nq\n'
    const b = 'p\n\nq\n'

    expect(shown(a, b, [hunk(1, 4, 1, 3, [c('p'), del(''), c(''), c('q')])])).toEqual([
      { fromA: 2, toA: 3, fromB: 2, toB: 2 },
    ])
  })

  it('shows a blank first line deleted below an equal blank line', () => {
    // At the top there is no line before to anchor on, so the run is shown one line down: line 2 deleted,
    // which leaves the same text.
    const a = '\n\nx\n'
    const b = '\nx\n'

    expect(shown(a, b, [hunk(1, 3, 1, 2, [del(''), c(''), c('x')])])).toEqual([{ fromA: 1, toA: 2, fromB: 1, toB: 1 }])
  })

  it('compares CRLF lines with their carriage returns', () => {
    const a = 'x\r\ny\r\n'
    const b = 'x\r\nz\r\n'

    expect(shown(a, b, [hunk(1, 2, 1, 2, [c('x\r'), del('y\r'), add('z\r')])])).toEqual(ownDiff(a, b))
  })

  it('shows nothing for a file whose diff has no hunks', () => {
    expect(shown('', '', [])).toEqual([])
  })

  it('refuses hunks naming a line the document does not have there', () => {
    expect(() => shown('a\nb\n', 'a\nB\n', [hunk(1, 2, 1, 2, [c('a'), del('c'), add('B')])])).toThrow(HunkMismatch)
  })

  it('refuses hunks reaching past the end of the document', () => {
    expect(() => shown('a\n', 'a\nb\n', [hunk(1, 1, 1, 3, [c('a'), add('b'), add('c')])])).toThrow(HunkMismatch)
  })

  it('refuses a hunk whose lines disagree with its header', () => {
    expect(() => shown('a\nb\n', 'a\nB\n', [hunk(1, 3, 1, 3, [c('a'), del('b'), add('B')])])).toThrow(HunkMismatch)
  })
})
