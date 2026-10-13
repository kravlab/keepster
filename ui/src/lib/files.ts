/**
 * The changed-file list of the selected Snapshot, in one shape whichever endpoint it came from: the diff
 * summary against the Snapshot before it, or, for a Root's first Snapshot, what that Snapshot holds.
 */
import type { DiffFile, Entry, FileChange } from './wire'

/**
 * The size past which the daemon computes no hunks (`DIFF_MAX_SIZE`, 8 MiB). The first Snapshot's files
 * are shown whole rather than diffed; the same limit keeps a huge one out of the editor.
 */
export const TOO_LARGE_BYTES = 8 * 1024 * 1024

/** Why a card stands in place of the editor. */
export type Card = 'binary' | 'too_large' | 'withheld' | 'omitted' | 'symlink'

/** One row of the file list. */
export interface ShownFile {
  path: string
  /** Where a renamed file was on the older side. */
  oldPath: string | null
  change: FileChange
  /** Why no editor is shown for it; `null` when one is. */
  card: Card | null
}

/** The rows of a diff summary. Too large is named before binary: the daemon decides it first. */
export function fromDiff(files: readonly DiffFile[]): ShownFile[] {
  return files.map((file) => ({
    path: file.path,
    oldPath: file.old_path ?? null,
    change: file.change ?? 'modified',
    card: file.too_large ? 'too_large' : file.binary ? 'binary' : file.withheld ? 'withheld' : null,
  }))
}

/** The rows of a first Snapshot: everything it holds is new, directories aside. */
export function fromEntries(entries: readonly Entry[]): ShownFile[] {
  return entries
    .filter((entry) => entry.kind !== 'dir')
    .map((entry) => ({ path: entry.path, oldPath: null, change: 'added', card: entryCard(entry) }))
}

/** Why a first Snapshot's entry gets a card: what is not a file, or a file too large or binary to show. */
function entryCard(entry: Entry): Card | null {
  if (entry.kind !== 'file') return entry.kind === 'dir' ? null : entry.kind
  if ((entry.size ?? 0) > TOO_LARGE_BYTES) return 'too_large'
  return entry.binary ? 'binary' : null
}
