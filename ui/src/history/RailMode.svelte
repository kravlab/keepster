<script lang="ts">
  /*
   * The rail, the History screen's default view mode (ticket 15, variant A): "something broke — when?".
   * Time is a permanent rail of Snapshots with sticky day headings; the selected Snapshot's changed files
   * are listed beside it; the selected file's diff fills the rest. The file and the Snapshot are the
   * screen's, bound in from `History`, so they outlive a switch to another mode.
   *
   * The rail pages through the timeline by cursor and stays current from the live stream: a new Snapshot
   * of this Root is put on top, and `resync` reloads it, since anything shown may be stale. A Snapshot
   * carried in from another mode is paged to, so the rail opens on it rather than on the newest.
   */
  import { tick, untrack } from 'svelte'
  import DiffPane from './DiffPane.svelte'
  import FileTree from './FileTree.svelte'
  import { newestTouching } from '../lib/api'
  import { failureText } from '../lib/failures'
  import type { DiffTarget } from '../lib/files'
  import { TimelinePages } from '../lib/pages.svelte'
  import { baseName, dirName, shownPath } from '../lib/paths'
  import { railRows, timeOfDay } from '../lib/rail'
  import { arrivingSnapshot, keptSelection } from '../lib/selection'
  import { currentFor, snapshotChanges, untouchedAt, type SnapshotChanges, type UntouchedFile } from '../lib/shown'
  import { changeShape, isRoutine } from '../lib/summary'
  import { strings } from '../lib/strings'
  import { loadedPredecessor } from '../lib/timeline'

  let {
    rootId,
    path = $bindable(),
    snapshotId = $bindable(),
    onSessionEnded,
  }: {
    /** Fixed for the life of the component: `History` mounts a new one for another Root. */
    rootId: string
    /** The selected file, carried across view modes. */
    path: string | null
    /** The selected Snapshot, carried across view modes. */
    snapshotId: string | null
    onSessionEnded: () => void
  } = $props()

  let error = $state('')
  let filesError = $state('')
  let rail = $state<HTMLOListElement>()
  let hideRoutine = $state(false)
  let onlyFile = $state(false)
  /* The file column: the flat list of changes by default, the Snapshot's whole tree as an option. */
  let view = $state<'list' | 'tree'>('list')
  /*
   * What was found for the selection, each naming the Snapshot (and file) it was found for: the selected
   * Snapshot's changes, and the selected file as that Snapshot holds it when it did not change it. The
   * selection moves at once while an answer is on its way, so everything shown is derived from these only
   * while they name the current selection; reset by effects instead, they reached the diff pane a step late,
   * the new Snapshot paired with the previous one's predecessor and file (ticket 83).
   */
  let found = $state<SnapshotChanges | null>(null)
  let foundUntouched = $state<UntouchedFile | null>(null)

  /** The text of a failure; a dead session also goes to the shell. */
  const failure = (e: unknown) => failureText(e, onSessionEnded)

  /* With the per-file filter on, the rail lists only the Snapshots that changed the selected file. */
  const filterPath = $derived(onlyFile && path !== null ? path : undefined)
  /* Derived only so that it reads `rootId` reactively: the shell keys this screen by Root, so it is one list. */
  const pages = $derived(new TimelinePages(rootId, failure, () => filterPath))
  /* The rail's rows: the loaded Snapshots, less the routine ones while that filter is on. */
  const visible = $derived(hideRoutine ? pages.snapshots.filter((s) => !isRoutine(s)) : pages.snapshots)
  const rows = $derived(railRows(visible, { now: new Date() }))
  const index = $derived(pages.snapshots.findIndex((s) => s.id === snapshotId))
  const selected = $derived(pages.snapshots[index] ?? null)
  /*
   * By identity: a reload replaces every Snapshot object with an equal new one, and keyed on the objects
   * the file list would be fetched again and the open diff rebuilt, losing its scroll.
   */
  const toId = $derived(selected?.id ?? null)
  /*
   * The predecessor as the loaded list gives it: the next row of an unfiltered list, `null` past the
   * Root's first Snapshot, `undefined` when the list cannot tell (filtered, or on a page not loaded).
   */
  const adjacentId = $derived(
    filterPath === undefined ? loadedPredecessor(pages.snapshots, index, pages.cursor !== undefined) : undefined,
  )
  /* The selected Snapshot's changes, once found; `null` while they are on their way. */
  const changes = $derived(currentFor(found, toId, null))
  const files = $derived(changes?.files ?? null)
  const isFirstSnapshot = $derived(changes !== null && changes.from === null)
  const file = $derived(files?.find((f) => f.path === path) ?? currentFor(foundUntouched, toId, path)?.file ?? null)
  /* Whether the selected file is one the selected Snapshot did not change. */
  const notChangedHere = $derived(path !== null && files !== null && !files.some((f) => f.path === path))
  /* The diff pane's Snapshot, predecessor and file, all from what was found for the current selection. */
  const target = $derived<DiffTarget | null>(changes && { to: changes.to, from: changes.from, file })

  /*
   * A selection the list does not hold, or the routine filter hides, gives way to the newest Snapshot still
   * shown; judged only once a load has walked back to the selection, and never against a failed one.
   */
  $effect(() => {
    const shown = visible
    if (pages.loaded && !pages.loading && !pages.error) untrack(() => (snapshotId = keptSelection(shown, snapshotId)))
  })

  /*
   * Opens on the selection carried in, a file alone resolving to the newest Snapshot that changed it; then
   * loads the timeline again whenever the per-file filter changes what it lists.
   */
  let arrived = $state(false)
  $effect(() => {
    untrack(() => {
      void (async () => {
        try {
          snapshotId = await arrivingSnapshot({ path, snapshotId }, (p) => newestTouching(rootId, p))
        } catch (e) {
          error = failure(e)
        }
        arrived = true
      })()
    })
  })
  $effect(() => {
    void filterPath
    if (arrived) untrack(() => void pages.reload(snapshotId))
  })

  /* The live stream, for as long as this mode is shown. */
  $effect(() => pages.follow({ until: () => snapshotId }))

  /*
   * The selected Snapshot's changes, against its predecessor: the list's next row when it can tell, otherwise
   * asked for. What was found for the same Snapshot stays shown until the new answer replaces it.
   */
  $effect(() => {
    const to = toId
    const adjacent = adjacentId
    filesError = ''
    if (to === null) return
    let cancelled = false
    snapshotChanges(rootId, to, adjacent)
      .then((list) => {
        if (cancelled) return
        found = list
        // The selection is kept even where this Snapshot did not change it: it is then "unchanged here".
        if (path === null) path = list.files[0]?.path ?? null
      })
      .catch((e: unknown) => {
        if (!cancelled) filesError = failure(e)
      })
    return () => {
      cancelled = true
    }
  })

  /* A selected file this Snapshot did not change is shown as the Snapshot holds it, if it holds it. */
  $effect(() => {
    const chosen = path
    const to = toId
    if (!notChangedHere || chosen === null || to === null) return
    let cancelled = false
    untouchedAt(rootId, to, chosen)
      .then((untouched) => {
        if (!cancelled) foundUntouched = untouched
      })
      .catch((e: unknown) => {
        if (!cancelled) filesError = failure(e)
      })
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
    const at = visible.findIndex((s) => s.id === snapshotId)
    const next = visible[at + step]
    if (next) snapshotId = next.id
    if (step === 1 && at + 2 >= visible.length) void pages.older()
    await tick()
    rail?.querySelector<HTMLElement>('[aria-current="true"]')?.focus()
  }
</script>

<div class="cols">
  <nav class="rail" aria-label={strings.railLabel}>
    <label class="filter"><input type="checkbox" bind:checked={hideRoutine} /> {strings.hideRoutine}</label>
    {#if path}
      <label class="filter" title={shownPath(path)}>
        <input type="checkbox" bind:checked={onlyFile} />
        <span class="only">{strings.onlyFile(shownPath(baseName(path)))}</span>
      </label>
    {/if}
    {#if error || pages.error}
      <p class="empty error">{strings.failed} {error || pages.error}</p>
    {:else if !pages.loaded}
      <p class="empty">{strings.loading}</p>
    {:else if pages.snapshots.length === 0}
      <p class="empty">{strings.noSnapshots}</p>
    {/if}
    <ol bind:this={rail}>
      {#each rows as { snapshot, day } (snapshot.id)}
        {#if day}<li class="day">{day}</li>{/if}
        <li>
          <button
            class="row"
            tabindex={snapshot.id === snapshotId ? 0 : -1}
            aria-current={snapshot.id === snapshotId}
            onkeydown={onRailKey}
            onclick={() => (snapshotId = snapshot.id)}
          >
            <span class="time">{timeOfDay(snapshot.taken_at)}</span>
            <span class="what {changeShape(snapshot).kind}">
              {#if filterPath && snapshot.change}<span
                  class="mark {snapshot.change}"
                  aria-label={strings.changes[snapshot.change].label}>{strings.changes[snapshot.change].mark}</span
                >{/if}
              {strings.shape(changeShape(snapshot))}
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
    {#if pages.cursor !== undefined}
      <button class="older" onclick={() => pages.older()} disabled={pages.paging || pages.loading}
        >{pages.paging ? strings.loading : strings.loadOlder}</button
      >
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
    <div class="views" role="group" aria-label={strings.fileViewLabel}>
      <button aria-pressed={view === 'list'} onclick={() => (view = 'list')}>{strings.listView}</button>
      <button aria-pressed={view === 'tree'} onclick={() => (view = 'tree')}>{strings.treeView}</button>
    </div>
    {#if view === 'tree' && toId && files}
      <FileTree {rootId} snapshotId={toId} changes={files} bind:path {onSessionEnded} />
    {:else}
      <ul>
        {#if notChangedHere && path}
          <li>
            <button class="file" aria-current="true" title={shownPath(path)}>
              <span class="mark unchanged" aria-hidden="true">{strings.changes.unchanged.mark}</span>
              <span class="name">{shownPath(baseName(path))}</span>
              <span class="dir">{strings.changes.unchanged.label}</span>
            </button>
          </li>
        {/if}
        {#each files ?? [] as f (f.path)}
          <li>
            <button
              class="file"
              aria-current={f.path === path}
              onclick={() => (path = f.path)}
              title={shownPath(f.path)}
            >
              <span class="mark {f.change}" aria-label={strings.changes[f.change].label}>
                {strings.changes[f.change].mark}
              </span>
              <span class="name">{shownPath(baseName(f.path))}</span>
              <span class="dir">{shownPath(dirName(f.path))}</span>
            </button>
          </li>
        {/each}
      </ul>
    {/if}
  </section>

  {#if toId}
    <!-- From what was found, not from `selected`: a reload replaces that object, and would re-run the pane. -->
    <DiffPane {rootId} {target} {onSessionEnded} />
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
  .views {
    display: flex;
    gap: 4px;
    padding: 6px 10px;
    border-bottom: 1px solid var(--line);
  }
  .views button[aria-pressed='true'] {
    background: var(--selected);
  }
  .mark.unchanged {
    color: var(--muted);
  }
  .filter {
    display: flex;
    gap: 6px;
    align-items: center;
    padding: 6px 10px;
    border-bottom: 1px solid var(--line);
    color: var(--muted);
  }
  .only {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .what.onlyDeleted {
    color: var(--danger);
  }
  .what.onlyAdded {
    color: var(--added);
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
