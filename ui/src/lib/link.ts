/**
 * Sending a path for a Root, the part registering a directory (`adding.svelte.ts`) and recording a Move
 * (`moving.svelte.ts`) share: the daemon never guesses which a path through a link is, so it refuses and
 * names where the link leads, and the flow asks the person, then sends the same path again with the
 * answer; any other refusal is shown with its reason. Each flow keeps its own step and holds a second
 * request back while one is on its way; this gives it the step the answer leads to.
 */
import type { ThroughLink } from './api'
import { pathRefusalText, type RefusalWording } from './failures'

/** `path` leads through a link to `realPath`: the person says which the Root is. Nothing is recorded. */
export type AskingLink = { kind: 'askingLink'; path: string; realPath: string }

/** The daemon refused `path`, for `reason`; the person may try again or choose another. */
export type PathRefused = { kind: 'failed'; path: string; reason: string }

/**
 * Sends `path` through `request` and says what it came to: the daemon's answer as it is, the link
 * question when the path leads through a link, or the refusal with its reason as `pathRefusalText` words
 * it with `wording`, naming an overlapped Root that it finds by its path. It never throws.
 */
export async function sendRootPath<Answer extends { kind: string }>(
  path: string,
  request: () => Promise<Answer | ThroughLink>,
  wording: RefusalWording,
): Promise<Answer | AskingLink | PathRefused> {
  try {
    const answer = await request()
    return isThroughLink(answer) ? { kind: 'askingLink', path, realPath: answer.realPath } : answer
  } catch (e) {
    return { kind: 'failed', path, reason: pathRefusalText(path, e, wording) }
  }
}

/** Whether `answer` is the refusal asking which a path through a link is. */
function isThroughLink(answer: { kind: string }): answer is ThroughLink {
  return answer.kind === 'throughLink'
}
