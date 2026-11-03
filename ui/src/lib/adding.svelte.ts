/**
 * Adding a directory as a Watched Root (ticket 93), the Root management page's add flow, as a small state
 * machine the add dialog draws: `choosing` a directory, `registering` it, `askingLink` when the path leads
 * through a link, and `done` or `failed`. It mirrors `keepster watch`: the daemon never guesses which a
 * path through a link is, it refuses and names where the link leads, and the person's answer is sent with
 * the same path again.
 *
 * `add` is where a directory chosen enters the flow; the Capture Preview (ticket 94) goes between it and
 * registering, as further steps.
 */
import { ApiError, readStatus, registerRoot, SessionEnded } from './api'
import { strings } from './strings'
import type { GitStatus, PathMode, Root } from './wire'

/** Where the add flow is. */
export type AddStep =
  /** The person is choosing the directory. */
  | { kind: 'choosing' }
  /** `POST /v1/roots` is on its way with `path`. */
  | { kind: 'registering'; path: string }
  /** `path` leads through a link to `realPath`: the person says which the Root is. Nothing is registered. */
  | { kind: 'askingLink'; path: string; realPath: string }
  /** The daemon refused `path`, for `reason`; the person may try again or choose another. */
  | { kind: 'failed'; path: string; reason: string }
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

  /**
   * @param failure the text a failed request shows, as `failureText` gives it
   * @param rootById the live list's Root with an id, so that a refusal can name it by its path; without
   *   it, the daemon's reason is shown as it is
   */
  constructor(
    private readonly failure: (e: unknown) => string,
    private readonly rootById: (id: string) => Root | undefined = () => undefined,
  ) {}

  /**
   * Adds the directory at `path`, the one the person chose. A refusal — overlapping a Root, not a
   * directory, … — is `failed` with the daemon's reason, from which adding may start again.
   */
  add(path: string): Promise<void> {
    return this.#register(path)
  }

  /** Answers the link question: registers the same path again as the link or as the real directory. */
  async choose(mode: PathMode): Promise<void> {
    if (this.step.kind === 'askingLink') await this.#register(this.step.path, mode)
  }

  /** Declines the link question: nothing is registered, and the person chooses again. */
  cancel(): void {
    if (this.step.kind === 'askingLink') this.step = { kind: 'choosing' }
  }

  /**
   * Sends `POST /v1/roots` for `path`, with `mode` once the person has said which a link is. Nothing is
   * sent while a request is already on its way: a second click must not register twice.
   */
  async #register(path: string, mode?: PathMode): Promise<void> {
    if (this.step.kind === 'registering') return
    this.step = { kind: 'registering', path }
    try {
      const answer = await registerRoot(path, mode)
      if (answer.kind === 'throughLink') {
        this.step = { kind: 'askingLink', path, realPath: answer.realPath }
        return
      }
      this.step = { kind: 'done', root: answer.root, maintenance: null }
      void this.#noticeFor(answer.root)
    } catch (e) {
      this.step = { kind: 'failed', path, reason: this.#reason(path, e) }
    }
  }

  /**
   * Why registering `path` failed. The daemon names an overlapped Root by its id, which a person cannot
   * place; one in the live list is named by its path instead.
   */
  #reason(path: string, e: unknown): string {
    const overlapped = e instanceof ApiError && e.code === 'root_overlaps' ? e.details?.root : undefined
    const root = overlapped === undefined ? undefined : this.rootById(overlapped)
    return root ? strings.adding.overlaps(path, root.path) : this.failure(e)
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
