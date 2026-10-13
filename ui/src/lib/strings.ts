/**
 * Every text the SPA shows, in one place. The project has no i18n system; keeping the strings out of the
 * components is what lets one be added without touching them.
 */
import { TOO_LARGE_BYTES, type Card } from './files'
import type { Theme } from './preferences'
import type { ChangeShape } from './summary'
import type { Confirmation, FileChange, ModesChoice, NameCaseChoice, RestoreOutcome } from './wire'

/** A moment as the page writes it out: date and time, where the user is. */
const when = (moment: Date) => moment.toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' })
/** Permission bits as `ls` users read them: four octal digits. */
const octal = (mode: number) => mode.toString(8).padStart(4, '0')
/** Paths, names or counts one after another. */
const list = (items: readonly string[]) => items.join(', ')
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

export const strings = {
  appName: 'Keepster',
  /** A moment written out in full: date and time, where the user is. */
  at: when,
  list,
  loading: 'Loading…',
  failed: 'Something went wrong:',

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
    cancel: 'Cancel',
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
    cancel: 'Cancel',
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
