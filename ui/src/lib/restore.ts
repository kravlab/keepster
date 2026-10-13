/**
 * Restore's preflight, as a plain module (ticket 65): the daemon's plan turned into the questions the
 * dialog asks, the answers turned back into a request, and whether the request may go.
 *
 * The daemon never asks, it refuses (ticket 47): a dry run answers with what would happen to every file,
 * and a file that needs something names it in `issues` — a confirmation, which is request-wide by kind; a
 * resolution, which is for one path; or nothing at all, when the file cannot be restored and can only be
 * skipped. The dialog builds its questions from a dry run made without answers, so a question stays on
 * screen once answered; the real request then carries the answers, and its refusal, if the Working Tree
 * changed meanwhile, is a plan of the same shape — made with the answers, so it no longer names what they
 * settled, and the questions are asked afresh from a dry run without them.
 */
import type { Confirmation, PathResolution, RestoreIssue, RestoreOutcome, RestoreRequest, RestoredPath } from './wire'

/** One question the dialog asks before a Restore may go. */
export type Question =
  /** A structural change, agreed to once for every path that needs it. */
  | { kind: 'confirm'; confirm: Confirmation; paths: string[] }
  /** Something only the user can settle for this one path. */
  | { kind: 'resolution'; resolution: NonNullable<RestoreIssue['resolution']>; path: string; issue: RestoreIssue }
  /** Files nothing settles: the Restore goes only if they are skipped. */
  | { kind: 'unrestorable'; paths: string[] }

/** What the user has answered so far. */
export interface Answers {
  confirm: Confirmation[]
  /** Keyed by the Snapshot path each answer is for. */
  resolutions: Record<string, PathResolution>
  skipUnresolvable: boolean
}

/** No answers yet. */
export function emptyAnswers(): Answers {
  return { confirm: [], resolutions: {}, skipUnresolvable: false }
}

/** Every question a plan raises, confirmations first in the order the plan first needs them. */
export function questions(plan: readonly RestoredPath[]): Question[] {
  const confirms = new Map<Confirmation, string[]>()
  const resolutions: Question[] = []
  const unrestorable: string[] = []

  for (const file of plan) {
    if (file.outcome !== 'blocked') continue
    for (const issue of file.issues ?? []) {
      if (issue.confirm) {
        const paths = confirms.get(issue.confirm) ?? []
        if (!paths.includes(file.path)) paths.push(file.path)
        confirms.set(issue.confirm, paths)
      } else if (issue.resolution) {
        resolutions.push({ kind: 'resolution', resolution: issue.resolution, path: file.path, issue })
      } else if (!unrestorable.includes(file.path)) {
        unrestorable.push(file.path)
      }
    }
  }

  const asked: Question[] = [...confirms].map(([confirm, paths]) => ({ kind: 'confirm', confirm, paths }))
  asked.push(...resolutions)
  if (unrestorable.length > 0) asked.push({ kind: 'unrestorable', paths: unrestorable })
  return asked
}

/** The answer given to one path's resolution, if it is one; a blank new name is none. */
function resolved(question: Extract<Question, { kind: 'resolution' }>, answers: Answers): PathResolution | null {
  const answer = answers.resolutions[question.path]
  switch (question.resolution) {
    case 'write':
      return answer?.write ? { write: true } : null
    case 'rename_to': {
      const name = answer?.rename_to?.trim()
      return name ? { rename_to: name } : null
    }
    case 'name_case':
      return answer?.name_case ? { name_case: answer.name_case } : null
    case 'modes':
      return answer?.modes ? { modes: answer.modes } : null
  }
}

/** Whether a question is settled by the answers given. */
function settled(question: Question, answers: Answers): boolean {
  switch (question.kind) {
    case 'confirm':
      return answers.confirm.includes(question.confirm)
    case 'unrestorable':
      return answers.skipUnresolvable
    case 'resolution':
      // Of paths that are one file, writing any one settles them all: the daemon skips the others.
      if (question.resolution === 'write') {
        return [question.path, ...(question.issue.paths ?? [])].some((path) => answers.resolutions[path]?.write)
      }
      return resolved(question, answers) !== null
  }
}

/**
 * Whether the Restore may go: every question settled, and something left to write once what cannot be
 * restored is skipped.
 */
export function ready(plan: readonly RestoredPath[], answers: Answers): boolean {
  const asked = questions(plan)
  if (!asked.every((question) => settled(question, answers))) return false
  const skipped = new Set(asked.flatMap((question) => (question.kind === 'unrestorable' ? question.paths : [])))
  return plan.some((file) => !skipped.has(file.path))
}

/**
 * The request for `paths` of Snapshot `from`, carrying only the answers the plan's questions asked for —
 * an answer to a question not on screen is not one the user gave.
 */
export function restoreRequest(
  from: string,
  paths: readonly string[],
  plan: readonly RestoredPath[],
  answers: Answers,
  dryRun: boolean,
): RestoreRequest {
  const asked = questions(plan)
  const confirm = asked.flatMap((question) =>
    question.kind === 'confirm' && answers.confirm.includes(question.confirm) ? [question.confirm] : [],
  )
  const resolutions: Record<string, PathResolution> = {}
  for (const question of asked) {
    if (question.kind !== 'resolution') continue
    const answer = resolved(question, answers)
    if (answer) resolutions[question.path] = answer
  }

  return {
    from,
    paths: [...paths],
    dry_run: dryRun,
    confirm,
    resolutions,
    // Never a standing order: a file that became unrestorable after the plan was shown is not skipped
    // unseen, it refuses the request and is shown.
    skip_unresolvable: answers.skipUnresolvable && asked.some((question) => question.kind === 'unrestorable'),
  }
}

/** How many files ended in each outcome, leaving out the outcomes none did. */
export function outcomeCounts(paths: readonly RestoredPath[]): Partial<Record<RestoreOutcome, number>> {
  const counts: Partial<Record<RestoreOutcome, number>> = {}
  for (const file of paths) counts[file.outcome] = (counts[file.outcome] ?? 0) + 1
  return counts
}

/**
 * Why the Safety Snapshot could not hold the content some files would lose, each reason once, as the
 * daemon's codes: what the `overwrite_unrecoverable` confirmation has to say beside its checkbox.
 */
export function unrecoverableReasons(plan: readonly RestoredPath[]): string[] {
  const reasons = plan
    .flatMap((file) => file.issues ?? [])
    .flatMap((issue) => (issue.confirm === 'overwrite_unrecoverable' && issue.why ? [issue.why] : []))
  return [...new Set(reasons)]
}
