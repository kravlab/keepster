/**
 * Every text the SPA shows, in one place. The project has no i18n system; keeping the strings out of the
 * components is what lets one be added without touching them.
 */
import { TOO_LARGE_BYTES, type Card } from './files'
import type { Theme } from './preferences'
import type { ChangeShape } from './summary'
import type {
  Confirmation,
  FileChange,
  ModesChoice,
  NameCaseChoice,
  PreviewGroup,
  PreviewTotals,
  RestoreOutcome,
  RuleReference,
} from './wire'

/** A moment as the page writes it out: date and time, where the user is. */
const when = (moment: Date) => moment.toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' })
/** Permission bits as `ls` users read them: four octal digits. */
const octal = (mode: number) => mode.toString(8).padStart(4, '0')
/** Paths, names or counts one after another. */
const list = (items: readonly string[]) => items.join(', ')
/** `count` of something, its noun singular for one: `1 file`, `2,048 files`. */
const counted = (count: number, one: string, many: string) => `${count.toLocaleString()} ${count === 1 ? one : many}`
/**
 * v1's claims for one Root, as a Capture Preview's `past_ceiling` names them, in words. An open set: a
 * claim without an entry is shown as it is.
 */
const CLAIMS: Partial<Record<string, string>> = {
  files: 'files',
  directories: 'directories',
  bytes: 'size',
}
/** The binary units a size past 1023 bytes is written in, smallest first. */
const SIZE_UNITS = ['KiB', 'MiB', 'GiB', 'TiB'] as const
/** A size as a person reads it, in binary units with one decimal, as `keepster preview` writes it. */
const byteSize = (bytes: number) => {
  if (bytes < 1024) return `${bytes} B`
  let value = bytes / 1024
  let unit = 0
  while (value >= 1024 && unit + 1 < SIZE_UNITS.length) {
    value /= 1024
    unit += 1
  }
  return `${value.toFixed(1)} ${SIZE_UNITS[unit]}`
}
/**
 * Why content in the Working Tree cannot go into the Safety Snapshot, by the daemon's reason code. The
 * codes are an open set: one this page does not know has no entry.
 */
const UNRECOVERABLE: Partial<Record<string, string>> = {
  too_large: 'too large to capture',
  excluded: 'excluded by the Include Rules',
  unreadable: 'unreadable',
  unsupported: 'not a kind of file Keepster captures',
}

/** A Root's state, by the daemon's name for it (`keepster_api::RootState`, an open set). */
const ROOT_STATES: Partial<Record<string, string>> = {
  watching: 'Watching',
  degraded: 'Degraded',
  unavailable: 'Unavailable',
}
/**
 * Why a Root is not `watching`, by its `state_code`, in the words `keepster status` uses where it has
 * them. The codes are an open set: one this page does not know has no entry, and is shown as it is.
 */
const ROOT_REASONS: Partial<Record<string, string>> = {
  root_missing: 'Its directory is not found at this path. Was it moved?',
  volume_mismatch: 'Its disk is not mounted there, so nothing is captured; its history reads as ever.',
  root_overlaps:
    'Its link now leads inside another Root, so nothing is captured; point the link elsewhere. Its history reads as ever.',
  store_unreachable: 'Its history store cannot be written, so nothing is captured, and nowhere else instead.',
  store_too_new: 'Its history was written by a newer Keepster, which this one cannot open.',
  record_unreadable: 'Its registration cannot be read.',
  config_invalid:
    'Its configuration is invalid: the last valid version stays in force, and without one nothing is captured.',
  capture_held:
    'Every file is gone and nothing proves it is the disk it was, so capturing is held until a Snapshot is asked for.',
  capture_unavailable: 'Every Trigger is off: only a Snapshot someone asks for is ever taken.',
  coverage_not_guaranteed: 'What changed while nothing watched is not captured until a Reconciliation has run.',
  watch_ceiling: 'Watching it would exceed the daemon’s limit on watches, so it is reconciled periodically instead.',
  watch_unavailable: 'The system refused to watch it, so it is reconciled periodically instead.',
  watch_blind: 'Changes made on the server are never announced here; it is reconciled periodically.',
  mtime_coarse:
    'Its filesystem keeps coarse modification times, so a change can go unseen until its content is hashed.',
}

export const strings = {
  appName: 'Keepster',
  /** A moment written out in full: date and time, where the user is. */
  at: when,
  list,
  byteSize,
  loading: 'Loading…',
  failed: 'Something went wrong:',
  /** Backs out of a dialog, a question or a search under way, every one of them alike. */
  cancel: 'Cancel',

  /** Shown after a 401: the daemon restarted or the cookie is gone. `command` is set as code. */
  sessionEnded: {
    before: 'This session has ended. To open Keepster again, run',
    command: 'keepster ui',
    after: '.',
  },
  /** A refusal the daemon gave no explanation for. */
  httpStatus: (status: number) => `The daemon answered with HTTP ${status}.`,
  unexpectedAnswer: 'The daemon answered in a shape this page cannot read.',
  hunkMismatch: 'The diff does not match the versions of the file it was drawn over.',

  rootLabel: 'Root',
  noRoots: 'No Roots are registered.',
  /** In the Root picker while no Root is shown. */
  noRootChosen: 'None',
  chooseRoot: 'Choose a Root to see its history.',

  /** The Root management page (ticket 92). */
  roots: {
    /** The header's way to the page, and the empty state's. */
    manage: 'Manage Roots',
    title: 'Watched Roots',
    /** Opens a Root's history from its row. */
    open: 'Open history',
    /** A Root's state, by the daemon's name for it; an open set, so one not here is shown as it is. */
    state: (state: string) => ROOT_STATES[state] ?? state,
    /** Why a Root is not `watching`, by its `state_code`. */
    reason: (code: string) => ROOT_REASONS[code] ?? code,
    /** Opens the add dialog, on the page and in the empty state, and heads it. */
    add: 'Add a directory',
    /** Opens the unwatch dialog from a Root's row. */
    unwatch: 'Unwatch…',
    /** Opens the Move dialog from a Root's row. */
    move: 'Move…',
    /**
     * Why `path` was not added, or not recorded as where a Root now is, when it overlaps the Root at
     * `rootPath`, a Root the page knows.
     */
    overlaps: (path: string, rootPath: string) => `${path} overlaps the Root at ${rootPath}`,
    /**
     * The question a path through a link needs, adding a directory or recording a Move, worded as
     * `keepster watch` and `keepster move` ask it.
     */
    link: {
      question: (path: string, realPath: string) =>
        `${path} leads through a link to ${realPath}. Use the link, following it when it is pointed ` +
        `elsewhere, or the real directory?`,
      asLink: 'Use the link',
      asReal: 'Use the real directory',
    },
  },

  /** The Move dialog (#32): recording where a Root's directory is now, as `keepster move` does. */
  move: {
    title: (path: string) => `Record a Move of ${path}`,
    /** What a Move is and is not, said before anything is chosen. */
    note:
      'Keepster moves no files: it only records where this directory is now, once you have moved it. ' +
      'The Root and its history stay the same.',
    /** Before the directory the browser has listed, the one that will be recorded. */
    chosen: 'Now at',
    go: 'Record the Move',
    working: 'Recording…',
    /** Before the daemon's reason the Move was not recorded. */
    failed: 'Not recorded:',
  },

  /** The unwatch dialog (#31), worded as `keepster unwatch` says it. */
  unwatch: {
    title: (path: string) => `Stop watching ${path}`,
    /** What becomes of the history, by `Outlook` in `lib/unwatching.svelte`. */
    outlook: {
      /** The Root has a pinned Snapshot and the history is not to be erased. */
      kept:
        'Its history is kept: it decays down to its last state, which is pinned. Watching the directory ' +
        'again resumes it.',
      /** The Root has no pinned Snapshot: it never captured anything. */
      nothingToKeep: 'It has no history to keep.',
      /** The pins are not read yet, or could not be: claims neither. */
      unknown: 'Its changes stop being captured; any history it has is kept unless you erase it.',
      /** Erasing is chosen, as `acknowledge` words it. */
      erasing: 'Its history is erased with it: every captured version of this Root is lost.',
    },
    /** The option that also erases the history. */
    erase: 'Also erase its history',
    /** The separate acknowledgement an erasing needs. */
    acknowledge: 'I understand that erasing cannot be undone: every captured version of this Root is lost.',
    /** The button, by the choice (a key of `wording` in `lib/unwatch`). */
    go: 'Unwatch',
    goErasing: 'Erase history and unwatch',
    working: 'Working…',
    /** Before the daemon's reason the Root was not unwatched. */
    failed: 'Not unwatched:',
  },

  /** The add dialog and the banner after it (ticket 93). */
  adding: {
    /** Before the directory the browser has listed, the one that will be added. */
    chosen: 'Watch',
    go: 'Add',
    working: 'Adding…',
    /** Before the daemon's reason a directory was not added. */
    failed: 'Not added:',
    /** The Capture Preview run before registering (ticket 94), while it walks. */
    preview: {
      looking: (path: string) => `Looking at what watching ${path} would capture…`,
      walked: (count: number) => `${counted(count, 'path', 'paths')} so far`,
      /** Why nothing was added: the preview was refused because another one walks, as `keepster watch` says it. */
      busy: 'Another Capture Preview is walking, and this directory must be looked at first; try again.',
    },
    /** The question a directory past what one Root is claimed to hold needs, and the summary it shows. */
    claim: {
      question: (path: string, past: readonly string[]) =>
        `${path} is past what one Root is claimed to hold, in ${list(past.map((claim) => CLAIMS[claim] ?? claim))}. ` +
        `Watch it anyway?`,
      captured: 'Captured',
      capturedTotals: (t: PreviewTotals) =>
        `${counted(t.files, 'file', 'files')}, ${counted(t.directories, 'directory', 'directories')}, ${byteSize(t.bytes)}`,
      excluded: 'Excluded',
      excludedTotals: (t: PreviewTotals) =>
        `${counted(t.files, 'file', 'files')}, ${byteSize(t.bytes)}` +
        (t.pruned ? `, and ${counted(t.pruned, 'directory', 'directories')} not entered` : ''),
      withheld: 'Without content',
      /** What is recorded without its content for `reason`, the daemon's word for why. */
      withheldTotals: (reason: string, t: PreviewTotals) =>
        `${counted(t.files + t.directories, 'path', 'paths')}, ${byteSize(t.bytes)} — ${reason}`,
      unreadable: 'Unreadable',
      unreadableCount: (count: number) => counted(count, 'path', 'paths'),
      /** What each rule decided, largest first: the title of the list `groupTotals` and `groupPruned` fill. */
      groups: 'By the rule that decided',
      /** A rule as a person reads it: the pattern, and where it was written. */
      rule: (rule: RuleReference) =>
        `${rule.pattern} (${rule.level}, ${rule.source}${rule.line == null ? '' : ` line ${rule.line}`})`,
      /** What `rule` decided in `group`: the files it took, of the class it put them in. */
      groupTotals: (group: PreviewGroup, rule: RuleReference) =>
        `${group.class}: ${counted(group.files, 'file', 'files')}, ${byteSize(group.bytes)} — ${strings.adding.claim.rule(rule)}`,
      /** A directory of `className` that `rule` excluded and the preview did not enter. */
      groupPruned: (className: string, path: string, rule: RuleReference) =>
        `${className}: ${path}/ not entered — ${strings.adding.claim.rule(rule)}`,
      largest: 'Largest captured',
      largestLeftOut: 'Largest left out',
      /** The one number a preview never gives, said as `keepster watch` says it. */
      noGrowth: 'Store growth is not estimated: only the capture itself can know what the history store will hold.',
      watch: 'Watch it anyway',
      decline: 'Don’t watch',
    },
    /** The banner once the directory is a Root; the page stays where it is. */
    added: (path: string) => `${path} is now watched.`,
    open: 'Open its history',
    dismiss: 'Dismiss',
  },

  /**
   * What the "added" banner says while store maintenance cannot run, as `keepster watch` says it (ticket
   * 11), without the part about the shell's own `git`: a page cannot know what a shell runs.
   */
  maintenance: {
    title: 'Store maintenance is unavailable.',
    /** The `git` the daemon found, where it knows its version as well. */
    gitAt: (version: string, path: string) => `git ${version} at ${path}`,
    /** The `git` the daemon found, without a version. */
    gitPath: (path: string) => `git ${path}`,
    /**
     * Why maintenance cannot run, by the daemon's `git.reason`, in the words `keepster watch` uses. An open
     * set: a reason without an entry is shown as it is. `too_old` and `failed` are worded below.
     */
    reasons: {
      missing: 'No git was found',
      macos_stub: 'Only the macOS stub is installed, without the Command Line Tools',
      not_executable: 'It could not be run',
    } as Partial<Record<string, string>>,
    /** Why, for `too_old`. */
    tooOld: (minimum: string) => `It is older than ${minimum}`,
    /** What `why` costs while no usable git is installed. */
    unreclaimed: (why: string, minimum: string) =>
      `${why}; nothing Retention removes is reclaimed until git ${minimum} or newer is installed.`,
    /** Why, and what it costs, for `failed`. */
    failed:
      'A Maintenance Pass with it failed; nothing Retention removes is reclaimed until the daemon restarts or git.path changes.',
    /** What running `git` said, word for word. */
    gitSaid: (detail: string) => `git said: ${detail}`,
    /** Before the command that installs `git`, which is set as code; Keepster never runs it. */
    install: 'Install it:',
  },

  /** The folder browser over the daemon's machine (ticket 93), for adding a directory and recording a Move. */
  folders: {
    label: 'Folders',
    /** The field holding the directory listed, where an absolute path can be typed or pasted. */
    path: 'Path',
    go: 'Go',
    up: 'Up',
    showHidden: 'Show hidden folders',
    /** Beside a folder that cannot be listed, which cannot be gone into. */
    unreadable: 'cannot be listed',
    empty: 'No folders here.',
  },

  themeLabel: 'Theme',
  /** The palettes, by the daemon's name for each. */
  themes: { system: 'System', light: 'Light', dark: 'Dark' } satisfies Record<Theme, string>,
  /** Before the reason the preferences could not be read, or a change to them was not stored and undone. */
  preferencesFailed: { read: 'Preferences could not be read:', save: 'Not saved:' },

  modesLabel: 'View',
  /** The view modes' tabs: the question each answers is the user's way in, not a screen's name. */
  modes: { rail: 'Snapshots', files: 'Files', feed: 'Activity' },

  railLabel: 'Snapshots',
  noSnapshots: 'This Root has no Snapshots yet.',
  loadOlder: 'Load older',
  hideRoutine: 'Hide routine Snapshots',
  onlyFile: (name: string) => `Only Snapshots that changed ${name}`,
  notCounted: 'not counted yet',
  /** What a Snapshot changed, in words: a Snapshot that only deleted or only added files says so. */
  shape: (shape: ChangeShape): string => {
    switch (shape.kind) {
      case 'uncounted':
        return strings.notCounted
      case 'none':
        return 'nothing changed'
      case 'onlyDeleted':
        return `${shape.count} deleted, nothing else`
      case 'onlyAdded':
        return `${shape.count} added, nothing else`
      case 'changed':
        return `${shape.count} changed`
      case 'mixed':
        return (['added', 'modified', 'deleted'] as const)
          .filter((kind) => shape[kind] > 0)
          .map((kind) => `${shape[kind]} ${kind}`)
          .join(', ')
    }
  },
  pinned: 'Pinned',
  incomplete: 'Incomplete',
  /** The rail's mark on an incomplete Snapshot; `incomplete` is its accessible name. */
  incompleteMark: '!',

  feedLabel: 'What happened, newest first',
  /** A feed card's changed paths past the first few, behind a button. */
  morePaths: (count: number) => `Show ${count} more`,
  /** The feed's diff overlay names the Snapshot it shows. */
  snapshotOf: (moment: Date) => `Snapshot of ${when(moment)}`,
  closeDiff: 'Close',

  treeLabel: 'Files of the Root',
  chooseFile: 'Choose a file to see every Snapshot that changed it.',
  scrubberLabel: 'Versions of this file, oldest first',
  versions: (count: number, more: boolean) =>
    `${more ? 'At least ' : ''}${count} ${count === 1 ? 'Snapshot' : 'Snapshots'} changed it`,
  /** A scrubber tick's name: when its version was taken, and how it changed the file. */
  versionTick: (moment: Date, change: 'added' | 'modified' | 'deleted') =>
    `${when(moment)}, ${strings.changes[change].label}`,
  olderVersion: 'Older',
  newerVersion: 'Newer',
  noVersions: 'No Snapshot has changed this file.',

  filesLabel: 'Changed files',
  fileViewLabel: 'Show files as',
  listView: 'Changes',
  treeView: 'Tree',
  unchangedHere: (name: string) =>
    `Unchanged here: this Snapshot did not change it, so it is shown as this Snapshot holds it. ` +
    `"Only Snapshots that changed ${name}" lists its versions.`,
  incompleteBanner: 'Incomplete: some files could not be read when this Snapshot was taken.',
  firstSnapshot: 'The first Snapshot of this Root: everything in it is new.',
  noChanges: 'Nothing changed in this Snapshot.',

  selectFile: 'Select a file to see what changed.',
  wrapLines: 'Wrap lines',
  renamedFrom: (path: string) => `renamed from ${path}`,
  /** Around the field holding the limit, which the reader can raise right there. */
  highlightOff: {
    before: 'Syntax highlighting is off: a line is longer than',
    after: 'characters.',
    field: 'Longest line still highlighted, in characters',
  },

  /** The text of the card shown in place of the editor. */
  cards: {
    binary: 'Binary file: there is no text to compare.',
    too_large: `Larger than ${TOO_LARGE_BYTES / (1024 * 1024)} MiB: too large to compare.`,
    withheld: 'Withheld: this Snapshot holds no content for it.',
    omitted: 'Could not be read when this Snapshot was taken.',
    symlink: 'A symbolic link: there is no file content to show.',
    absent: 'This Snapshot holds nothing at this path: the file did not exist then.',
  } satisfies Record<Card, string>,

  /** The Restore dialog: one for every view mode, so an irreversible action has one face (ticket 15). */
  restore: {
    open: 'Restore…',
    title: (path: string) => `Restore ${path}`,
    version: (moment: Date) => `The version in the Snapshot of ${when(moment)}.`,
    beforeDeletion: (moment: Date) =>
      `The version in the Snapshot of ${when(moment)}, the last one before it was deleted.`,
    planning: 'Checking what Restore would do…',
    dryRun: 'Dry run: nothing has been written yet. Restore will do this:',
    /** What will happen to a file, in a dry run. */
    planned: {
      restored: 'overwrites the file on disk',
      created: 'creates it: nothing is there now',
      unchanged: 'already matches: nothing to write',
      skipped: 'is skipped',
      failed: 'cannot be written',
      blocked: 'needs an answer below',
    } satisfies Record<RestoreOutcome, string>,
    /** What happened to a file, once restored. */
    outcome: {
      restored: 'restored',
      created: 'created',
      unchanged: 'already matched',
      skipped: 'skipped',
      failed: 'failed',
      blocked: 'not written',
    } satisfies Record<RestoreOutcome, string>,
    writtenAs: (target: string) => `written as ${target}`,
    /** What happened to the files, once restored, kind by kind. */
    summary: (counts: Partial<Record<RestoreOutcome, number>>) =>
      list(
        Object.entries(counts).map(
          ([outcome, count]) => `${count} ${strings.restore.outcome[outcome as RestoreOutcome]}`,
        ),
      ),
    /** The checkbox agreeing to a structural change, for every file that needs it. */
    confirm: {
      create_parents: 'Create the missing folders it goes in',
      replace_type: 'Replace what is in its place: a folder, a link or another kind of file',
      break_hardlinks: 'Break its hard links: its other names keep the content they have now',
      overwrite_unrecoverable: 'Overwrite content that the Safety Snapshot cannot hold, so this cannot be undone',
    } satisfies Record<Confirmation, string>,
    /** Why content in the Working Tree cannot go into the Safety Snapshot; an unknown code as it is. */
    unrecoverable: (why: string) => UNRECOVERABLE[why] ?? why,
    nameCase: (existing: string) => `A file named ${existing}, in another case, is on disk.`,
    nameCaseChoice: {
      keep_existing: 'Keep the name on disk',
      use_snapshot: "Use the Snapshot's name",
    } satisfies Record<NameCaseChoice, string>,
    modes: (now: number, then: number) =>
      `Its permission bits are ${octal(now)} on disk and ${octal(then)} in the Snapshot.`,
    modesUnknown: "Its permission bits on disk differ from the Snapshot's.",
    modesChoice: {
      snapshot: "Use the Snapshot's",
      keep: 'Keep those on disk',
    } satisfies Record<ModesChoice, string>,
    renameTo: 'This name cannot be written here. Write it as:',
    write: (others: string[]) => `This path and ${list(others)} are one file here. Write this one.`,
    unrestorable: 'These cannot be restored:',
    skip: 'Skip them and restore the rest',
    safety:
      'Before anything is written, Keepster takes a Safety Snapshot of the current state. Restoring from it ' +
      'undoes this Restore, except that a file this Restore creates stays.',
    changedSince: 'The files changed since this plan was made. This is the plan as it stands now.',
    go: 'Restore',
    working: 'Restoring…',
    close: 'Close',
    done: 'Done.',
    undo: (moment: Date, id: string) => `To undo it, restore from the Safety Snapshot of ${when(moment)} (${id}).`,
    noSafety: 'Nothing was written, so no Safety Snapshot was taken.',
  },

  /** The search field in the header and the overlay it opens (ticket 66). */
  search: {
    field: 'Search',
    placeholder: 'Search…',
    title: 'Search',
    query: 'Search for',
    content: 'Content',
    everyRoot: 'Every Root',
    results: 'Results',
    close: 'Close',
    searching: 'Searching…',
    nothing: 'Nothing found.',
    cancelled: 'Search cancelled.',
    truncated: 'More paths match than are shown: narrow the search.',
    /** A Root the search could not read, named by its directory. */
    notSearched: (root: string, detail: string) => `${root} was not searched: ${detail}`,
    progress: (done: number, total: number) =>
      `Searched ${done.toLocaleString()} of ${total.toLocaleString()} versions`,
    unshown: (count: number) => `${count.toLocaleString()} more matches not shown: narrow the search.`,
    summary: (scanned: number, binaries: number, withheld: number) =>
      `Searched ${scanned.toLocaleString()} versions; skipped ${binaries.toLocaleString()} binary, ` +
      `${withheld.toLocaleString()} without content.`,
    /** How often a path changed, and whether the newest Snapshot no longer holds it. */
    pathDetail: (count: number, exists: boolean) =>
      `${count === 1 ? 'changed once' : `changed ${count.toLocaleString()} times`}${exists ? '' : ', gone now'}`,
    /** One Root's part of a content search over several, named by its directory. */
    rootProgress: (root: string, done: number, total: number) =>
      `${root}: ${done.toLocaleString()} of ${total.toLocaleString()} versions`,
    /** Where a match is: its line, and when the version holding it was first captured. */
    match: (line: number, since: Date) => `line ${line.toLocaleString()}, since ${when(since)}`,
    /** Said aloud when picking a result switched the Root picker. */
    switchedRoot: (root: string) => `Switched to ${root}`,
    /** Said when a picked result is in a Root no longer registered, so there is nowhere to go. */
    rootGone: 'That result is in a Root that is no longer registered.',
  },

  /** The mark and the accessible name of each kind of change in the file list and the tree. */
  changes: {
    added: { mark: '+', label: 'added' },
    modified: { mark: '~', label: 'modified' },
    deleted: { mark: '−', label: 'deleted' },
    renamed: { mark: '→', label: 'renamed' },
    /** A file chosen that the selected Snapshot did not change (ticket 15). */
    unchanged: { mark: '·', label: 'unchanged here' },
  } satisfies Record<FileChange | 'unchanged', { mark: string; label: string }>,
}
