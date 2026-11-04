/**
 * What a failed request says on screen. The UI's own texts come from `strings`; text that is not ours,
 * the daemon's explanation of a refusal or the browser's of a network error, is shown as it is.
 */
import { ApiError, SessionEnded, UnexpectedAnswer } from './api'
import { HunkMismatch } from './mismatch'
import { strings } from './strings'
import type { Root } from './wire'

/** The text for failure `e`; a dead session is also handed to `onSessionEnded`, which replaces the screen. */
export function failureText(e: unknown, onSessionEnded: () => void): string {
  if (e instanceof SessionEnded) {
    onSessionEnded()
    return strings.sessionEnded.before
  }
  if (e instanceof ApiError) return e.detail ?? strings.httpStatus(e.status)
  if (e instanceof UnexpectedAnswer) return strings.unexpectedAnswer
  if (e instanceof HunkMismatch) return strings.hunkMismatch
  return e instanceof Error ? e.message : String(e)
}

/**
 * What a flow that sends a path for a Root — registering a directory or recording a Move — words a failed
 * request with. The two travel together, from the page that builds them once down to `pathRefusalText`.
 */
export interface RefusalWording {
  /** The text a failed request shows, as `failureText` gives it; an ended session is handed to it too. */
  failure: (e: unknown) => string
  /**
   * The live list's Root with an id, so that a refusal can name it by its path; one it does not find is
   * named as the daemon names it.
   */
  rootById: (id: string) => Root | undefined
}

/**
 * Why sending `path` for a Root — registering a directory or recording a Move — was refused, as
 * `wording.failure` words it. The daemon names an overlapped Root by its id, which a person cannot place;
 * one `wording.rootById` finds is named by its path instead.
 */
export function pathRefusalText(path: string, e: unknown, wording: RefusalWording): string {
  const overlapped = e instanceof ApiError && e.code === 'root_overlaps' ? e.details?.root : undefined
  const root = overlapped === undefined ? undefined : wording.rootById(overlapped)
  return root ? strings.roots.overlaps(path, root.path) : wording.failure(e)
}
