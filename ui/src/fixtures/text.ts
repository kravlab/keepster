// PROTOTYPE fixtures (ticket 09): TypeScript, TOML and Markdown.
// TOML is here on purpose: it is the one language in the curated set that neither engine supports
// first-class (CodeMirror uses a legacy stream mode, Monaco falls back to `ini`).

export const TS_BEFORE = `import type { Root, Snapshot, SnapshotId } from './model'

export interface TimelinePage {
  items: Snapshot[]
  next_cursor?: string
}

const V1 = '/v1'

export class KeepsterClient {
  constructor(private readonly base: string) {}

  async roots(): Promise<Root[]> {
    const res = await fetch(this.base + V1 + '/roots')
    if (!res.ok) throw new Error('roots failed: ' + res.status)
    return res.json()
  }

  async timeline(root: string, opts: { path?: string; limit?: number } = {}): Promise<TimelinePage> {
    const q = new URLSearchParams()
    if (opts.path) q.set('path', opts.path)
    if (opts.limit) q.set('limit', String(opts.limit))
    const res = await fetch(this.base + V1 + '/roots/' + root + '/snapshots?' + q)
    return res.json()
  }

  async content(root: string, snapshot: SnapshotId, path: string): Promise<string> {
    const q = new URLSearchParams({ path })
    const res = await fetch(this.base + V1 + '/roots/' + root + '/snapshots/' + snapshot + '/content?' + q)
    return res.text()
  }
}
`

export const TS_AFTER = `import type { Problem, Root, Snapshot, SnapshotId } from './model'

export interface TimelinePage {
  items: Snapshot[]
  next_cursor?: string
}

const V1 = '/v1'

/** Every non-2xx response is RFC 9457; the stable \`code\` is what callers switch on. */
export class ApiError extends Error {
  constructor(readonly problem: Problem, readonly status: number) {
    super(problem.title)
  }
}

export class KeepsterClient {
  constructor(
    private readonly base: string,
    private readonly signal?: AbortSignal,
  ) {}

  private async get(path: string): Promise<Response> {
    const res = await fetch(this.base + V1 + path, { signal: this.signal, credentials: 'include' })
    if (!res.ok) throw new ApiError(await res.json(), res.status)
    return res
  }

  async roots(): Promise<Root[]> {
    return (await this.get('/roots')).json()
  }

  async timeline(
    root: string,
    opts: { path?: string; limit?: number; cursor?: string } = {},
  ): Promise<TimelinePage> {
    const q = new URLSearchParams()
    if (opts.path) q.set('path', opts.path)
    if (opts.limit) q.set('limit', String(opts.limit))
    if (opts.cursor) q.set('cursor', opts.cursor)
    return (await this.get('/roots/' + root + '/snapshots?' + q)).json()
  }

  async content(root: string, snapshot: SnapshotId, path: string): Promise<string> {
    const q = new URLSearchParams({ path })
    return (await this.get('/roots/' + root + '/snapshots/' + snapshot + '/content?' + q)).text()
  }

  async diff(root: string, from: SnapshotId, to: SnapshotId | 'working', path?: string): Promise<string> {
    const q = new URLSearchParams({ from, to, format: 'unified' })
    if (path) q.set('path', path)
    return (await this.get('/roots/' + root + '/diff?' + q)).text()
  }
}
`

export const TOML_BEFORE = `# .keepster.toml — per-Root configuration
segment_period = "1h"

[triggers]
watch = true
periodic = "5m"
reconcile = true

[include]
rules = [
  "!node_modules/**",
  "!target/**",
  "!.git/**",
]

[retention]
everything_for = "24h"
hourly_until = "7d"
daily_until = "30d"
`

export const TOML_AFTER = `# .keepster.toml — per-Root configuration
segment_period = "1h"
store = "global"

[triggers]
watch = true
periodic = "2m"
reconcile = true
rate_limit = "10s"

[include]
rules = [
  "!node_modules/**",
  "!target/**",
  "!.git/**",
  "!dist/**",
  "secrets/allowed.env",
]
respect_gitignore = false

[retention]
everything_for = "24h"
hourly_until = "7d"
daily_until = "30d"
disk_cap = "2GiB"
`

export const MD_BEFORE = `# Notes — restore flow

The daemon serves the UI on 127.0.0.1 with a token.

## Open questions

- What does the timeline look like at hundreds of snapshots?
- Which diff editor?

| Mode | Meaning |
| --- | --- |
| Full file | the file as of that Snapshot |
| Delta previous | against the Snapshot before it |
`

export const MD_AFTER = `# Notes — restore flow

The daemon serves the UI on 127.0.0.1 with a token. The SPA is embedded in the binary, so there is
one artifact and no runtime dependency beyond an external \`git\`.

## Open questions

- What does the timeline look like at hundreds of snapshots?
- Which diff editor? *(this prototype)*
- How is a Snapshot that only deleted files shown?

| Mode | Meaning |
| --- | --- |
| Full file | the file as of that Snapshot |
| Delta previous | against the Snapshot before it |
| Delta current | against the Working Tree — **the default** |

> Restore never deletes. It overwrites and creates, and always takes a Safety Snapshot first.
`
