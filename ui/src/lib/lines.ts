/**
 * The long-line threshold: a file with a line longer than this is shown without syntax highlighting
 * (ticket 09). On such a line the grammar's parse costs seconds of main-thread time while scrolling.
 */

/**
 * The threshold in characters (UTF-16 code units, what the editor measures): `syntax_line_limit`'s
 * default (ticket 16). A constant until ticket 66 makes it a preference.
 */
export const SYNTAX_LINE_LIMIT = 20_000

/** Whether any line of `text` (split on `\n`) is longer than `limit`. */
export function hasLongLine(text: string, limit: number = SYNTAX_LINE_LIMIT): boolean {
  let start = 0
  for (;;) {
    const end = text.indexOf('\n', start)
    if ((end === -1 ? text.length : end) - start > limit) return true
    if (end === -1) return false
    start = end + 1
  }
}
