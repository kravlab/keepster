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
 * Why sending `path` for a Root — registering a directory or recording a Move — was refused, as `failure`
 * words it. The daemon names an overlapped Root by its id, which a person cannot place; one `rootById`
 * finds is named by its path instead.
 */
export function pathRefusalText(
  path: string,
  e: unknown,
  failure: (e: unknown) => string,
  rootById: (id: string) => Root | undefined,
): string {
  const overlapped = e instanceof ApiError && e.code === 'root_overlaps' ? e.details?.root : undefined
  const root = overlapped === undefined ? undefined : rootById(overlapped)
  return root ? strings.roots.overlaps(path, root.path) : failure(e)
}
