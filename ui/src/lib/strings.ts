/**
 * Every text the SPA shows, in one place. The project has no i18n system; keeping the strings out of the
 * components is what lets one be added without touching them.
 */
import { TOO_LARGE_BYTES, type Card } from './files'
import type { FileChange } from './wire'

export const strings = {
  appName: 'Keepster',
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

  railLabel: 'Snapshots',
  noSnapshots: 'This Root has no Snapshots yet.',
  loadOlder: 'Load older',
  changed: (count: number) => `${count} changed`,
  notCounted: 'not counted yet',
  pinned: 'Pinned',
  incomplete: 'Incomplete',
  /** The rail's mark on an incomplete Snapshot; `incomplete` is its accessible name. */
  incompleteMark: '!',

  filesLabel: 'Changed files',
  incompleteBanner: 'Incomplete: some files could not be read when this Snapshot was taken.',
  firstSnapshot: 'The first Snapshot of this Root: everything in it is new.',
  noChanges: 'Nothing changed in this Snapshot.',

  selectFile: 'Select a file to see what changed.',
  wrapLines: 'Wrap lines',
  renamedFrom: (path: string) => `renamed from ${path}`,
  highlightOff: (limit: number) =>
    `Syntax highlighting is off: a line is longer than ${limit.toLocaleString()} characters.`,

  /** The text of the card shown in place of the editor. */
  cards: {
    binary: 'Binary file: there is no text to compare.',
    too_large: `Larger than ${TOO_LARGE_BYTES / (1024 * 1024)} MiB: too large to compare.`,
    withheld: 'Withheld: this Snapshot holds no content for it.',
    omitted: 'Could not be read when this Snapshot was taken.',
    symlink: 'A symbolic link: there is no file content to show.',
  } satisfies Record<Card, string>,

  /** The mark and the accessible name of each kind of change in the file list. */
  changes: {
    added: { mark: '+', label: 'added' },
    modified: { mark: '~', label: 'modified' },
    deleted: { mark: '−', label: 'deleted' },
    renamed: { mark: '→', label: 'renamed' },
  } satisfies Record<FileChange, { mark: string; label: string }>,
}
