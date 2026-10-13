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
  /** How many of `changed_paths` it added; absent exactly when `changed_paths` is. */
  added_paths?: number
  /** How many of `changed_paths` it deleted; absent exactly when `changed_paths` is. */
  deleted_paths?: number
  /** With a timeline filtered by a path, how that path changed here (`keepster_api::Change`). */
  change?: 'added' | 'modified' | 'deleted'
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

/** A structural change a Restore must be told it may make (`keepster_api::Confirmation`). */
export type Confirmation = 'create_parents' | 'replace_type' | 'break_hardlinks' | 'overwrite_unrecoverable'

/** Whose permission bits a restored file ends up with (`keepster_api::ModesChoice`). */
export type ModesChoice = 'snapshot' | 'keep'

/** Which name a file restored over a name in another case keeps (`keepster_api::NameCaseChoice`). */
export type NameCaseChoice = 'keep_existing' | 'use_snapshot'

/** The answer to one path's question; exactly one field is set (`keepster_api::PathResolution`). */
export interface PathResolution {
  write?: boolean
  rename_to?: string
  name_case?: NameCaseChoice
  modes?: ModesChoice
}

/** What a client sends to restore (`keepster_api::RestoreRequest`). */
export interface RestoreRequest {
  /** The Snapshot whose versions are written. */
  from: string
  /** Root-relative paths; a directory means everything beneath it. */
  paths: string[]
  dry_run: boolean
  confirm: Confirmation[]
  /** Keyed by the Snapshot path each answer is for. */
  resolutions: Record<string, PathResolution>
  skip_unresolvable: boolean
}

/** What happened to one file of a Restore, or would (`keepster_api::RestoreOutcome`). */
export type RestoreOutcome = 'restored' | 'created' | 'unchanged' | 'skipped' | 'failed' | 'blocked'

/** Why a file of a Restore is not simply written (`keepster_api::RestoreIssue`). */
export interface RestoreIssue {
  /** An open set, as error codes are: `parent_missing`, `type_change`, `hardlinked`, … */
  reason: string
  /** The confirmation that settles it, for the reasons one does. */
  confirm?: Confirmation | null
  /** The resolution that settles it, for the reasons only the user can answer. */
  resolution?: 'write' | 'rename_to' | 'name_case' | 'modes' | null
  /** For `type_change` and `link_check_failed`: where, Root-relative. */
  at?: string | null
  /** For `overwrite_unrecoverable`: why the current content could not be got back. */
  why?: string | null
  /** For `name_case`: the name on disk. */
  existing?: string | null
  /** For `hardlinked`, the file's other names in the Root; for `name_collision`, the other paths. */
  paths?: string[]
  /** For `hardlinked`: how many more names it has outside the Root. */
  links_outside?: number | null
  /** For `modes_differ`: the bits on disk and in the Snapshot. */
  current_mode?: number | null
  snapshot_mode?: number | null
}

/** One file of a Restore (`keepster_api::RestoredPath`). */
export interface RestoredPath {
  path: string
  outcome: RestoreOutcome
  /** Where it is written, when that is not `path`. */
  target?: string | null
  /** For a `blocked` or `skipped` path, every reason. */
  issues?: RestoreIssue[]
  /** For a `failed` path, why. */
  error?: string | null
}

/** What a Restore did, or would do (`keepster_api::Restored`). */
export interface Restored {
  dry_run: boolean
  paths: RestoredPath[]
  /** The Snapshot that undoes it; absent from a dry run. */
  safety_snapshot?: string | null
}

/** An `application/problem+json` error body (`keepster_api::Problem`). */
export interface Problem {
  title: string
  status: number
  detail: string
  code: string
  /** With `confirmation_required`: every file of the Restore and what it needs. */
  paths?: RestoredPath[]
}

/** The web UI's preferences, as `GET /v1/preferences` answers and `PUT` takes them (`keepster_api::Preferences`). */
export interface Preferences {
  /**
   * `system`, `light` or `dark`. A string on the wire, as the contract has it: a daemon newer than this
   * page may name a palette it does not know, which it then paints as `system`.
   */
  theme: string
  /** The longest line, in characters, a diff is still syntax-highlighted with: 1 to 10 000 000. */
  syntax_line_limit: number
  wrap_lines: boolean
}

/** One path a path search found (`keepster_api::PathHit`). */
export interface PathHit {
  root: string
  path: string
  /** How many Snapshots changed it. */
  snapshots: number
  /** The first Snapshot that changed it. */
  first: string
  /** The last Snapshot that changed it: for a file already gone, the one that deleted it. */
  last: string
  /** Whether the newest Snapshot still holds it. */
  exists: boolean
}

/** A Root a search could not read (`keepster_api::Unsearched`). */
export interface Unsearched {
  root: string
  detail: string
}

/** What a path search found (`keepster_api::PathSearch`). */
export interface PathSearch {
  /** By Root, then by path. */
  hits: PathHit[]
  /** Whether there were more than the limit. */
  truncated: boolean
  unsearched?: Unsearched[]
}

/** What a content search reads in one Root (`keepster_api::SearchCost`). */
export interface SearchCost {
  root: string
  versions: number
  bytes: number
}

/** A line of a stored version that matched a content search. */
export interface ContentMatch {
  type: 'match'
  root: string
  path: string
  /** The version's content identity, opaque. */
  content: string
  /** The first Snapshot that held this content at this path. */
  first: string
  last: string
  /** Counted from 1. */
  line: number
  snippet: string
}

/** What one Root's content search is at: the totals of a planned walk. */
export interface SearchProgress {
  type: 'progress'
  root: string
  done: number
  total: number
}

/** What the whole content search looked at, sent last. */
export interface SearchSummary {
  type: 'summary'
  versions_scanned: number
  binaries_skipped: number
  withheld: number
  unreadable?: number
}

/** One line of the `GET /v1/search/content` stream (`keepster_api::ContentRecord`). */
export type ContentRecord =
  | { type: 'plan'; roots: SearchCost[] }
  | ContentMatch
  | { type: 'withheld'; root: string; path: string; reason: string; snapshot: string; kind?: string | null }
  | SearchProgress
  | { type: 'error'; root: string; detail: string }
  | SearchSummary
