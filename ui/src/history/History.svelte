<script lang="ts">
  /*
   * The History screen: one screen, three view modes (ticket 15). A user picks the question rather than
   * the screen — "when did this break" (the rail, the default), "what happened to this file" (file
   * first), "what happened while I was away" (the activity feed). The selected file and Snapshot belong
   * to the screen and are carried across a switch, so a switch reads as continuity; what is local to a
   * mode (its filters, its scroll) stays in it. Restore looks the same in every mode: it lives in the diff
   * pane they all share.
   *
   * The mode and the selection belong to the shell (ticket 66), so that a search result can set them from
   * outside; a mode reads the selection when it mounts, and the shell remounts the screen to hand one in.
   */
  import FeedMode from './FeedMode.svelte'
  import FileMode from './FileMode.svelte'
  import RailMode from './RailMode.svelte'
  import { MODES, type Mode, type Selection } from '../lib/selection'
  import { strings } from '../lib/strings'

  let {
    rootId,
    mode = $bindable(),
    selection = $bindable(),
    arriving = $bindable(),
    onSessionEnded,
  }: {
    /** Fixed for the life of the screen: the shell mounts a new one for another Root. */
    rootId: string
    /** The view mode shown, kept by the shell across Roots. */
    mode: Mode
    /** The file and Snapshot carried across view modes. */
    selection: Selection
    /**
     * Whether the selection was just handed in by a search result: the feed then opens its diff. Choosing
     * another mode clears it, so a later switch to the feed opens nothing by itself.
     */
    arriving: boolean
    onSessionEnded: () => void
  } = $props()

  let tabs = $state<HTMLDivElement>()

  /** Shows mode `m`; a selection handed in by a search is an ordinary one from here on. */
  function choose(m: Mode) {
    mode = m
    arriving = false
  }

  /** Moves between the tabs with ← and →, as a tab list does. */
  function onTabKey(event: KeyboardEvent) {
    const step = event.key === 'ArrowRight' ? 1 : event.key === 'ArrowLeft' ? -1 : 0
    if (step === 0) return
    event.preventDefault()
    choose(MODES[(MODES.indexOf(mode) + step + MODES.length) % MODES.length] ?? mode)
    tabs?.querySelector<HTMLElement>(`[data-mode="${mode}"]`)?.focus()
  }
</script>

<div class="screen">
  <div class="tabs" role="tablist" aria-label={strings.modesLabel} bind:this={tabs}>
    {#each MODES as m (m)}
      <button
        role="tab"
        data-mode={m}
        aria-selected={mode === m}
        tabindex={mode === m ? 0 : -1}
        onclick={() => choose(m)}
        onkeydown={onTabKey}
      >
        {strings.modes[m]}
      </button>
    {/each}
  </div>

  <div class="body" role="tabpanel">
    {#if mode === 'rail'}
      <RailMode {rootId} bind:path={selection.path} bind:snapshotId={selection.snapshotId} {onSessionEnded} />
    {:else if mode === 'files'}
      <FileMode {rootId} bind:path={selection.path} bind:snapshotId={selection.snapshotId} {onSessionEnded} />
    {:else}
      <FeedMode
        {rootId}
        bind:path={selection.path}
        bind:snapshotId={selection.snapshotId}
        {arriving}
        {onSessionEnded}
      />
    {/if}
  </div>
</div>

<style>
  .screen {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .tabs {
    display: flex;
    gap: 2px;
    padding: 0 10px;
    border-bottom: 1px solid var(--line);
    background: var(--panel);
  }
  [role='tab'] {
    border: 0;
    border-bottom: 2px solid transparent;
    border-radius: 0;
    background: none;
    padding: 6px 10px;
  }
  [role='tab'][aria-selected='true'] {
    border-bottom-color: var(--accent);
    font-weight: 600;
  }
  [role='tab']:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: -2px;
  }
  .body {
    flex: 1;
    min-height: 0;
    display: flex;
  }
</style>
