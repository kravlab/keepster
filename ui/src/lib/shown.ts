/**
 * What the diff pane is about to show, loaded for one Snapshot (ticket 83). Every result names the Snapshot
 * — and, where it is about one file, the path — it was found for, because the selection moves on at once
 * while an answer is still on its way: a result that does not name the current selection is stale, and
 * `currentFor` keeps it from the pane. Without the name, the pane was handed the new Snapshot with the
 * previous one's predecessor and file, and asked the daemon for a diff that does not exist.
 */
import { changedFiles, entryAt, previousSnapshot, snapshotEntries } from './api'
import { fromDiff, fromEntries, unchangedFile, type ShownFile } from './files'

/** A Snapshot's changed files: against its predecessor, or all it holds for a Root's first Snapshot. */
export interface SnapshotChanges {
  to: string
  /** The Snapshot before `to`; `null` for a Root's first Snapshot. */
  from: string | null
  files: ShownFile[]
}

/** One file's version in a Snapshot, as the file-first mode's scrubber shows it. */
export interface FileVersion {
  to: string
  path: string
  /** The Snapshot before `to`; `null` for a Root's first Snapshot. */
  from: string | null
  /** `null` when the Snapshot neither changed nor, being the first, holds the file. */
  file: ShownFile | null
}

/** A file the Snapshot did not change, as the Snapshot holds it. */
export interface UntouchedFile {
  to: string
  path: string
  file: ShownFile
}

/**
 * The changes of Snapshot `to`. `adjacent` is its predecessor when the loaded timeline can tell (`null`
 * past the Root's first Snapshot); `undefined` asks the daemon for it.
 */
export async function snapshotChanges(
  root: string,
  to: string,
  adjacent: string | null | undefined,
): Promise<SnapshotChanges> {
  const from = adjacent === undefined ? await previousSnapshot(root, to) : adjacent
  const files = from ? fromDiff(await changedFiles(root, from, to)) : fromEntries(await snapshotEntries(root, to))
  return { to, from, files }
}

/** The version of `path` in Snapshot `to`: its change against the Snapshot before, or whole in the first. */
export async function fileVersion(root: string, to: string, path: string): Promise<FileVersion> {
  const from = await previousSnapshot(root, to)
  let file: ShownFile | undefined
  if (from) {
    file = fromDiff(await changedFiles(root, from, to)).find((f) => f.path === path)
  } else {
    const entry = await entryAt(root, to, path)
    file = entry ? fromEntries([entry])[0] : undefined
  }
  return { to, path, from, file: file ?? null }
}

/** `path` as Snapshot `to` holds it, for a file that Snapshot did not change; `absent` when it holds nothing. */
export async function untouchedAt(root: string, to: string, path: string): Promise<UntouchedFile> {
  return { to, path, file: unchangedFile(path, await entryAt(root, to, path)) }
}

/**
 * `found` while it belongs to the Snapshot selected, and to the file selected when it is about one file;
 * otherwise `null`, so that nothing found for an earlier selection reaches the pane.
 */
export function currentFor<T extends { to: string; path?: string }>(
  found: T | null,
  to: string | null,
  path: string | null,
): T | null {
  if (found === null || found.to !== to) return null
  return found.path === undefined || found.path === path ? found : null
}
