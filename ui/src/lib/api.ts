/**
 * The SPA's client for the daemon's `/v1`, served from the same origin as the page.
 *
 * The session is the `keepster-<port>` cookie `keepster ui` set at bootstrap; the browser sends it with
 * every same-origin request. It dies with the daemon, so a 401 is reported as `SessionEnded`: only a new
 * `keepster ui` gets back in. Every other failure carries the daemon's own explanation (its problem body).
 */
import type { Diff, DiffFile, Entries, Entry, Problem, Root, Timeline } from './wire'

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

/** GETs `target`, turning a failure into `SessionEnded` or `ApiError`. */
async function get(target: string): Promise<Response> {
  const response = await fetch(target)
  if (response.ok) return response
  if (response.status === 401) throw new SessionEnded('the session has ended')
  const problem = (response.headers.get('content-type') ?? '').includes('json')
    ? ((await response.json()) as Partial<Problem>)
    : {}
  throw new ApiError(response.status, problem.detail, problem.code)
}

/** GETs `target` and parses its JSON body as `T`, which the caller names from the endpoint's contract. */
async function getJson<T>(target: string): Promise<T> {
  return (await get(target)).json() as Promise<T>
}

/** Every registered Root. */
export function listRoots(): Promise<Root[]> {
  return getJson(url(['roots']))
}

/** One page of a Root's timeline, newest first; `cursor` is the previous page's `next_cursor`. */
export function timeline(root: string, cursor: string | undefined): Promise<Timeline> {
  return getJson(url(['roots', root, 'snapshots'], { limit: String(PAGE_SIZE), cursor }))
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

/**
 * A file's content in a Snapshot, as text. Decoded the way the daemon renders hunk lines, so the two
 * agree line for line: bytes that are not UTF-8 become replacement characters, and a BOM is kept as the
 * character it is rather than swallowed.
 */
export async function fileContent(root: string, snapshot: string, path: string): Promise<string> {
  const bytes = await (await get(url(['roots', root, 'snapshots', snapshot, 'content'], { path }))).arrayBuffer()
  return new TextDecoder('utf-8', { ignoreBOM: true }).decode(bytes)
}
