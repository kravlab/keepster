<script lang="ts">
  /*
   * The application shell: the header with the Root picker, the search field and the theme, and the
   * History screen for that Root. The shell owns the page's preferences and provides them to every screen
   * below it.
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
  import History from './history/History.svelte'
  import SearchOverlay from './search/SearchOverlay.svelte'
  import { listRoots } from './lib/api'
  import { failureText } from './lib/failures'
  import { THEMES } from './lib/preferences'
  import { PreferencesState, providePreferences } from './lib/preferences.svelte'
  import { SEARCH_PAUSE_MS, type SearchTarget } from './lib/search'
  import type { Mode, Selection } from './lib/selection'
  import { strings } from './lib/strings'
  import type { Root } from './lib/wire'

  let roots = $state<Root[] | null>(null)
  let rootId = $state<string | null>(null)
  let error = $state('')
  let ended = $state(false)

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

  const root = $derived(roots?.find((r) => r.id === rootId) ?? null)

  /** Every screen calls this when the daemon answers 401. */
  function endSession() {
    ended = true
  }

  /** A Root chosen by hand: its history starts with nothing selected, in the mode already shown. */
  function chooseRoot(id: string) {
    rootId = id
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
    const into = roots?.find((r) => r.id === target.root)
    closeSearch()
    if (!into) {
      notice = strings.search.rootGone
      return
    }
    notice = into.id === rootId ? '' : strings.search.switchedRoot(into.path)
    rootId = into.id
    selection = { path: target.path, snapshotId: target.snapshot }
    arriving = true
    visit += 1
  }

  /* Painted from the cache as it is made, before the page first shows: the daemon's answer follows. */
  const preferences = providePreferences(new PreferencesState((e) => failureText(e, endSession)))

  $effect(() => {
    void preferences.load()
  })

  $effect(() => {
    listRoots()
      .then((list) => {
        roots = list
        rootId = list[0]?.id ?? null
      })
      .catch((e: unknown) => {
        error = failureText(e, endSession)
      })
  })
</script>

<div class="app">
  <header>
    <b>{strings.appName}</b>
    {#if roots && roots.length > 0 && !ended}
      <label>
        {strings.rootLabel}
        <select value={rootId} onchange={(e) => chooseRoot(e.currentTarget.value)}>
          {#each roots as r (r.id)}
            <option value={r.id}>{r.path}</option>
          {/each}
        </select>
      </label>
      {#if root}<span class="muted">{root.state}</span>{/if}
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
  {:else if error}
    <p class="message error">{strings.failed} {error}</p>
  {:else if roots === null}
    <p class="message">{strings.loading}</p>
  {:else if root}
    <!--
      Keyed: another Root is another history, and a picked search result must be opened by the mode
      shown, which reads the selection only when it mounts.
    -->
    {#key `${root.id}:${visit}`}
      <History rootId={root.id} bind:mode bind:selection bind:arriving onSessionEnded={endSession} />
    {/key}
  {:else}
    <p class="message">{strings.noRoots}</p>
  {/if}
</div>

{#if searching !== null && roots}
  <SearchOverlay
    query={searching}
    {roots}
    currentRoot={rootId}
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
