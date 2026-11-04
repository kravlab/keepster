<script lang="ts">
  /*
   * The application shell: the header with the Root picker, the way to the Root management page, the
   * search field and the theme, and below it the History screen for that Root or the management page. The
   * shell owns the page's preferences and provides them to every screen below it.
   *
   * It owns the Watched Roots too (ticket 92), which the picker, the management page and search share.
   * They follow the daemon, but which Root is shown changes only by the person's hand or a search result
   * they picked: the first Root is chosen once, when the Roots are first known, and never by an event.
   * A shown Root unwatched or erased elsewhere stays shown (#33): marked no longer watched, kept in the
   * picker until another is chosen, and with a banner once its history is erased.
   *
   * It also owns the History screen's mode and selection (ticket 66), because a search result sets them:
   * picking one selects its file and Snapshot, switching the Root picker when the result is in another
   * Root, and mounts the screen anew so that the mode shown opens there. Choosing a Root by hand starts
   * with nothing selected, in the same mode.
   *
   * The browser arrives here already signed in: `keepster ui` opened `/?bootstrap=…`, the daemon swapped
   * the code for the session cookie and redirected to `/` (ticket 60), so the address bar holds no secret.
   * When a request finds the session gone, every screen gives way to one that says how to get back.
   */
  import ErasedBanner from './history/ErasedBanner.svelte'
  import History from './history/History.svelte'
  import RootsPage from './roots/RootsPage.svelte'
  import SearchOverlay from './search/SearchOverlay.svelte'
  import { failureText } from './lib/failures'
  import { THEMES } from './lib/preferences'
  import { PreferencesState, providePreferences } from './lib/preferences.svelte'
  import { WatchedRoots } from './lib/roots.svelte'
  import { SEARCH_PAUSE_MS, type SearchTarget } from './lib/search'
  import type { Mode, Selection } from './lib/selection'
  import { ShownRoot } from './lib/showing.svelte'
  import { strings } from './lib/strings'

  let ended = $state(false)
  /* Whether the Root management page is shown instead of the History screen. */
  let managing = $state(false)
  /* Whether the management page opens with its add dialog: the empty state's way there asks for it. */
  let addFirst = $state(false)
  /*
   * A Root opened before the live list holds it, as one just added is: until it arrives the screen says it
   * is loading. Forgotten once it arrives, so a Root removed later reads as no Root chosen.
   */
  let awaited = $state<string | null>(null)

  let mode = $state<Mode>('rail')
  let selection = $state<Selection>({ path: null, snapshotId: null })
  /* Whether the selection was just handed in by a search result. */
  let arriving = $state(false)
  /* Counts the search results picked: each one mounts the History screen anew. */
  let visit = $state(0)
  /* Said when a search result switched the Root picker, or led nowhere. */
  let notice = $state('')

  let searchText = $state('')
  /* The query the overlay opened with, while it is open. */
  let searching = $state<string | null>(null)
  let searchPause: ReturnType<typeof setTimeout> | undefined

  /* The Registry, `watched.roots`: what the management page lists. */
  const watched = new WatchedRoots((e) => failureText(e, endSession))
  /*
   * The Root shown, chosen only here, through `shown.show`. The picker offers `shown.listed`: the Registry,
   * and the shown Root while it is kept no longer watched.
   */
  const shown = new ShownRoot(watched)
  /*
   * The shown Root's id, apart from its record. Every change to the Registry reloads the Roots as new
   * records, and the History screen must not take the shown Root's new record for another Root: what it
   * builds from the id would be built again, empty, while the Root stayed the same.
   */
  const shownId = $derived(shown.current?.id ?? null)

  /** Every screen calls this when the daemon answers 401. */
  function endSession() {
    ended = true
  }

  /** Shows the Root management page, opening its add dialog first when `add` says so. */
  function openRoots(add: boolean) {
    managing = true
    addFirst = add
  }

  /** A Root chosen by hand: its history starts with nothing selected, in the mode already shown. */
  function chooseRoot(id: string) {
    shown.show(id)
    awaited = watched.roots?.some((r) => r.id === id) ? null : id
    managing = false
    selection = { path: null, snapshotId: null }
    arriving = false
    notice = ''
  }

  /** Opens the search overlay with what the field holds, unless it holds nothing. */
  function openSearch() {
    clearTimeout(searchPause)
    if (searchText.trim() !== '') searching = searchText
  }

  /** Typing opens the overlay once it pauses. */
  function onSearchInput() {
    clearTimeout(searchPause)
    searchPause = setTimeout(openSearch, SEARCH_PAUSE_MS)
  }

  /** Closing the overlay empties the field, so that typing there opens it afresh. */
  function closeSearch() {
    searching = null
    searchText = ''
  }

  /**
   * Takes the History screen to a search result. A result in a Root that is not registered, such as one
   * removed since the search, leads nowhere, and the header says so.
   */
  function pick(target: SearchTarget) {
    const into = shown.listed?.find((r) => r.id === target.root)
    closeSearch()
    if (!into) {
      notice = strings.search.rootGone
      return
    }
    notice = into.id === shown.id ? '' : strings.search.switchedRoot(into.path)
    shown.show(into.id)
    managing = false
    selection = { path: target.path, snapshotId: target.snapshot }
    arriving = true
    visit += 1
  }

  /** Shows the shown Root's history afresh, from nothing selected: what was selected may be erased. */
  function refreshHistory() {
    shown.acknowledgeErasure()
    selection = { path: null, snapshotId: null }
    arriving = false
    visit += 1
  }

  /* Painted from the cache as it is made, before the page first shows: the daemon's answer follows. */
  const preferences = providePreferences(new PreferencesState((e) => failureText(e, endSession)))

  $effect(() => {
    void preferences.load()
  })

  $effect(() => {
    void watched.load()
    const stopRegistry = watched.follow()
    const stopShown = shown.follow()
    return () => {
      stopShown()
      stopRegistry()
    }
  })

  /*
   * Whether the first Root was chosen. It is chosen when the Roots are first known, whichever load brings
   * them: an event arriving before the first answer, or a first load that failed, must not leave none.
   */
  let firstChosen = false
  $effect(() => {
    const roots = watched.roots
    if (firstChosen || roots === null) return
    firstChosen = true
    if (shown.id === null) shown.show(roots[0]?.id ?? null)
  })

  $effect(() => {
    if (awaited !== null && shown.current?.id === awaited) awaited = null
  })
</script>

<div class="app">
  <header>
    <b>{strings.appName}</b>
    {#if shown.listed && shown.listed.length > 0 && !ended}
      <label>
        {strings.rootLabel}
        <select value={shown.id ?? ''} onchange={(e) => chooseRoot(e.currentTarget.value)}>
          {#if !shown.current}<option value="" disabled>{strings.noRootChosen}</option>{/if}
          {#each shown.listed as r (r.id)}
            <option value={r.id}>
              {r.id === shown.id && shown.unwatched ? strings.roots.unwatchedOption(r.path) : r.path}
            </option>
          {/each}
        </select>
      </label>
      {#if shown.current && shown.unwatched}
        <span class="unwatched">{strings.roots.noLongerWatched}</span>
      {:else if shown.current}
        <span class="muted">{strings.roots.state(shown.current.state)}</span>
        {#if shown.current.state_code}
          {@const reason = strings.roots.reason(shown.current.state_code)}
          <span class="muted reason" title={reason}>{reason}</span>
        {/if}
      {/if}
    {/if}
    {#if watched.roots && !ended}
      <button
        type="button"
        aria-pressed={managing}
        onclick={() => {
          if (managing) managing = false
          else openRoots(false)
        }}
      >
        {strings.roots.manage}
      </button>
    {/if}
    {#if shown.listed && shown.listed.length > 0 && !ended}
      <input
        class="search"
        type="search"
        aria-label={strings.search.field}
        placeholder={strings.search.placeholder}
        bind:value={searchText}
        oninput={onSearchInput}
        onkeydown={(e) => {
          if (e.key === 'Enter') openSearch()
        }}
      />
    {/if}
    <span class="muted" role="status">{notice}</span>
    <span class="spacer"></span>
    <span class="error" role="status">
      {#if watched.roots && watched.error}{watched.error}{/if}
      {#if preferences.error}{strings.preferencesFailed[preferences.error.when]} {preferences.error.text}{/if}
    </span>
    <label>
      {strings.themeLabel}
      <select
        value={preferences.current.theme}
        onchange={(e) => void preferences.change({ theme: e.currentTarget.value })}
      >
        {#each THEMES as theme (theme)}
          <option value={theme}>{strings.themes[theme]}</option>
        {/each}
      </select>
    </label>
  </header>

  {#if ended}
    <p class="message">
      {strings.sessionEnded.before} <code>{strings.sessionEnded.command}</code>{strings.sessionEnded.after}
    </p>
  {:else if watched.roots === null}
    {#if watched.error}
      <p class="message error">{strings.failed} {watched.error}</p>
    {:else}
      <p class="message">{strings.loading}</p>
    {/if}
  {:else if managing}
    <RootsPage roots={watched.roots} current={shown.id} onOpen={chooseRoot} {addFirst} onSessionEnded={endSession} />
  {:else if shownId !== null}
    {#if shown.erasure && shown.current}
      <ErasedBanner
        path={shown.current.path}
        erasure={shown.erasure}
        onRefresh={refreshHistory}
        onManage={() => openRoots(false)}
      />
    {/if}
    <!--
      Keyed: another Root is another history, and a picked search result must be opened by the mode
      shown, which reads the selection only when it mounts.
    -->
    {#key `${shownId}:${visit}`}
      <History
        rootId={shownId}
        unwatched={shown.unwatched}
        bind:mode
        bind:selection
        bind:arriving
        onSessionEnded={endSession}
      />
    {/key}
  {:else if awaited !== null && awaited === shown.id}
    <p class="message">{strings.loading}</p>
  {:else if watched.roots.length === 0}
    <p class="message">
      {strings.noRoots}
      <button type="button" onclick={() => openRoots(false)}>{strings.roots.manage}</button>
      <button type="button" onclick={() => openRoots(true)}>{strings.roots.add}</button>
    </p>
  {:else}
    <p class="message">{strings.chooseRoot}</p>
  {/if}
</div>

{#if searching !== null && shown.listed}
  <SearchOverlay
    query={searching}
    roots={shown.listed}
    currentRoot={shown.id}
    onPick={pick}
    onClose={closeSearch}
    onSessionEnded={endSession}
  />
{/if}

<style>
  .app {
    height: 100vh;
    display: flex;
    flex-direction: column;
  }
  header {
    display: flex;
    gap: 12px;
    align-items: center;
    padding: 8px 14px;
    border-bottom: 1px solid var(--line);
  }
  .spacer {
    flex: 1;
  }
  header button {
    white-space: nowrap;
  }
  /*
   * The shown Root's state reason: one line, cut short, read in full from the tooltip. It gives way to the
   * rest of the header first, but never down to nothing.
   */
  .reason {
    min-width: 12ch;
    max-width: 32ch;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  /* The way to the Root management page reads as pressed while the page is shown. */
  header button[aria-pressed='true'] {
    background: var(--selected);
  }
  /* The shown Root's mark once it is no longer watched: its history is read-only from here on. */
  .unwatched {
    color: var(--warn);
    white-space: nowrap;
  }
  .search {
    width: min(280px, 30vw);
    font: inherit;
    color: inherit;
    background: var(--panel);
    border: 1px solid var(--line);
    border-radius: 6px;
    padding: 3px 8px;
  }
  label {
    display: flex;
    gap: 6px;
    align-items: center;
  }
  .message {
    padding: 24px;
  }
</style>
