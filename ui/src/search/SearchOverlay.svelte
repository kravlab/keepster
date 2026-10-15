<script lang="ts">
  /*
   * The search overlay (ticket 66): a way into history, not a view mode. The header's field opens it with
   * what was typed; it searches paths across every Root, or, with "Content", what the current Root's
   * stored versions hold (every Root's, with "Every Root"), streamed with its progress and a Cancel.
   *
   * Keyboard first: ↑ and ↓ move through the results, Enter picks one, Esc closes. Picking hands the
   * result's file and Snapshot to the shell, which takes the History screen there. Closing stops any
   * search still running.
   */
  import { failureText } from '../lib/failures'
  import { momentOf } from '../lib/ids'
  import { shownPath } from '../lib/paths'
  import {
    SEARCH_PAUSE_MS,
    byRoot,
    matchTarget,
    pathTarget,
    rootProgress,
    searchProgress,
    type SearchTarget,
  } from '../lib/search'
  import { SearchRun } from '../lib/searching.svelte'
  import { strings } from '../lib/strings'
  import type { Root } from '../lib/wire'

  let {
    query,
    roots,
    currentRoot,
    onPick,
    onClose,
    onSessionEnded,
  }: {
    /** What the header's field held when the overlay opened. */
    query: string
    /** Every registered Root, to name each by its directory. */
    roots: readonly Root[]
    /** The Root the History screen shows: the one a content search reads unless "Every Root". */
    currentRoot: string | null
    onPick: (target: SearchTarget) => void
    onClose: () => void
    onSessionEnded: () => void
  } = $props()

  /** Where the results list is, for the field to name as the list it controls. */
  const LIST_ID = 'search-results'

  /* Seeded once: the overlay's field is the user's from here on. */
  // svelte-ignore state_referenced_locally
  let q = $state(query)
  let content = $state(false)
  let everyRoot = $state(false)
  let active = $state(0)
  let list = $state<HTMLElement>()
  let field = $state<HTMLInputElement>()

  const run = new SearchRun((e) => failureText(e, onSessionEnded))

  /** One result as a row: where it leads, and what it says. */
  type Row = { key: string; root: string; target: SearchTarget; title: string; detail: string; code?: string }

  const rows = $derived.by((): Row[] => {
    if (content) {
      return (run.content?.matches ?? []).map((m, i) => ({
        key: `m${i}`,
        root: m.root,
        target: matchTarget(m),
        title: shownPath(m.path),
        detail: strings.search.match(m.line, momentOf(m.first)),
        code: m.snippet,
      }))
    }
    return (run.paths?.hits ?? []).map((hit, i) => ({
      key: `p${i}`,
      root: hit.root,
      target: pathTarget(hit),
      title: shownPath(hit.path),
      detail: strings.search.pathDetail(hit.snapshots, hit.exists),
    }))
  })
  const groups = $derived(byRoot(rows.map((row, index) => ({ ...row, index }))))
  const progress = $derived(run.content ? searchProgress(run.content) : null)
  /* Each Root's part, shown beside the total when the search reads more than one. */
  const perRoot = $derived(run.content ? (rootProgress(run.content) ?? []) : [])
  const unsearched = $derived(content ? (run.content?.errors ?? []) : (run.paths?.unsearched ?? []))

  /** A Root named by its directory, or by its identity when it is not registered any more. */
  const rootName = (id: string) => roots.find((r) => r.id === id)?.path ?? id

  /* Runs the search once typing pauses; the first run, with what the header held, starts at once. */
  let first = true
  $effect(() => {
    const options = { content, root: currentRoot, everyRoot }
    const wanted = q
    const timer = setTimeout(
      () => {
        active = 0
        void run.start(wanted, options)
      },
      first ? 0 : SEARCH_PAUSE_MS,
    )
    first = false
    return () => clearTimeout(timer)
  })

  /* Whatever still runs stops with the overlay. */
  $effect(() => () => run.cancel())

  /* The active row stays in view as ↑ and ↓ move it. */
  $effect(() => {
    list?.querySelector(`[data-index="${active}"]`)?.scrollIntoView({ block: 'nearest' })
  })

  /** Hands a result's file and Snapshot to the shell; nothing when there is no active result. */
  function pick(row: Row | undefined) {
    if (row) onPick(row.target)
  }

  /**
   * ↑ and ↓ move through the results from anywhere in the overlay, a toggle just switched included; Enter
   * picks the active one from the field, where it would otherwise do nothing.
   */
  function onKey(event: KeyboardEvent) {
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault()
      if (rows.length === 0) return
      const step = event.key === 'ArrowDown' ? 1 : -1
      active = (active + step + rows.length) % rows.length
    } else if (event.key === 'Enter' && event.target === field) {
      event.preventDefault()
      pick(rows[active])
    }
  }
</script>

<dialog
  class="overlay"
  aria-label={strings.search.title}
  {@attach (d: HTMLDialogElement) => d.showModal()}
  onclose={onClose}
  onkeydown={onKey}
>
  <header class="bar">
    <!-- svelte-ignore a11y_autofocus -->
    <input
      class="query"
      type="search"
      role="combobox"
      aria-label={strings.search.query}
      aria-expanded="true"
      aria-controls={LIST_ID}
      aria-activedescendant={rows[active] ? `search-row-${active}` : undefined}
      autocomplete="off"
      spellcheck="false"
      autofocus
      bind:value={q}
      bind:this={field}
    />
    <label><input type="checkbox" bind:checked={content} /> {strings.search.content}</label>
    {#if content}
      <label><input type="checkbox" bind:checked={everyRoot} /> {strings.search.everyRoot}</label>
    {/if}
    {#if run.running}
      <button onclick={() => run.cancel()}>{strings.search.cancel}</button>
    {/if}
    <button onclick={(e) => e.currentTarget.closest('dialog')?.close()}>{strings.search.close}</button>
  </header>

  <div class="status" role="status">
    {#if run.error}
      <span class="error">{strings.failed} {run.error}</span>
    {:else if progress && (run.running || !run.content?.summary)}
      <span>{strings.search.progress(progress.done, progress.total)}</span>
      {#if perRoot.length > 1}
        {#each perRoot as part (part.root)}
          <span>{strings.search.rootProgress(rootName(part.root), part.done, part.total)}</span>
        {/each}
      {/if}
      {#if run.cancelled}<span>{strings.search.cancelled}</span>{/if}
    {:else if run.running}
      {strings.search.searching}
    {:else if run.cancelled}
      {strings.search.cancelled}
    {:else if run.content?.summary}
      {@const s = run.content.summary}
      {strings.search.summary(s.versions_scanned, s.binaries_skipped, s.withheld)}
    {/if}
    {#each unsearched as u (u.root)}
      <span class="warn">{strings.search.notSearched(rootName(u.root), u.detail)}</span>
    {/each}
  </div>

  <div class="results" id={LIST_ID} role="listbox" aria-label={strings.search.results} bind:this={list}>
    {#each groups as group (group.root)}
      <div role="group" aria-label={rootName(group.root)}>
        <h3 class="root">{rootName(group.root)}</h3>
        {#each group.items as row (row.key)}
          <!-- The keyboard's ↑, ↓ and Enter drive the list; a click is the pointer's way to the same pick. -->
          <!-- svelte-ignore a11y_click_events_have_key_events -->
          <div
            class="row"
            role="option"
            id={`search-row-${row.index}`}
            data-index={row.index}
            aria-selected={row.index === active}
            tabindex="-1"
            onclick={() => pick(row)}
            onmousemove={() => (active = row.index)}
          >
            <code class="path">{row.title}</code>
            <span class="muted">{row.detail}</span>
            {#if row.code !== undefined}<code class="snippet">{row.code}</code>{/if}
          </div>
        {/each}
      </div>
    {/each}
    {#if !run.running && !run.error && (run.paths || run.content?.summary) && rows.length === 0}
      <p class="empty">{strings.search.nothing}</p>
    {/if}
    {#if run.paths?.truncated && !content}
      <p class="empty">{strings.search.truncated}</p>
    {/if}
    {#if content && run.content && run.content.unshown > 0}
      <p class="empty">{strings.search.unshown(run.content.unshown)}</p>
    {/if}
  </div>
</dialog>

<style>
  .overlay {
    width: min(880px, calc(100vw - 32px));
    height: min(640px, calc(100vh - 64px));
    max-width: none;
    max-height: none;
    margin: 32px auto 0;
    padding: 0;
    border: 1px solid var(--line);
    border-radius: 8px;
    background: var(--bg);
    color: var(--fg);
  }
  .overlay[open] {
    display: flex;
    flex-direction: column;
  }
  .overlay::backdrop {
    background: rgb(0 0 0 / 0.35);
  }
  .bar {
    display: flex;
    gap: 10px;
    align-items: center;
    padding: 8px 12px;
    border-bottom: 1px solid var(--line);
    background: var(--panel);
  }
  .query {
    flex: 1;
    min-width: 0;
    font: inherit;
    color: inherit;
    background: var(--bg);
    border: 1px solid var(--line);
    border-radius: 6px;
    padding: 4px 8px;
  }
  label {
    display: flex;
    gap: 4px;
    align-items: center;
    white-space: nowrap;
  }
  .status {
    display: flex;
    flex-wrap: wrap;
    gap: 4px 12px;
    min-height: 1.5em;
    padding: 4px 12px;
    color: var(--muted);
    border-bottom: 1px solid var(--line);
  }
  .warn {
    color: var(--warn);
  }
  .results {
    flex: 1;
    min-height: 0;
    overflow: auto;
    padding: 4px 0 12px;
  }
  .root {
    position: sticky;
    top: 0;
    margin: 0;
    padding: 6px 12px;
    background: var(--bg);
    color: var(--muted);
    font-size: 12px;
    font-weight: 500;
  }
  .row {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    gap: 2px 12px;
    padding: 4px 12px;
    cursor: pointer;
  }
  .row[aria-selected='true'] {
    background: var(--selected);
  }
  .path {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .snippet {
    grid-column: 1 / -1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: pre;
    color: var(--muted);
  }
  .empty {
    margin: 0;
    padding: 12px;
    color: var(--muted);
  }
</style>
