/**
 * Daemon hunks → the ranges MergeView draws.
 *
 * The daemon computes every diff (ticket 09); MergeView is only ever handed its hunks through
 * `diffConfig.override` and never diffs on its own. The override receives both documents as strings and
 * wants character ranges, so each run of deleted and added lines between context lines becomes one
 * `Change` spanning whole lines, each line's `\n` included.
 *
 * Contract with the editor: both documents are split on `\n` only (`EditorState.lineSeparator`), the way
 * git counts lines. With CodeMirror's default separators a lone `\r` would start a line git does not see,
 * and every hunk after it would land on the wrong lines.
 */
import { Change } from '@codemirror/merge'
import { HunkMismatch } from './mismatch'
import type { DiffHunk } from './wire'

/** A document's lines, addressed the way hunks address them: from 1. */
class Lines {
  private readonly starts: number[] = [0]

  constructor(private readonly text: string) {
    for (let i = text.indexOf('\n'); i !== -1; i = text.indexOf('\n', i + 1)) this.starts.push(i + 1)
  }

  /** The document's length in characters. */
  get length(): number {
    return this.text.length
  }

  /** Where line `n` starts; the document's end for the line after the last. */
  offset(n: number): number {
    return this.starts[n - 1] ?? this.text.length
  }

  /** Line `n` without its `\n`, or `undefined` past the end. */
  line(n: number): string | undefined {
    const start = this.starts[n - 1]
    if (start === undefined) return undefined
    const next = this.starts[n]
    return this.text.slice(start, next === undefined ? this.text.length : next - 1)
  }
}

/** The first line a hunk covers on one side: a side with no lines names the line before the run. */
const firstLine = (start: number, count: number) => (count === 0 ? start + 1 : start)

/**
 * One run as a `Change`: lines `[oldFrom, oldTo)` of the older document replaced by `[newFrom, newTo)` of
 * the newer one.
 *
 * MergeView widens every change to whole lines, and reads a change starting at the end of a line as
 * starting on the next one. When both runs start on a blank line, the start of that line is also its end,
 * so it would be shown one line too late. Starting one character earlier, at the end of the line before,
 * lands it back on the blank line. At the top of both documents there is no line before; there the run is
 * shown one line later instead, which reads the same: the two first lines are equal (both blank), and so
 * are the two lines after the run, which are context.
 */
function lineChange(
  older: Lines,
  oldFrom: number,
  oldTo: number,
  newer: Lines,
  newFrom: number,
  newTo: number,
): Change {
  const plain = () => new Change(older.offset(oldFrom), older.offset(oldTo), newer.offset(newFrom), newer.offset(newTo))
  const blank = older.line(oldFrom) === '' && newer.line(newFrom) === ''
  const atEnd = older.offset(oldFrom) === older.length || newer.offset(newFrom) === newer.length
  if (!blank || atEnd) return plain()
  if (oldFrom > 1 && newFrom > 1) {
    return new Change(older.offset(oldFrom) - 1, older.offset(oldTo), newer.offset(newFrom) - 1, newer.offset(newTo))
  }
  if (older.line(oldTo) !== undefined && older.line(oldTo) === newer.line(newTo)) {
    return lineChange(older, oldFrom + 1, oldTo + 1, newer, newFrom + 1, newTo + 1)
  }
  // Nothing after the run to move onto: it is shown a line late, as MergeView's own diff would show it.
  return plain()
}

/**
 * The `diffConfig.override` for one file's hunks. It throws `HunkMismatch` when a line the hunks name is
 * not in the document at that place, rather than drawing a diff of something else.
 */
export function daemonChanges(hunks: readonly DiffHunk[]): (a: string, b: string) => readonly Change[] {
  return (a, b) => {
    const older = new Lines(a)
    const newer = new Lines(b)
    const changes: Change[] = []

    for (const hunk of hunks) {
      let oldLine = firstLine(hunk.old_start, hunk.old_lines)
      let newLine = firstLine(hunk.new_start, hunk.new_lines)
      let run: { old: number; new: number } | null = null
      /** Ends the run of changed lines being collected, if any, as one `Change`. */
      const flush = () => {
        if (!run) return
        changes.push(lineChange(older, run.old, oldLine, newer, run.new, newLine))
        run = null
      }
      /** Checks that line `n` of one side is the line the hunk names there. */
      const expect = (lines: Lines, n: number, text: string, side: string) => {
        if (lines.line(n) !== text) throw new HunkMismatch(`${side} line ${n} is not the line the diff names`)
      }

      for (const line of hunk.lines) {
        if (line.kind === 'context') {
          flush()
          expect(older, oldLine++, line.text, 'old')
          expect(newer, newLine++, line.text, 'new')
          continue
        }
        run ??= { old: oldLine, new: newLine }
        if (line.kind === 'deleted') expect(older, oldLine++, line.text, 'old')
        else expect(newer, newLine++, line.text, 'new')
      }
      flush()

      const oldEnd = firstLine(hunk.old_start, hunk.old_lines) + hunk.old_lines
      const newEnd = firstLine(hunk.new_start, hunk.new_lines) + hunk.new_lines
      if (oldLine !== oldEnd || newLine !== newEnd) throw new HunkMismatch('a hunk has the wrong number of lines')
    }

    return changes
  }
}
