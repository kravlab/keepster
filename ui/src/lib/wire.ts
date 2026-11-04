/**
 * The `/v1` shapes the SPA reads, written by hand and kept narrow: only the fields the UI uses. Each type
 * names its counterpart in the `keepster-api` crate, which is the contract (`openapi.json` is generated
 * from it).
 *
 * A path that is not UTF-8 comes as its lossy text plus a `_bytes` companion in base64url (ticket 74).
 * `api.ts` folds each companion into its path as `paths.ts` holds one, both ways, so past it a `path` is
 * the exact path and its companion is never read.
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
  /**
   * The reason behind a state that is not `watching`, such as `root_missing`; absent while it is. An
   * open set: a code the page has no words for is shown as it is.
   */
  state_code?: string | null
}

/**
 * Which a Root registered through a link is (`keepster_api::PathMode`): the link as typed, which it then
 * follows when the link is pointed elsewhere, or the real directory the link leads to now.
 */
export type PathMode = 'link' | 'canonical'

/** What a client sends to register a directory (`keepster_api::RegisterRoot`). */
export interface RegisterRoot {
  /** The directory to watch. */
  path: string
  /** For a path through a link, which the Root is; without it, such a path is refused to ask. */
  path_mode?: PathMode
}

/** One directory of a listing of the daemon's machine (`keepster_api::Subdirectory`). */
export interface Subdirectory {
  name: string
  /** Absolute: what to list next. */
  path: string
  /** A dot name, or the platform's hidden attribute: a picker shows these only when asked. */
  hidden: boolean
  /** Whether it can be listed in turn. */
  readable: boolean
}

/** The directories in one directory on the daemon's machine (`keepster_api::DirectoryListing`). */
export interface DirectoryListing {
  /** The directory listed, absolute. */
  path: string
  /** The directory above it; absent at the top. */
  parent?: string | null
  /** By name; files and links are left out. */
  directories: Subdirectory[]
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

/** A Root's pinned Snapshots, newest first (`keepster_api::Pins`); only `snapshots` is read. */
export interface Pins {
  snapshots: Snapshot[]
}

/** How a file changed (`keepster_api::FileChange`). */
export type FileChange = 'added' | 'modified' | 'deleted' | 'renamed'

/** One file's part of a diff (`keepster_api::DiffFile`). */
export interface DiffFile {
  path: string
  path_bytes?: string | null
  /** The path on the older side, for a rename. */
  old_path?: string | null
  old_path_bytes?: string | null
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
  path_bytes?: string | null
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
  /** One per path, `null` for each that is UTF-8; absent when every one is. */
  paths_bytes?: (string | null)[]
  dry_run: boolean
  confirm: Confirmation[]
  /** Keyed by the Snapshot path each answer is for. */
  resolutions: Record<string, PathResolution>
  /** The answers for paths that are not UTF-8, keyed by their bytes in base64url. */
  resolutions_bytes?: Record<string, PathResolution>
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
  at_bytes?: string | null
  /** For `overwrite_unrecoverable`: why the current content could not be got back. */
  why?: string | null
  /** For `name_case`: the name on disk. */
  existing?: string | null
  existing_bytes?: string | null
  /** For `hardlinked`, the file's other names in the Root; for `name_collision`, the other paths. */
  paths?: string[]
  paths_bytes?: (string | null)[]
  /** For `hardlinked`: how many more names it has outside the Root. */
  links_outside?: number | null
  /** For `modes_differ`: the bits on disk and in the Snapshot. */
  current_mode?: number | null
  snapshot_mode?: number | null
}

/** One file of a Restore (`keepster_api::RestoredPath`). */
export interface RestoredPath {
  path: string
  path_bytes?: string | null
  outcome: RestoreOutcome
  /** Where it is written, when that is not `path`. */
  target?: string | null
  target_bytes?: string | null
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
  /**
   * Machine-readable particulars. For `confirmation_required` to a register through a link: `path` as it
   * was sent, `real_path` where the link leads now, and `missing` = `path_mode`, the choice to send again.
   */
  details?: Record<string, string>
}

/** How a person installs `git` on the daemon's machine (`keepster_api::InstallHint`). */
export interface InstallHint {
  /** The command for the platform's package manager, when it is recognised. Keepster never runs it. */
  command?: string | null
  /** What to know besides. */
  note?: string | null
  /** Where to read more. */
  link: string
}

/** The external `git` store maintenance runs with, or why there is none (`keepster_api::GitStatus`). */
export interface GitStatus {
  /** The oldest version maintenance runs with. */
  minimum: string
  /** Where it is: found in configuration (`git.path`) or a stable place, never an inherited `PATH`. */
  path?: string | null
  /** The version it reported. */
  version?: string | null
  /** Why maintenance cannot run: `missing`, `macos_stub`, `not_executable`, `too_old`, `failed`; an open set. */
  reason?: string | null
  /** What running it said, for `not_executable` and `failed`, word for word. */
  detail?: string | null
  /** How to install one, while maintenance cannot run. */
  install?: InstallHint | null
}

/** The health surface, as `GET /v1/status` answers (`keepster_api::Status`): only the daemon's maintenance rows. */
export interface Status {
  daemon: {
    /** Whether store maintenance can run at all: `unknown`, `available` or `unavailable` (`keepster_api::Reported`). */
    maintenance: string
    git: GitStatus
  }
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
  path_bytes?: string | null
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
  path_bytes?: string | null
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
  | {
      type: 'withheld'
      root: string
      path: string
      path_bytes?: string | null
      reason: string
      snapshot: string
      kind?: string | null
    }
  | SearchProgress
  | { type: 'error'; root: string; detail: string }
  | SearchSummary

/** How many files and directories, and how many bytes of files (`keepster_api::PreviewTotals`). */
export interface PreviewTotals {
  /** Files, links and special files. */
  files: number
  directories: number
  /** The files' apparent sizes, summed. */
  bytes: number
  /** Excluded directories not entered, and so not measured. */
  pruned?: number
}

/** One path, relative to the directory previewed, and its apparent size (`keepster_api::PreviewSize`). */
export interface PreviewSize {
  path: string
  path_bytes?: string | null
  bytes: number
}

/**
 * A rule, named the way a person wrote it, and where (`keepster_api::RuleReference`, whose `Origin` is
 * flattened into it).
 */
export interface RuleReference {
  /** The pattern as written, `!` included where it was written. */
  pattern: string
  effect: 'exclude' | 'include'
  /** The level it was set at: `default`, `ignore-file`, `global`, `global-pattern`, `global-root`, `root` or `forget`. */
  level: string
  /** The file it was written in, or a description of a built-in value. */
  source: string
  /** Its line in that file, where it has one. */
  line?: number | null
}

/** Everything one rule — or none — decided (`keepster_api::PreviewGroup`). */
export interface PreviewGroup {
  /** `captured`, `excluded`, `withheld` or `unreadable`. */
  class: string
  /** Why, for a Withheld group. */
  reason?: string | null
  /** The rule, absent where none decided. */
  rule?: RuleReference | null
  files: number
  directories: number
  /** The files' apparent sizes, summed. */
  bytes: number
  /** The directories it pruned, named and unmeasured; absent when there are none. */
  pruned?: string[]
  /** `pruned` as their exact bytes, base64url, `null` for each that is UTF-8; absent when every one is. */
  pruned_bytes?: (string | null)[]
}

/**
 * What a whole Capture Preview walk found (`keepster_api::PreviewRecord::Summary`): the totals, what each
 * rule decided, the largest files on each side, and which of v1's claims for one Root — `files`,
 * `directories`, `bytes`, an open set — the directory is past.
 */
export interface PreviewSummary {
  type: 'summary'
  captured: PreviewTotals
  excluded: PreviewTotals
  /** What would be recorded without its content, by the daemon's reason. */
  withheld: Record<string, PreviewTotals>
  /** How many paths could not be looked at. */
  unreadable: number
  /** Every rule that decided something, and what it decided, largest first. */
  groups: PreviewGroup[]
  largest_captured: PreviewSize[]
  largest_left_out: PreviewSize[]
  past_ceiling: string[]
}

/**
 * One line of the `GET /v1/preview` stream at its default depth (`keepster_api::PreviewRecord`): what is
 * previewed, how many paths the walk has classified so far, and then its summary or why it stopped.
 * Only the fields the add flow reads are typed; an `entry`, sent only at a deeper depth, is not.
 */
export type PreviewRecord =
  | { type: 'header'; path: string; root?: string | null }
  | { type: 'progress'; walked: number }
  | { type: 'error'; detail: string }
  | PreviewSummary
