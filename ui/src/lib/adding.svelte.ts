/**
 * Adding a directory as a Watched Root (tickets 93 and 94), the Root management page's add flow, as a
 * small state machine the add dialog draws: `choosing` a directory, `previewing` what watching it would
 * capture, `askingClaim` when it is past what one Root is claimed to hold, `registering` it, `askingLink`
 * when the path leads through a link, and `done` or `failed`. It mirrors `keepster watch`: a directory is
 * never watched past the claim unseen, a preview that cannot be had does not stop registering, and the
 * daemon never guesses which a path through a link is — it refuses and names where the link leads, and
 * the person's answer is sent with the same path again.
 */
import { ApiError, previewDirectory, readStatus, registerRoot, SessionEnded } from './api'
import { type AskingLink, type PathRefused, sendRootPath } from './link'
import { strings } from './strings'
import type { GitStatus, PathMode, PreviewSummary, Root } from './wire'

/** The daemon's code for a preview refused because another one is walking. */
const PREVIEW_BUSY = 'preview_busy'

/** How a Capture Preview ended, as far as adding goes on from it. */
type PreviewOutcome =
  /** `path` is within what one Root is claimed to hold: registering goes on. */
  | { kind: 'within' }
  /** No summary could be had, which says nothing either way, as `keepster watch` has it: registering goes on. */
  | { kind: 'unavailable' }
  /** `path` is past what one Root is claimed to hold, as `summary` says: the person decides. */
  | { kind: 'past'; summary: PreviewSummary }
  /** Adding must not go on: the preview was stopped, refused as busy, or the session ended. */
  | { kind: 'stop' }

/** Where the add flow is. */
export type AddStep =
  /** The person is choosing the directory. */
  | { kind: 'choosing' }
  /** `GET /v1/preview` is walking `path`; `walked` is how many paths it has classified so far. */
  | { kind: 'previewing'; path: string; walked: number }
  /** `path` is past what one Root is claimed to hold, as `summary` says: the person decides. Nothing is registered. */
  | { kind: 'askingClaim'; path: string; summary: PreviewSummary }
  /** `POST /v1/roots` is on its way with `path`. */
  | { kind: 'registering'; path: string }
  /** `path` leads through a link: the person says which the Root is. Nothing is registered. */
  | AskingLink
  /** The daemon refused `path`; the person may try again or choose another. */
  | PathRefused
  /**
   * The directory is now `root`. `maintenance` is the daemon's `git` while store maintenance is
   * unavailable: `null` until the status answers, and after it unless maintenance is unavailable.
   */
  | { kind: 'done'; root: Root; maintenance: GitStatus | null }

/**
 * One add, from choosing a directory to its outcome: `step` is what the dialog and the banner draw. A
 * request on its way holds every further one back, and the outcome shows as soon as the daemon registers
 * the directory, the maintenance notice following when the status answers.
 */
export class AddingRoot {
  /** Where the flow is now. */
  step = $state<AddStep>({ kind: 'choosing' })

  /** Stops the preview walking now; `null` while none is. */
  #walk: AbortController | null = null

  /**
   * @param failure the text a failed request shows, as `failureText` gives it
   * @param rootById the live list's Root with an id, so that a refusal can name it by its path; one it
   *   does not find is named as the daemon names it
   */
  constructor(
    private readonly failure: (e: unknown) => string,
    private readonly rootById: (id: string) => Root | undefined,
  ) {}

  /**
   * Adds the directory at `path`, the one the person chose, once its Capture Preview says it is within
   * what one Root is claimed to hold; past it, the person is asked first (`askingClaim`). A refusal —
   * overlapping a Root, not a directory, another preview walking, … — is `failed` with its reason, from
   * which adding may start again. Nothing starts while a preview or a request is on its way.
   */
  async add(path: string): Promise<void> {
    if (this.step.kind === 'previewing' || this.step.kind === 'registering') return
    const outcome = await this.#preview(path)
    if (outcome.kind === 'stop') return
    if (outcome.kind === 'past') this.step = { kind: 'askingClaim', path, summary: outcome.summary }
    else await this.#register(path)
  }

  /** Answers the claim question with yes: registers the directory past the claim. */
  async watchAnyway(): Promise<void> {
    if (this.step.kind === 'askingClaim') await this.#register(this.step.path)
  }

  /** Answers the link question: registers the same path again as the link or as the real directory. */
  async choose(mode: PathMode): Promise<void> {
    if (this.step.kind === 'askingLink') await this.#register(this.step.path, mode)
  }

  /**
   * Backs out of the add before anything is registered: stops a preview walking, which closes its stream
   * and so the daemon's walk, or declines the claim or link question. The person chooses again.
   */
  cancel(): void {
    if (this.step.kind === 'previewing') this.#walk?.abort()
    if (this.step.kind === 'previewing' || this.step.kind === 'askingClaim' || this.step.kind === 'askingLink')
      this.step = { kind: 'choosing' }
  }

  /**
   * Runs the Capture Preview of `path`, showing its progress, and says how it ended: `past` with its
   * summary when `path` is past the claim; `within` when it is not; `unavailable` for a preview that
   * failed, which says nothing either way, as `keepster watch` has it; and `stop` when adding must not go
   * on: cancelled, refused as busy (registering would not notice another walk, and the directory would be
   * watched unseen), or the session ended.
   */
  async #preview(path: string): Promise<PreviewOutcome> {
    const walk = new AbortController()
    this.#walk = walk
    const previewing = (walked: number): AddStep => ({ kind: 'previewing', path, walked })
    this.step = previewing(0)
    let summary: PreviewSummary | null = null
    try {
      for await (const record of previewDirectory(path, walk.signal)) {
        if (record.type === 'progress') this.step = previewing(record.walked)
        else if (record.type === 'summary') summary = record
        else if (record.type === 'error') break
      }
    } catch (e) {
      if (walk.signal.aborted) return { kind: 'stop' }
      if (e instanceof ApiError && e.status === 409 && e.code === PREVIEW_BUSY) {
        this.step = { kind: 'failed', path, reason: strings.adding.preview.busy }
        return { kind: 'stop' }
      }
      if (e instanceof SessionEnded) {
        this.step = { kind: 'failed', path, reason: this.failure(e) }
        return { kind: 'stop' }
      }
    } finally {
      if (this.#walk === walk) this.#walk = null
    }
    if (walk.signal.aborted) return { kind: 'stop' }
    if (summary === null) return { kind: 'unavailable' }
    return summary.past_ceiling.length > 0 ? { kind: 'past', summary } : { kind: 'within' }
  }

  /**
   * Sends `POST /v1/roots` for `path`, with `mode` once the person has said which a link is. Nothing is
   * sent while a request is already on its way: a second click must not register twice.
   */
  async #register(path: string, mode?: PathMode): Promise<void> {
    if (this.step.kind === 'registering') return
    this.step = { kind: 'registering', path }
    const outcome = await sendRootPath(path, () => registerRoot(path, mode), this.failure, this.rootById)
    if (outcome.kind !== 'registered') {
      this.step = outcome
      return
    }
    this.step = { kind: 'done', root: outcome.root, maintenance: null }
    void this.#noticeFor(outcome.root)
  }

  /**
   * Fills the maintenance notice of the `done` step for `root` in once `GET /v1/status` answers. It is
   * not waited for: the directory is added whatever the status says, so the outcome shows at once.
   */
  async #noticeFor(root: Root): Promise<void> {
    const git = await gitIfUnmaintained(this.failure)
    if (git && this.step.kind === 'done' && this.step.root.id === root.id)
      this.step = { ...this.step, maintenance: git }
  }
}

/**
 * The daemon's `git` while store maintenance is unavailable, so that the history just started is known
 * never to be reclaimed until one is installed, as `keepster watch` says (ticket 11); `null` otherwise.
 * The directory is added whatever happens here: a status that cannot be read only leaves the notice out,
 * the way the CLI leaves it to `keepster status`. An ended session is the one failure handed to
 * `failure`, since it ends every screen (`failureText` passes it on); nobody awaits this to rethrow it.
 */
async function gitIfUnmaintained(failure: (e: unknown) => string): Promise<GitStatus | null> {
  try {
    const status = await readStatus()
    return status.daemon.maintenance === 'unavailable' ? status.daemon.git : null
  } catch (e) {
    if (e instanceof SessionEnded) failure(e)
    return null
  }
}
