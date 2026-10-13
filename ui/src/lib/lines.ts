/**
 * The long-line threshold: a file with a line longer than the limit is shown without syntax highlighting
 * (ticket 09). On such a line the grammar's parse costs seconds of main-thread time while scrolling.
 */

/**
 * Whether any line of `text` (split on `\n`) is longer than `limit` characters: UTF-16 code units, what the
 * editor measures. The limit is the `syntax_line_limit` preference (tickets 16 and 66).
 */
export function hasLongLine(text: string, limit: number): boolean {
  let start = 0
  for (;;) {
    const end = text.indexOf('\n', start)
    if ((end === -1 ? text.length : end) - start > limit) return true
    if (end === -1) return false
    start = end + 1
  }
}
