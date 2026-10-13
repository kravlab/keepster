/**
 * The `/v1` shapes the SPA reads, written by hand and kept narrow: only the fields the UI uses. Each type
 * names its counterpart in the `keepster-api` crate, which is the contract (`openapi.json` is generated
 * from it).
 */

/** Which side of a diff a line is on (`keepster_api::LineKind`). */
export type LineKind = 'context' | 'added' | 'deleted'

/** One line of a hunk (`keepster_api::DiffLine`). */
export interface DiffLine {
  kind: LineKind
  /** The line without its `\n`; a CRLF file's lines keep their `\r`. */
  text: string
  /** `false` for a file's last line when the file ends without a newline; absent otherwise. */
  newline?: boolean
}

/** One run of changed lines with its context (`keepster_api::DiffHunk`). */
export interface DiffHunk {
  /** The run's first line on the older side, counting from 1; the line before it when `old_lines` is 0. */
  old_start: number
  old_lines: number
  /** The run's first line on the newer side, counting from 1; the line before it when `new_lines` is 0. */
  new_start: number
  new_lines: number
  lines: DiffLine[]
}

/** A Watched Root (`keepster_api::Root`). */
export interface Root {
  id: string
  path: string
  /** An open set (`keepster_api::RootState`); shown as it is. */
  state: string
}

/** One Snapshot on the timeline (`keepster_api::Snapshot`). */
export interface Snapshot {
  id: string
  /** UTC, RFC 3339, to the millisecond. */
  taken_at: string
  /** An open set: `manual`, `watcher`, …; shown as it is. */
  trigger: string
  incomplete: boolean
  /** How many paths differ from the Snapshot before it; absent while the index has no count. */
  changed_paths?: number
  pinned: boolean
}

/** A page of the timeline, newest first (`keepster_api::Timeline`). */
export interface Timeline {
  snapshots: Snapshot[]
  /** Fetches the next, older page; absent on the last one. */
  next_cursor?: string
}

/** How a file changed (`keepster_api::FileChange`). */
export type FileChange = 'added' | 'modified' | 'deleted' | 'renamed'

/** One file's part of a diff (`keepster_api::DiffFile`). */
export interface DiffFile {
  path: string
  /** The path on the older side, for a rename. */
  old_path?: string | null
  /** Absent for a path asked about that did not change. */
  change?: FileChange | null
  /** Either version is binary: no hunks. */
  binary: boolean
  /** Either version is larger than the daemon diffs (8 MiB): no hunks. */
  too_large: boolean
  /** Against the Working Tree only: why the next Snapshot would not hold it. */
  withheld?: string | null
  /** Empty in a summary, and for a binary or too large file. */
  hunks: DiffHunk[]
}

/** The answer of the diff endpoint (`keepster_api::Diff`). */
export interface Diff {
  files: DiffFile[]
}

/** What a Snapshot holds at a path (`keepster_api::EntryKind`). */
export type EntryKind = 'file' | 'dir' | 'symlink' | 'withheld' | 'omitted'

/** One path a Snapshot holds (`keepster_api::Entry`). */
export interface Entry {
  path: string
  kind: EntryKind
  binary?: boolean | null
  size?: number | null
}

/** The paths a Snapshot holds (`keepster_api::Entries`). */
export interface Entries {
  entries: Entry[]
}

/** An `application/problem+json` error body (`keepster_api::Problem`). */
export interface Problem {
  title: string
  status: number
  detail: string
  code: string
}
