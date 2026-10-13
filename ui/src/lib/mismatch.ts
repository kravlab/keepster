/**
 * The failure `hunks.ts` raises, kept apart from it: `failures.ts` names it on every screen, and importing
 * it from `hunks.ts` would pull CodeMirror's merge code into the eager bundle the editor is lazy to avoid.
 */

/** The hunks do not describe these documents: they belong to other versions of the file. */
export class HunkMismatch extends Error {
  override name = 'HunkMismatch'
}
