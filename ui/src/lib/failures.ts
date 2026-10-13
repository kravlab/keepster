/**
 * What a failed request says on screen. The UI's own texts come from `strings`; text that is not ours,
 * the daemon's explanation of a refusal or the browser's of a network error, is shown as it is.
 */
import { ApiError, SessionEnded, UnexpectedAnswer } from './api'
import { HunkMismatch } from './mismatch'
import { strings } from './strings'

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
