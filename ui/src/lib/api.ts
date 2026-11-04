/**
 * The SPA's client for the daemon's `/v1`, served from the same origin as the page.
 *
 * The session is the `keepster-<port>` cookie `keepster ui` set at bootstrap; the browser sends it with
 * every same-origin request. It dies with the daemon, so a 401 is reported as `SessionEnded`: only a new
 * `keepster ui` gets back in. Every other failure carries the daemon's own explanation (its problem body).
 */
import type {
  ContentRecord,
  Diff,
  DiffFile,
  DirectoryListing,
  Entries,
  Entry,
  PathHit,
  PathMode,
  PathResolution,
  PathSearch,
  Preferences,
  PreviewGroup,
  PreviewRecord,
  PreviewSize,
  Problem,
  RegisterRoot,
  RestoreIssue,
  RestoreRequest,
  Restored,
  RestoredPath,
  Root,
  Status,
  Timeline,
} from './wire'
import { momentOf } from './ids'
import { ndjsonRecords } from './ndjson'
import { dirName, exactPath, exactPaths, pathQuery, shownPath, wirePath, wirePaths } from './paths'

/** How many Snapshots one timeline page asks for: the daemon's default page size. */
const PAGE_SIZE = 100

/** The daemon no longer knows this browser's session: it restarted, or the cookie is gone. */
export class SessionEnded extends Error {
  override name = 'SessionEnded'
}

/**
 * The daemon refused or failed a request; `detail` is its own explanation, when it gave one, and `details`
 * its machine-readable particulars, such as the `root` a `root_overlaps` refusal names.
 */
export class ApiError extends Error {
  override name = 'ApiError'

  constructor(
    readonly status: number,
    readonly detail?: string,
    readonly code?: string,
    readonly details?: Record<string, string>,
  ) {
    super(detail ?? `HTTP ${status}`)
  }
}

/** The daemon answered, but not in the shape `/v1` promises. */
export class UnexpectedAnswer extends Error {
  override name = 'UnexpectedAnswer'
}

/**
 * A `/v1` path with its segments escaped and its query, leaving out absent parameters. A `path` — Root-
 * relative, or an absolute path on the daemon's machine for `/v1/fs` — goes last, as `pathQuery` encodes
 * its bytes: `URLSearchParams` takes text, and would spell a byte that is not UTF-8 as U+FFFD (ticket 74).
 */
function url(segments: readonly string[], query: Record<string, string | undefined> = {}, path?: string): string {
  const target = `/v1/${segments.map(encodeURIComponent).join('/')}`
  const params = new URLSearchParams()
  for (const [key, value] of Object.entries(query)) if (value !== undefined) params.set(key, value)
  const search = [params.toString(), path === undefined ? '' : `path=${pathQuery(path)}`].filter(Boolean).join('&')
  return search ? `${target}?${search}` : target
}

/*
 * The wire's paths, each with its `_bytes` companion folded in, as `paths.ts` holds a path: every answer
 * naming a path passes through one of these on its way in, and a request on its way out.
 */

/** An optional path field and its companion: absent or `null` as it came, the exact path otherwise. */
function optionalPath(text: string | null | undefined, bytes: string | null | undefined): string | null | undefined {
  return text === undefined || text === null ? text : exactPath(text, bytes)
}

/** One path a Snapshot holds, its path exact. */
function entryIn(entry: Entry): Entry {
  return { ...entry, path: exactPath(entry.path, entry.path_bytes) }
}

/** One file of a diff, its paths exact. */
function diffFileIn(file: DiffFile): DiffFile {
  return {
    ...file,
    path: exactPath(file.path, file.path_bytes),
    old_path: optionalPath(file.old_path, file.old_path_bytes),
  }
}

/** One reason a file of a Restore is not simply written, its paths exact. */
function issueIn(issue: RestoreIssue): RestoreIssue {
  return {
    ...issue,
    at: optionalPath(issue.at, issue.at_bytes),
    existing: optionalPath(issue.existing, issue.existing_bytes),
    paths: issue.paths === undefined ? undefined : exactPaths(issue.paths, issue.paths_bytes),
  }
}

/** One file of a Restore, its paths exact. */
function restoredPathIn(file: RestoredPath): RestoredPath {
  return {
    ...file,
    path: exactPath(file.path, file.path_bytes),
    target: optionalPath(file.target, file.target_bytes),
    issues: file.issues?.map(issueIn),
  }
}

/** One path a path search found, its path exact. */
function hitIn(hit: PathHit): PathHit {
  return { ...hit, path: exactPath(hit.path, hit.path_bytes) }
}

/** One record of a content search, its path exact. */
function contentRecordIn(record: ContentRecord): ContentRecord {
  return record.type === 'match' || record.type === 'withheld'
    ? { ...record, path: exactPath(record.path, record.path_bytes) }
    : record
}

/** One of a preview's largest files, its path exact. */
function sizeIn(size: PreviewSize): PreviewSize {
  return { path: exactPath(size.path, size.path_bytes), bytes: size.bytes }
}

/** What one rule decided in a preview, the paths it pruned exact. */
function groupIn(group: PreviewGroup): PreviewGroup {
  return group.pruned === undefined ? group : { ...group, pruned: exactPaths(group.pruned, group.pruned_bytes) }
}

/**
 * One line of a Capture Preview, its paths exact; `null` for a record of a type this page does not know,
 * which the contract says to pass over.
 */
function previewRecordIn(record: PreviewRecord): PreviewRecord | null {
  switch (record.type) {
    case 'summary':
      return {
        ...record,
        groups: record.groups.map(groupIn),
        largest_captured: record.largest_captured.map(sizeIn),
        largest_left_out: record.largest_left_out.map(sizeIn),
      }
    case 'header':
    case 'progress':
    case 'error':
      return record
    default:
      return null
  }
}

/** A Restore request as the wire takes it: each path that is not UTF-8 named by its bytes as well. */
function restoreRequestOut(request: RestoreRequest): RestoreRequest {
  const paths = wirePaths(request.paths)
  const resolutions: Record<string, PathResolution> = {}
  const resolutionsBytes: Record<string, PathResolution> = {}
  for (const [path, answer] of Object.entries(request.resolutions)) {
    const wire = wirePath(path)
    if (wire.bytes === undefined) resolutions[path] = answer
    else resolutionsBytes[wire.bytes] = answer
  }
  return {
    ...request,
    paths: paths.texts,
    ...(paths.bytes && { paths_bytes: paths.bytes }),
    resolutions,
    ...(Object.keys(resolutionsBytes).length > 0 && { resolutions_bytes: resolutionsBytes }),
  }
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
  return new ApiError(response.status, problem.detail, problem.code, problem.details)
}

/** GETs `target`, turning a failure into `SessionEnded` or `ApiError`; `signal` aborts the request. */
async function get(target: string, signal?: AbortSignal): Promise<Response> {
  const response = await fetch(target, signal ? { signal } : undefined)
  if (response.ok) return response
  throw failure(response, await problemOf(response))
}

/** GETs `target` and parses its JSON body as `T`, which the caller names from the endpoint's contract. */
async function getJson<T>(target: string, signal?: AbortSignal): Promise<T> {
  return (await get(target, signal)).json() as Promise<T>
}

/** Sends `body` as JSON to `target` with `method`, and hands back the answer whatever its status. */
function sendJson(method: 'POST' | 'PUT', target: string, body: unknown): Promise<Response> {
  return fetch(target, { method, headers: { 'content-type': 'application/json' }, body: JSON.stringify(body) })
}

/** Every registered Root. */
export function listRoots(): Promise<Root[]> {
  return getJson(url(['roots']))
}

/**
 * The directories in the absolute directory `path` on the daemon's machine, or in its home directory
 * when `path` is absent. A path that is not absolute, names nothing, is not a directory or cannot be
 * listed is refused as an `ApiError` with the daemon's explanation.
 */
export function listDirectories(path?: string): Promise<DirectoryListing> {
  return getJson(url(['fs'], {}, path))
}

/**
 * One page of a Root's timeline, newest first; `cursor` is the previous page's `next_cursor`. With `path`,
 * only the Snapshots that changed that path, each saying how in `change`.
 */
export function timeline(root: string, cursor: string | undefined, path?: string): Promise<Timeline> {
  return getJson(url(['roots', root, 'snapshots'], { limit: String(PAGE_SIZE), cursor }, path))
}

/** The newest Snapshot that changed `path`, or `null` when none did. */
export async function newestTouching(root: string, path: string): Promise<string | null> {
  const page = await getJson<Timeline>(url(['roots', root, 'snapshots'], { limit: '1' }, path))
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
  return (await getJson<Diff>(url(['roots', root, 'diff'], { from, to }))).files.map(diffFileIn)
}

/** One file's diff from Snapshot `from` to Snapshot `to`, with its hunks. */
export async function fileDiff(root: string, from: string, to: string, path: string): Promise<DiffFile> {
  const [file] = (await getJson<Diff>(url(['roots', root, 'diff'], { from, to }, path))).files
  if (!file) throw new UnexpectedAnswer(`the diff of ${shownPath(path)} names no file`)
  return diffFileIn(file)
}

/** Every path a Snapshot holds, directories included. */
export async function snapshotEntries(root: string, snapshot: string): Promise<Entry[]> {
  return (
    await getJson<Entries>(url(['roots', root, 'snapshots', snapshot, 'entries'], { depth: 'infinity' }))
  ).entries.map(entryIn)
}

/** What a Snapshot holds directly in directory `dir` (`''` for the Root's top), one level deep. */
export async function directoryEntries(root: string, snapshot: string, dir: string): Promise<Entry[]> {
  const path = dir === '' ? undefined : dir
  return (await getJson<Entries>(url(['roots', root, 'snapshots', snapshot, 'entries'], {}, path))).entries.map(entryIn)
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
  const bytes = await (await get(url(['roots', root, 'snapshots', snapshot, 'content'], {}, path))).arrayBuffer()
  return new TextDecoder('utf-8', { ignoreBOM: true }).decode(bytes)
}

/**
 * What a Restore request came to: done (or, for a dry run, planned), or refused for want of answers,
 * with the plan saying what each file needs. Any other refusal is thrown, as everywhere.
 */
export type RestoreAnswer = { kind: 'done'; restored: Restored } | { kind: 'blocked'; paths: RestoredPath[] }

/** Puts a Snapshot's versions back over the Working Tree, or, with `dry_run`, says what that would do. */
export async function restore(root: string, request: RestoreRequest): Promise<RestoreAnswer> {
  const response = await sendJson('POST', url(['roots', root, 'restore']), restoreRequestOut(request))
  if (response.ok) {
    const restored = (await response.json()) as Restored
    return { kind: 'done', restored: { ...restored, paths: restored.paths.map(restoredPathIn) } }
  }
  const problem = await problemOf(response)
  if (response.status === 400 && problem.code === 'confirmation_required' && problem.paths) {
    return { kind: 'blocked', paths: problem.paths.map(restoredPathIn) }
  }
  throw failure(response, problem)
}

/**
 * What registering a directory came to: the Root it now is, or a refusal asking which a path through a
 * link is — `path` as it was sent and `realPath` where the link leads now — with nothing registered. Any
 * other refusal is thrown, as everywhere.
 */
export type RegisterAnswer =
  { kind: 'registered'; root: Root } | { kind: 'throughLink'; path: string; realPath: string }

/**
 * Registers the directory at `path` as a Watched Root. A path through a link is refused until `pathMode`
 * says which the Root is; that refusal is answered as `throughLink`, so the person can be asked.
 */
export async function registerRoot(path: string, pathMode?: PathMode): Promise<RegisterAnswer> {
  const body: RegisterRoot = pathMode === undefined ? { path } : { path, path_mode: pathMode }
  const response = await sendJson('POST', url(['roots']), body)
  if (response.ok) return { kind: 'registered', root: (await response.json()) as Root }
  const problem = await problemOf(response)
  const details = problem.details
  if (
    response.status === 400 &&
    problem.code === 'confirmation_required' &&
    details?.missing === 'path_mode' &&
    details.path !== undefined &&
    details.real_path !== undefined
  ) {
    return { kind: 'throughLink', path: details.path, realPath: details.real_path }
  }
  throw failure(response, problem)
}

/** The daemon's health surface; only its maintenance rows are read. */
export function readStatus(): Promise<Status> {
  return getJson(url(['status']))
}

/** The web UI's preferences, as the daemon keeps them. */
export function readPreferences(): Promise<Preferences> {
  return getJson(url(['preferences']))
}

/**
 * Replaces the web UI's preferences whole, and answers with what the daemon stored. A value it does not
 * take is refused as an `ApiError` with code `preference_invalid` and its explanation.
 */
export async function writePreferences(preferences: Preferences): Promise<Preferences> {
  const response = await sendJson('PUT', url(['preferences']), preferences)
  if (response.ok) return (await response.json()) as Preferences
  throw failure(response, await problemOf(response))
}

/** Every path any Snapshot of any Root changed that `q` finds, a fragment matched anywhere in the path. */
export async function searchPaths(q: string, signal?: AbortSignal): Promise<PathSearch> {
  const found = await getJson<PathSearch>(url(['search', 'paths'], { q }), signal)
  return { ...found, hits: found.hits.map(hitIn) }
}

/**
 * The records of a newline-delimited JSON answer, as they arrive. An answer with no body, or a line that
 * is not JSON, is an `UnexpectedAnswer` that says `what` sent it.
 */
async function* streamedRecords(response: Response, what: string, signal?: AbortSignal): AsyncGenerator<unknown> {
  if (!response.body) throw new UnexpectedAnswer(`${what} answered with no body`)
  try {
    yield* ndjsonRecords(response.body, signal)
  } catch (e) {
    if (e instanceof SyntaxError) throw new UnexpectedAnswer(`${what} sent a line that is not JSON: ${e.message}`)
    throw e
  }
}

/**
 * The records of a content search for `q` in the Roots named, or in every Root, as the daemon finds them.
 * Leaving the loop early, or aborting `signal`, closes the connection, which stops the daemon's walk.
 */
export async function* searchContent(
  q: string,
  scope: { roots: readonly string[] } | 'all',
  signal?: AbortSignal,
): AsyncGenerator<ContentRecord> {
  const query = scope === 'all' ? { q, all: 'true' } : { q, roots: scope.roots.join(',') }
  const response = await get(url(['search', 'content'], query), signal)
  for await (const record of streamedRecords(response, 'the content search', signal))
    yield contentRecordIn(record as ContentRecord)
}

/**
 * The records of a Capture Preview of the absolute directory `path` on the daemon's machine, at the
 * default depth, which sends the summary alone: a `header`, `progress` now and then while the walk runs,
 * then its `summary`, or an `error` in its place. A preview the daemon refuses — `preview_busy` while
 * another one walks — is thrown as an `ApiError` with its code. Leaving the loop early, or aborting
 * `signal`, closes the connection, which stops the daemon's walk.
 */
export async function* previewDirectory(path: string, signal?: AbortSignal): AsyncGenerator<PreviewRecord> {
  const response = await get(url(['preview'], {}, path), signal)
  for await (const line of streamedRecords(response, 'the preview', signal)) {
    const record = previewRecordIn(line as PreviewRecord)
    if (record) yield record
  }
}
