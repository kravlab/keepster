/**
 * Recording a Move of one Root (#32), as the state the Move dialog draws, as `adding.svelte.ts` is for the
 * add dialog: `choosing` where the directory is now, `moving` while `PATCH /v1/roots/{id}` is on its way,
 * `askingLink` when the path leads through a link, and `done` or `failed`. It mirrors `keepster move`:
 * Keepster moves no file and infers no Move — the person says where the directory is now, the daemon
 * records it — and the daemon never guesses which a path through a link is: it refuses and names where
 * the link leads, and the person's answer is sent with the same path again.
 */
import { moveRoot } from './api'
import { type AskingLink, type PathRefused, sendRootPath } from './link'
import type { PathMode, Root } from './wire'

/** Where the Move is. */
export type MoveStep =
  /** The person is choosing where the directory is now. */
  | { kind: 'choosing' }
  /** `PATCH /v1/roots/{id}` is on its way with `path`. */
  | { kind: 'moving'; path: string }
  /** `path` leads through a link: the person says which the Root is. Nothing is recorded. */
  | AskingLink
  /** The daemon refused `path`; the person may try again or choose another. */
  | PathRefused
  /** The Move is recorded: the Root is now `root`, as the daemon describes it at its new path. */
  | { kind: 'done'; root: Root }

/**
 * One Root's Move, from the dialog opening to the Move recorded: `step` is what the dialog draws. A
 * refusal leaves the dialog as it was, to try again or back out; a request on its way holds every further
 * one back.
 */
export class MovingRoot {
  /** Where the Move is now. */
  step = $state<MoveStep>({ kind: 'choosing' })

  /**
   * @param root the Root to move, as the list shows it when the Move begins: its id is the one sent, and
   *   the dialog names it by it once the list has dropped it
   * @param failure the text a failed request shows, as `failureText` gives it; an ended session is
   *   handed to it too, since it ends every screen
   * @param rootById the live list's Root with an id, so that a refusal can name it by its path; one it
   *   does not find is named as the daemon names it
   */
  constructor(
    readonly root: Root,
    private readonly failure: (e: unknown) => string,
    private readonly rootById: (id: string) => Root | undefined,
  ) {}

  /**
   * Records that the Root's directory is now at `path`, the one the person chose, sending
   * `PATCH /v1/roots/{id}`, with `mode` once the person has said which a link is. A refusal — not a
   * directory, overlapping a Root, no such Root — is `failed` with its reason, from which the Move may be
   * tried again. Nothing is sent while a request is on its way: a second click must not send twice.
   */
  async move(path: string, mode?: PathMode): Promise<void> {
    if (this.step.kind === 'moving') return
    this.step = { kind: 'moving', path }
    const outcome = await sendRootPath(path, () => moveRoot(this.root.id, path, mode), this.failure, this.rootById)
    this.step = outcome.kind === 'moved' ? { kind: 'done', root: outcome.root } : outcome
  }

  /** Answers the link question: sends the same path again as the link or as the real directory. */
  async choose(mode: PathMode): Promise<void> {
    if (this.step.kind === 'askingLink') await this.move(this.step.path, mode)
  }

  /** Declines the link question before anything is recorded: the person chooses again. */
  cancel(): void {
    if (this.step.kind === 'askingLink') this.step = { kind: 'choosing' }
  }
}
