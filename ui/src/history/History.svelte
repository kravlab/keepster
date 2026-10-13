<script lang="ts">
  /*
   * The History screen: variant A of ticket 15. Time is a permanent rail of Snapshots with sticky day
   * headings; the selected Snapshot's changed files are listed beside it; the selected file's diff fills
   * the rest. View modes, the file filter and Restore belong to ticket 65.
   *
   * The rail pages through the timeline by cursor and stays current from the live stream: a new Snapshot
   * of this Root is put on top, and `resync` reloads it, since anything shown may be stale.
   */
  import { tick, untrack } from 'svelte'
  import DiffPane from './DiffPane.svelte'
  import { changedFiles, snapshotEntries, timeline } from '../lib/api'
  import { connectEvents } from '../lib/events'
  import { failureText } from '../lib/failures'
  import { fromDiff, fromEntries, type ShownFile } from '../lib/files'
  import { railRows, timeOfDay } from '../lib/rail'
  import { strings } from '../lib/strings'
  import { withNewer } from '../lib/timeline'
  import type { Snapshot } from '../lib/wire'

  let { rootId, onSessionEnded }: { rootId: string; onSessionEnded: () => void } = $props()

  let snapshots = $state<Snapshot[]>([])
  let cursor = $state<string | undefined>()
  let loaded = $state(false)
  let paging = $state(false)
  let error = $state('')
  let selectedId = $state<string | null>(null)
  let files = $state<ShownFile[] | null>(null)
  let filesError = $state('')
  let path = $state<string | null>(null)
  let rail = $state<HTMLOListElement>()

  const rows = $derived(railRows(snapshots, { now: new Date() }))
  const index = $derived(snapshots.findIndex((s) => s.id === selectedId))
  const selected = $derived(snapshots[index] ?? null)
  /* The Snapshot before the selected one; `null` while it is on a page not loaded yet, or there is none. */
  const previous = $derived(index === -1 ? null : (snapshots[index + 1] ?? null))
  const isFirstSnapshot = $derived(selected !== null && previous === null && cursor === undefined)
  /*
   * The pair the file list is for, by identity. A reload replaces every Snapshot object with an equal new
   * one; keyed on the objects, the list would be fetched again and the open diff rebuilt, losing its scroll.
   */
  const toId = $derived(selected?.id ?? null)
  const fromId = $derived(previous?.id ?? null)
  const file = $derived(files?.find((f) => f.path === path) ?? null)

  /** The text of a failure; a dead session also goes to the shell. */
  const failure = (e: unknown) => failureText(e, onSessionEnded)

  /** Loads the newest page afresh, keeping the selection when it is still on it. */
  async function reload(id: string) {
    try {
      const page = await timeline(id, undefined)
      if (id !== rootId) return
      snapshots = page.snapshots
      cursor = page.next_cursor
      if (!snapshots.some((s) => s.id === selectedId)) selectedId = snapshots[0]?.id ?? null
      error = ''
    } catch (e) {
      error = failure(e)
    } finally {
      loaded = true
    }
  }

  /** Puts Snapshots taken since the newest shown one on top, or reloads when too many were. */
  async function refresh(id: string) {
    try {
      const page = await timeline(id, undefined)
      if (id !== rootId) return
      const merged = withNewer(snapshots, page.snapshots)
      if (merged === null) await reload(id)
      else snapshots = merged
    } catch (e) {
      error = failure(e)
    }
  }

  /** Appends the next, older page. */
  async function older() {
    if (cursor === undefined || paging) return
    const id = rootId
    paging = true
    try {
      const page = await timeline(id, cursor)
      if (id !== rootId) return
      snapshots = [...snapshots, ...page.snapshots]
      cursor = page.next_cursor
    } catch (e) {
      error = failure(e)
    } finally {
      paging = false
    }
  }

  /* A Root change starts over. */
  $effect(() => {
    const id = rootId
    snapshots = []
    cursor = undefined
    loaded = false
    selectedId = null
    void reload(id)
  })

  /* The live stream, for as long as this Root is shown. */
  $effect(() => {
    const id = rootId
    const live = connectEvents({
      names: ['snapshot.created'],
      onEvent: ({ name, data }) => {
        if (name === 'resync') void reload(id)
        else if ((data as { root?: string }).root === id) void refresh(id)
      },
    })
    return () => live.close()
  })

  /* The selected Snapshot's files. Its predecessor may sit on the next page, which is fetched first. */
  $effect(() => {
    const id = rootId
    const to = toId
    const from = fromId
    const isFirst = isFirstSnapshot
    files = null
    filesError = ''
    if (to === null) return
    if (from === null && !isFirst) {
      // Untracked: paging state must not re-run this effect and blank the list.
      untrack(() => void older())
      return
    }
    let cancelled = false
    void (async () => {
      try {
        const list = from ? fromDiff(await changedFiles(id, from, to)) : fromEntries(await snapshotEntries(id, to))
        if (cancelled) return
        files = list
        if (!list.some((f) => f.path === path)) path = list[0]?.path ?? null
      } catch (e) {
        if (!cancelled) filesError = failure(e)
      }
    })()
    return () => {
      cancelled = true
    }
  })

  /**
   * Moves the selection along the rail with ↑ and ↓, paging in older Snapshots at the bottom. The rail is
   * one tab stop: only the selected row is focusable, and focus follows the selection.
   */
  async function onRailKey(event: KeyboardEvent) {
    const step = event.key === 'ArrowDown' ? 1 : event.key === 'ArrowUp' ? -1 : 0
    if (step === 0) return
    event.preventDefault()
    const next = snapshots[index + step]
    if (next) selectedId = next.id
    if (step === 1 && index + 2 >= snapshots.length) void older()
    await tick()
    rail?.querySelector<HTMLElement>('[aria-current="true"]')?.focus()
  }
</script>

<div class="cols">
  <nav class="rail" aria-label={strings.railLabel}>
    {#if error}
      <p class="empty error">{strings.failed} {error}</p>
    {:else if !loaded}
      <p class="empty">{strings.loading}</p>
    {:else if snapshots.length === 0}
      <p class="empty">{strings.noSnapshots}</p>
    {/if}
    <ol bind:this={rail}>
      {#each rows as { snapshot, day } (snapshot.id)}
        {#if day}<li class="day">{day}</li>{/if}
        <li>
          <button
            class="row"
            tabindex={snapshot.id === selectedId ? 0 : -1}
            aria-current={snapshot.id === selectedId}
            onkeydown={onRailKey}
            onclick={() => (selectedId = snapshot.id)}
          >
            <span class="time">{timeOfDay(snapshot.taken_at)}</span>
            <span class="what">
              {snapshot.changed_paths === undefined ? strings.notCounted : strings.changed(snapshot.changed_paths)}
              {#if snapshot.pinned}<span class="tag" title={strings.pinned}>{strings.pinned}</span>{/if}
              {#if snapshot.incomplete}<span class="tag warn" title={strings.incomplete} aria-label={strings.incomplete}
                  >{strings.incompleteMark}</span
                >{/if}
            </span>
            <span class="trigger">{snapshot.trigger}</span>
          </button>
        </li>
      {/each}
    </ol>
    {#if cursor !== undefined}
      <button class="older" onclick={older} disabled={paging}>{paging ? strings.loading : strings.loadOlder}</button>
    {/if}
  </nav>

  <section class="files" aria-label={strings.filesLabel}>
    {#if selected?.incomplete}<p class="banner warn">{strings.incompleteBanner}</p>{/if}
    {#if isFirstSnapshot}<p class="banner">{strings.firstSnapshot}</p>{/if}
    {#if filesError}
      <p class="empty error">{strings.failed} {filesError}</p>
    {:else if selected && files === null}
      <p class="empty">{strings.loading}</p>
    {:else if files?.length === 0}
      <p class="empty">{strings.noChanges}</p>
    {/if}
    <ul>
      {#each files ?? [] as f (f.path)}
        <li>
          <button class="file" aria-current={f.path === path} onclick={() => (path = f.path)} title={f.path}>
            <span class="mark {f.change}" aria-label={strings.changes[f.change].label}>
              {strings.changes[f.change].mark}
            </span>
            <span class="name">{f.path.split('/').pop()}</span>
            <span class="dir">{f.path.split('/').slice(0, -1).join('/')}</span>
          </button>
        </li>
      {/each}
    </ul>
  </section>

  {#if toId}
    <!-- By identity, not `selected.id`: reading it through the object re-runs the pane on every reload. -->
    <DiffPane {rootId} from={fromId} to={toId} {file} {onSessionEnded} />
  {/if}
</div>

<style>
  .cols {
    flex: 1;
    min-height: 0;
    display: grid;
    grid-template-columns: 260px 280px 1fr;
  }
  .rail,
  .files {
    border-right: 1px solid var(--line);
    background: var(--panel);
    overflow: auto;
    min-height: 0;
  }
  ol,
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .row:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: -2px;
  }
  .day {
    position: sticky;
    top: 0;
    z-index: 1;
    padding: 4px 10px;
    background: var(--bg);
    color: var(--muted);
    border-bottom: 1px solid var(--line);
  }
  .row,
  .file {
    display: grid;
    gap: 8px;
    width: 100%;
    text-align: left;
    border: 0;
    border-radius: 0;
    background: none;
    padding: 5px 10px;
  }
  .row {
    grid-template-columns: auto 1fr auto;
  }
  .file {
    grid-template-columns: 12px auto 1fr;
    gap: 6px;
  }
  .row[aria-current='true'],
  .file[aria-current='true'] {
    background: var(--selected);
  }
  .time,
  .trigger,
  .dir {
    color: var(--muted);
  }
  .time {
    white-space: nowrap;
    font-variant-numeric: tabular-nums;
  }
  .trigger,
  .dir {
    font-size: 11px;
  }
  .tag {
    margin-left: 4px;
    font-size: 11px;
    color: var(--accent);
  }
  .name,
  .dir {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .mark.added {
    color: var(--added);
  }
  .mark.deleted {
    color: var(--danger);
  }
  .mark.modified,
  .mark.renamed {
    color: var(--accent);
  }
  .older {
    margin: 8px 10px;
  }
  .banner {
    margin: 0;
    padding: 6px 10px;
    border-bottom: 1px solid var(--line);
  }
  .warn {
    color: var(--warn);
  }
  .empty {
    padding: 12px 10px;
    margin: 0;
    color: var(--muted);
  }
</style>
