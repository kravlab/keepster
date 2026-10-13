<script lang="ts">
  /*
   * The activity feed, the History screen's third view mode (ticket 15, variant C): "what happened here
   * while I was away?". One column of Snapshot cards under day headings, each saying in words what it
   * changed, a Snapshot that only deleted files the loudest card on the page, and listing its changed
   * paths. A path opens its diff in an overlay above the feed, Restore in its bar as in every mode.
   *
   * A card asks for its changed paths only once it comes into view, and keeps them: a page holds a hundred
   * cards, and most are never looked at. A Root's first Snapshot lists what it holds, all of it new. The
   * feed publishes the file and Snapshot a user opens, but never consumes them: arriving from another mode
   * must not open an overlay by itself. A search result is the exception (ticket 66): picked here, it is
   * what the user asked to see, so the feed walks back to its Snapshot and opens its diff.
   */
  import { untrack } from 'svelte'
  import type { Attachment } from 'svelte/attachments'
  import { SvelteMap, SvelteSet } from 'svelte/reactivity'
  import DiffPane from './DiffPane.svelte'
  import { changedFiles, entryAt, previousSnapshot, snapshotEntries } from '../lib/api'
  import { failureText } from '../lib/failures'
  import { fromDiff, fromEntries, unchangedFile, type DiffTarget, type ShownFile } from '../lib/files'
  import { TimelinePages } from '../lib/pages.svelte'
  import { railRows, timeOfDay } from '../lib/rail'
  import { strings } from '../lib/strings'
  import { changeShape, isRoutine } from '../lib/summary'
  import { loadedPredecessor } from '../lib/timeline'
  import type { Snapshot } from '../lib/wire'

  let {
    rootId,
    path = $bindable(),
    snapshotId = $bindable(),
    arriving,
    onSessionEnded,
  }: {
    /** Fixed for the life of the component: `History` mounts a new one for another Root. */
    rootId: string
    /** The selected file, carried across view modes; set when a path is opened. */
    path: string | null
    /** The selected Snapshot, carried across view modes; set when a path is opened. */
    snapshotId: string | null
    /** Whether the selection was handed in by a search result, whose diff then opens at once. */
    arriving: boolean
    onSessionEnded: () => void
  } = $props()

  /** How many of a card's changed paths show before "Show N more". */
  const FIRST_PATHS = 20

  /**
   * A card's changed paths, as far as they are known, with the Snapshot they were compared against:
   * `null` for a Root's first Snapshot, whose paths are all it holds.
   */
  type CardFiles =
    | { state: 'loading' }
    | { state: 'ready'; from: string | null; files: ShownFile[] }
    | { state: 'failed'; error: string }

  /** The text of a failure; a dead session also goes to the shell. */
  const failure = (e: unknown) => failureText(e, onSessionEnded)

  /* Derived only so that it reads `rootId` reactively: the shell keys this screen by Root, so it is one list. */
  const pages = $derived(new TimelinePages(rootId, failure))
  let hideRoutine = $state(false)
  /* Each card's changed paths by Snapshot id, kept once asked for. */
  const cards = new SvelteMap<string, CardFiles>()
  /* The cards showing all their paths, not only the first few. */
  const expanded = new SvelteSet<string>()
  /* Counts the times the kept paths were thrown away; the cards are drawn anew, and those in view ask again. */
  let epoch = $state(0)
  let open = $state<{ snapshot: Snapshot; from: string | null; file: ShownFile } | null>(null)
  /* The open diff as the pane takes it: one value, made anew only when another diff is opened. */
  const openTarget = $derived<DiffTarget | null>(open && { to: open.snapshot.id, from: open.from, file: open.file })
  /* Why a search result handed in could not be opened. */
  let arrivalError = $state('')

  const visible = $derived(hideRoutine ? pages.snapshots.filter((s) => !isRoutine(s)) : pages.snapshots)
  const rows = $derived(railRows(visible, { now: new Date() }))

  /*
   * The feed opens on the newest Snapshots; with a search result handed in, on as many pages as reach its
   * Snapshot, and then on its diff.
   */
  $effect(() => {
    const list = pages
    untrack(() => {
      const target = arriving && path !== null && snapshotId !== null ? { path, snapshot: snapshotId } : null
      void list.reload(target?.snapshot).then(() => {
        if (target) void openArrival(target.path, target.snapshot)
      })
    })
  })

  /* The live stream: new Snapshots go on top; after `resync` anything shown may be stale, paths included. */
  $effect(() =>
    pages.follow({
      onResync: () => {
        cards.clear()
        epoch += 1
      },
    }),
  )

  /**
   * Asks for one card's changed paths, against the Snapshot before it: the next card when the loaded
   * pages tell, otherwise asked of the daemon. Paths already kept are not asked for again.
   */
  async function load(id: string) {
    if (cards.has(id)) return
    const mine = epoch
    cards.set(id, { state: 'loading' })
    let files: CardFiles
    try {
      const index = pages.snapshots.findIndex((s) => s.id === id)
      const known = loadedPredecessor(pages.snapshots, index, pages.cursor !== undefined)
      const from = known !== undefined ? known : await previousSnapshot(rootId, id)
      const list = from
        ? fromDiff(await changedFiles(rootId, from, id))
        : fromEntries(await snapshotEntries(rootId, id))
      files = { state: 'ready', from, files: list }
    } catch (e) {
      files = { state: 'failed', error: failure(e) }
    }
    if (mine === epoch) cards.set(id, files)
  }

  /**
   * Opens the diff of `file` in Snapshot `id`, as its card lists it; a file the Snapshot did not change is
   * shown as the Snapshot holds it. Nothing opens when the Snapshot is not in the feed.
   */
  async function openArrival(file: string, id: string) {
    const snapshot = pages.snapshots.find((s) => s.id === id)
    if (!snapshot) return
    await load(id)
    const card = cards.get(id)
    if (card?.state !== 'ready') return
    try {
      const shown = card.files.find((f) => f.path === file) ?? unchangedFile(file, await entryAt(rootId, id, file))
      show(snapshot, card.from, shown)
    } catch (e) {
      arrivalError = failure(e)
    }
  }

  /** Loads a card's paths the first time it comes within a screen of view. */
  function onFirstSight(id: string): Attachment<HTMLElement> {
    return (card) => {
      const sight = new IntersectionObserver(
        (entries) => {
          if (!entries.some((entry) => entry.isIntersecting)) return
          sight.disconnect()
          void load(id)
        },
        { root: card.closest('.feed'), rootMargin: '100% 0px' },
      )
      sight.observe(card)
      return () => sight.disconnect()
    }
  }

  /** Opens a path's diff in the overlay, and makes the file and Snapshot the screen's selection. */
  function show(snapshot: Snapshot, from: string | null, file: ShownFile) {
    open = { snapshot, from, file }
    path = file.path
    snapshotId = snapshot.id
  }
</script>

<section class="feed" aria-label={strings.feedLabel}>
  <div class="column">
    <label class="filter"><input type="checkbox" bind:checked={hideRoutine} /> {strings.hideRoutine}</label>
    {#if arrivalError}
      <p class="empty error">{strings.failed} {arrivalError}</p>
    {/if}
    {#if pages.error}
      <p class="empty error">{strings.failed} {pages.error}</p>
    {:else if !pages.loaded}
      <p class="empty">{strings.loading}</p>
    {:else if pages.snapshots.length === 0}
      <p class="empty">{strings.noSnapshots}</p>
    {/if}

    {#key epoch}
      {#each rows as { snapshot, day } (snapshot.id)}
        {@const shape = changeShape(snapshot)}
        {@const files = cards.get(snapshot.id)}
        {#if day}<h3 class="day">{day}</h3>{/if}
        <article class:alarm={shape.kind === 'onlyDeleted'} {@attach onFirstSight(snapshot.id)}>
          <div class="head">
            <span class="time">{timeOfDay(snapshot.taken_at)}</span>
            <span class="trigger">{snapshot.trigger}</span>
            {#if snapshot.pinned}<span class="tag">{strings.pinned}</span>{/if}
            {#if snapshot.incomplete}<span class="tag warn">{strings.incomplete}</span>{/if}
          </div>
          <p class="lead {shape.kind}">{strings.shape(shape)}</p>
          {#if files?.state === 'ready' && files.from === null}
            <p class="muted">{strings.firstSnapshot}</p>
          {/if}
          {#if files?.state === 'failed'}
            <p class="error">{strings.failed} {files.error}</p>
          {:else if files?.state === 'ready'}
            {@const shown = expanded.has(snapshot.id) ? files.files : files.files.slice(0, FIRST_PATHS)}
            <ul>
              {#each shown as f (f.path)}
                <li>
                  <button
                    class="path"
                    aria-current={f.path === path && snapshot.id === snapshotId}
                    onclick={() => show(snapshot, files.from, f)}
                    title={f.path}
                  >
                    <span class="mark {f.change}" aria-label={strings.changes[f.change].label}>
                      {strings.changes[f.change].mark}
                    </span>
                    <code>{f.path}</code>
                  </button>
                </li>
              {/each}
            </ul>
            {#if shown.length < files.files.length}
              <button class="more" onclick={() => expanded.add(snapshot.id)}>
                {strings.morePaths(files.files.length - shown.length)}
              </button>
            {/if}
          {/if}
        </article>
      {/each}
    {/key}

    {#if pages.cursor !== undefined}
      <button class="older" onclick={() => pages.older()} disabled={pages.paging || pages.loading}>
        {pages.paging ? strings.loading : strings.loadOlder}
      </button>
    {/if}
  </div>
</section>

{#if open}
  <dialog class="overlay" {@attach (d: HTMLDialogElement) => d.showModal()} onclose={() => (open = null)}>
    <header class="bar">
      <code class="name">{open.file.path}</code>
      <span class="muted">{strings.snapshotOf(new Date(open.snapshot.taken_at))}</span>
      <span class="spacer"></span>
      <button onclick={(e) => e.currentTarget.closest('dialog')?.close()}>{strings.closeDiff}</button>
    </header>
    <div class="body">
      <DiffPane {rootId} target={openTarget} {onSessionEnded} />
    </div>
  </dialog>
{/if}

<style>
  .feed {
    flex: 1;
    min-height: 0;
    overflow: auto;
  }
  .column {
    width: min(680px, calc(100% - 32px));
    margin: 0 auto;
    padding: 10px 0 40px;
  }
  .filter {
    display: flex;
    gap: 6px;
    align-items: center;
    padding: 4px 0 8px;
    color: var(--muted);
  }
  .day {
    position: sticky;
    top: 0;
    z-index: 1;
    margin: 0;
    padding: 6px 0;
    background: var(--bg);
    color: var(--muted);
    font-size: 12px;
    font-weight: 500;
  }
  article {
    margin: 0 0 10px;
    padding: 10px 12px;
    border: 1px solid var(--line);
    border-radius: 8px;
    background: var(--panel);
  }
  article.alarm {
    border-color: var(--danger);
    background: color-mix(in srgb, var(--danger) 6%, var(--panel));
  }
  .head {
    display: flex;
    gap: 10px;
    align-items: baseline;
  }
  .time {
    font-variant-numeric: tabular-nums;
  }
  .trigger,
  .muted {
    color: var(--muted);
  }
  .trigger {
    font-size: 11px;
  }
  .tag {
    font-size: 11px;
    color: var(--accent);
  }
  .lead {
    margin: 6px 0;
  }
  .lead.onlyDeleted {
    color: var(--danger);
    font-weight: 600;
  }
  .lead.onlyAdded {
    color: var(--added);
  }
  p {
    margin: 4px 0;
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .path {
    display: grid;
    grid-template-columns: 12px 1fr;
    gap: 6px;
    width: 100%;
    border: 0;
    border-radius: 4px;
    background: none;
    padding: 2px 4px;
    text-align: left;
  }
  .path[aria-current='true'] {
    background: var(--selected);
  }
  .path code {
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
  .more {
    margin-top: 4px;
  }
  .older {
    display: block;
    margin: 10px auto;
  }
  .warn {
    color: var(--warn);
  }
  .empty {
    padding: 12px 0;
    margin: 0;
    color: var(--muted);
  }
  .overlay {
    width: 100vw;
    height: 100vh;
    max-width: none;
    max-height: none;
    margin: 0;
    padding: 0;
    border: 0;
    background: var(--bg);
    color: var(--fg);
  }
  .overlay[open] {
    display: flex;
    flex-direction: column;
  }
  .bar {
    display: flex;
    gap: 10px;
    align-items: baseline;
    padding: 8px 14px;
    border-bottom: 1px solid var(--line);
    background: var(--panel);
  }
  .name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .spacer {
    flex: 1;
  }
  .body {
    flex: 1;
    min-height: 0;
    display: grid;
  }
</style>
