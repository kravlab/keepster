/**
 * The notice that store maintenance is unavailable (ticket 11), as the "added" banner shows it once a
 * directory is added: the lines `keepster watch` writes for the daemon's `git`, in its order, built here
 * so the banner only draws them. It mirrors the CLI's `write_git`, without the part about the shell's own
 * `git`: a page cannot know what a shell runs.
 */
import { strings } from './strings'
import type { GitStatus } from './wire'

/** The notice's lines; each one the CLI would leave out is `null`. */
export interface MaintenanceNotice {
  title: string
  /** Which `git` the daemon found: `git <version> at <path>`, or `git <path>` without a version. */
  git: string | null
  /** Why maintenance cannot run, and what that costs; only with a reason. */
  why: string | null
  /** What running `git` said, word for word; only with a reason. */
  gitSaid: string | null
  /** How to install one: only where a newer `git` is what is missing, never after a failed Pass. */
  install: { command: string | null; note: string | null; link: string } | null
}

/** The notice for the daemon's `git` while store maintenance is unavailable. */
export function maintenanceNotice(git: GitStatus): MaintenanceNotice {
  const reason = git.reason ?? null
  return {
    title: strings.maintenance.title,
    git: git.path
      ? git.version
        ? strings.maintenance.gitAt(git.version, git.path)
        : strings.maintenance.gitPath(git.path)
      : null,
    why: reason === null ? null : why(reason, git.minimum),
    gitSaid: reason !== null && git.detail ? strings.maintenance.gitSaid(git.detail) : null,
    install:
      reason !== null && reason !== 'failed' && git.install
        ? { command: git.install.command ?? null, note: git.install.note ?? null, link: git.install.link }
        : null,
  }
}

/** Why maintenance cannot run for `reason`, and what it costs; an unknown reason is shown as it is. */
function why(reason: string, minimum: string): string {
  if (reason === 'failed') return strings.maintenance.failed
  const cause =
    reason === 'too_old' ? strings.maintenance.tooOld(minimum) : (strings.maintenance.reasons[reason] ?? reason)
  return strings.maintenance.unreclaimed(cause, minimum)
}
