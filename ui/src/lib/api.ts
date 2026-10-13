/**
 * The SPA's client for the daemon's `/v1`, served from the same origin as the page.
 *
 * The session is the `keepster-<port>` cookie `keepster ui` set at bootstrap; the browser sends it with
 * every same-origin request. It dies with the daemon, so a 401 is reported as `SessionEnded`: only a new
 * `keepster ui` gets back in. Every other failure carries the daemon's own explanation (its problem body).
 */
import type {
  Diff,
  DiffFile,
  Entries,
  Entry,
  Problem,
  RestoreRequest,
  Restored,
  RestoredPath,
  Root,
  Timeline,
} from './wire'
import { momentOf } from './ids'
import { dirName } from './paths'

/** How many Snapshots one timeline page asks for: the daemon's default page size. */
const PAGE_SIZE = 100

/** The daemon no longer knows this browser's session: it restarted, or the cookie is gone. */
export class SessionEnded extends Error {
  override name = 'SessionEnded'
}

/** The daemon refused or failed a request; `detail` is its own explanation, when it gave one. */
export class ApiError extends Error {
  override name = 'ApiError'

  constructor(
    readonly status: number,
    readonly detail?: string,
    readonly code?: string,
  ) {
    super(detail ?? `HTTP ${status}`)
  }
}

/** The daemon answered, but not in the shape `/v1` promises. */
export class UnexpectedAnswer extends Error {
  override name = 'UnexpectedAnswer'
}

/** A `/v1` path with its segments escaped and its query, leaving out absent parameters. */
function url(segments: readonly string[], query: Record<string, string | undefined> = {}): string {
  const path = `/v1/${segments.map(encodeURIComponent).join('/')}`
  const params = new URLSearchParams()
  for (const [key, value] of Object.entries(query)) if (value !== undefined) params.set(key, value)
  const search = params.toString()
  return search ? `${path}?${search}` : path
}

/** The problem body of a failed answer, or nothing when it carries none. */
async function problemOf(response: Response): Promise<Partial<Problem>> {
  return (response.headers.get('content-type') ?? '').includes('json')
    ? ((await response.json()) as Partial<Problem>)
    : {}
}

/** The failure an answer that is not OK stands for: `SessionEnded` for a 401, `ApiError` otherwise. */
function failure(response: Response, problem: Partial<Problem>): Error {
  if (response.status === 401) return new SessionEnded('the session has ended')
  return new ApiError(response.status, problem.detail, problem.code)
}

/** GETs `target`, turning a failure into `SessionEnded` or `ApiError`. */
async function get(target: string): Promise<Response> {
  const response = await fetch(target)
  if (response.ok) return response
  throw failure(response, await problemOf(response))
}

/** GETs `target` and parses its JSON body as `T`, which the caller names from the endpoint's contract. */
async function getJson<T>(target: string): Promise<T> {
  return (await get(target)).json() as Promise<T>
}

/** Every registered Root. */
export function listRoots(): Promise<Root[]> {
  return getJson(url(['roots']))
}

/**
 * One page of a Root's timeline, newest first; `cursor` is the previous page's `next_cursor`. With `path`,
 * only the Snapshots that changed that path, each saying how in `change`.
 */
export function timeline(root: string, cursor: string | undefined, path?: string): Promise<Timeline> {
  return getJson(url(['roots', root, 'snapshots'], { limit: String(PAGE_SIZE), cursor, path }))
}

/** The newest Snapshot that changed `path`, or `null` when none did. */
export async function newestTouching(root: string, path: string): Promise<string | null> {
  const page = await getJson<Timeline>(url(['roots', root, 'snapshots'], { limit: '1', path }))
  return page.snapshots[0]?.id ?? null
}

/**
 * How many Snapshots one millisecond can hold, plus the one before them. The rate limit keeps a Root's
 * Snapshots apart, but a stopping daemon's flush ignores it, so a second Snapshot can share a millisecond.
 */
const AROUND_A_MOMENT = 3

/**
 * The Snapshot just before `snapshot` in the Root's history, or `null` for its first: the newest one
 * older than it among those taken at or before its moment (`before` includes the moment). Snapshot ids
 * order as the Snapshots were taken, within a millisecond too.
 */
export async function previousSnapshot(root: string, snapshot: string): Promise<string | null> {
  const before = momentOf(snapshot).toISOString()
  const page = await getJson<Timeline>(url(['roots', root, 'snapshots'], { limit: String(AROUND_A_MOMENT), before }))
  return page.snapshots.find((s) => s.id < snapshot)?.id ?? null
}

/** The files changed from Snapshot `from` to Snapshot `to`, in path order, without hunks. */
export async function changedFiles(root: string, from: string, to: string): Promise<DiffFile[]> {
  return (await getJson<Diff>(url(['roots', root, 'diff'], { from, to }))).files
}

/** One file's diff from Snapshot `from` to Snapshot `to`, with its hunks. */
export async function fileDiff(root: string, from: string, to: string, path: string): Promise<DiffFile> {
  const [file] = (await getJson<Diff>(url(['roots', root, 'diff'], { from, to, path }))).files
  if (!file) throw new UnexpectedAnswer(`the diff of ${path} names no file`)
  return file
}

/** Every path a Snapshot holds, directories included. */
export async function snapshotEntries(root: string, snapshot: string): Promise<Entry[]> {
  return (await getJson<Entries>(url(['roots', root, 'snapshots', snapshot, 'entries'], { depth: 'infinity' }))).entries
}

/** What a Snapshot holds directly in directory `dir` (`''` for the Root's top), one level deep. */
export async function directoryEntries(root: string, snapshot: string, dir: string): Promise<Entry[]> {
  const path = dir === '' ? undefined : dir
  return (await getJson<Entries>(url(['roots', root, 'snapshots', snapshot, 'entries'], { path }))).entries
}

/** What a Snapshot holds at `path`, read from its directory's listing; `null` when it holds nothing there. */
export async function entryAt(root: string, snapshot: string, path: string): Promise<Entry | null> {
  return (await directoryEntries(root, snapshot, dirName(path))).find((entry) => entry.path === path) ?? null
}

/**
 * A file's content in a Snapshot, as text. Decoded the way the daemon renders hunk lines, so the two
 * agree line for line: bytes that are not UTF-8 become replacement characters, and a BOM is kept as the
 * character it is rather than swallowed.
 */
export async function fileContent(root: string, snapshot: string, path: string): Promise<string> {
  const bytes = await (await get(url(['roots', root, 'snapshots', snapshot, 'content'], { path }))).arrayBuffer()
  return new TextDecoder('utf-8', { ignoreBOM: true }).decode(bytes)
}

/**
 * What a Restore request came to: done (or, for a dry run, planned), or refused for want of answers,
 * with the plan saying what each file needs. Any other refusal is thrown, as everywhere.
 */
export type RestoreAnswer = { kind: 'done'; restored: Restored } | { kind: 'blocked'; paths: RestoredPath[] }

/** Puts a Snapshot's versions back over the Working Tree, or, with `dry_run`, says what that would do. */
export async function restore(root: string, request: RestoreRequest): Promise<RestoreAnswer> {
  const response = await fetch(url(['roots', root, 'restore']), {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(request),
  })
  if (response.ok) return { kind: 'done', restored: (await response.json()) as Restored }
  const problem = await problemOf(response)
  if (response.status === 400 && problem.code === 'confirmation_required' && problem.paths) {
    return { kind: 'blocked', paths: problem.paths }
  }
  throw failure(response, problem)
}
