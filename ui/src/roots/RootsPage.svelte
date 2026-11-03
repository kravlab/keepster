<script lang="ts">
  /*
   * The Root management page (ticket 92): every Watched Root with its path, its state and, for a state
   * that is not `watching`, the reason in words. The list is the live one the shell keeps, so a Root
   * registered, unwatched or moved elsewhere shows here as it happens. Opening a Root's history is the
   * person's choice, from its row; the page never goes anywhere by itself.
   *
   * It adds a directory too (ticket 93), through the add dialog. Once added, the page stays as it is: the
   * new Root shows in the list as the live list brings it, and a banner says it was added, with the way to
   * its history and, while store maintenance cannot run, what `keepster watch` would warn of.
   */
  import { onMount } from 'svelte'
  import { AddingRoot } from '../lib/adding.svelte'
  import { failureText } from '../lib/failures'
  import { FolderBrowser } from '../lib/folders.svelte'
  import { strings } from '../lib/strings'
  import type { Root } from '../lib/wire'
  import AddDialog from './AddDialog.svelte'
  import AddedBanner from './AddedBanner.svelte'

  let {
    roots,
    current,
    onOpen,
    addFirst = false,
    onSessionEnded,
  }: {
    roots: Root[]
    /** The Root whose history the shell shows, if any; marked in the list. */
    current: string | null
    /** Called with a Root's id when its history is asked for. */
    onOpen: (id: string) => void
    /** Whether the page opens with the add dialog, as the empty state's way here asks. Read once. */
    addFirst?: boolean
    onSessionEnded: () => void
  } = $props()

  /* The add flow under way, and then its outcome for the banner, until dismissed or another add starts. */
  let adding = $state<AddingRoot | null>(null)
  /* Where the directory to add is chosen; kept across adds, so the browser reopens where it was. */
  let browser = $state<FolderBrowser | null>(null)
  const added = $derived(adding?.step.kind === 'done' ? adding.step : null)

  /**
   * Opens the add dialog with a fresh flow, the browser where it was but without a refusal from before.
   * The flow names an overlapped Root by its path from the live list.
   */
  function startAdding() {
    const failure = (e: unknown) => failureText(e, onSessionEnded)
    browser ??= new FolderBrowser(failure)
    browser.startOver()
    adding = new AddingRoot(failure, (id) => roots.find((r) => r.id === id))
  }

  onMount(() => {
    if (addFirst) startAdding()
  })
</script>

<section class="page" aria-labelledby="roots-title">
  <div class="top">
    <h1 id="roots-title">{strings.roots.title}</h1>
    <button type="button" onclick={startAdding}>{strings.roots.add}</button>
  </div>
  {#if added}
    <AddedBanner root={added.root} maintenance={added.maintenance} {onOpen} onDismiss={() => (adding = null)} />
  {/if}
  {#if roots.length === 0}
    <p class="muted">{strings.noRoots}</p>
  {:else}
    <ul>
      {#each roots as r (r.id)}
        <li class:current={r.id === current}>
          <div class="row">
            <code class="path">{r.path}</code>
            <span class="state {r.state}">{strings.roots.state(r.state)}</span>
            <span class="spacer"></span>
            <button type="button" onclick={() => onOpen(r.id)}>{strings.roots.open}</button>
          </div>
          {#if r.state_code}
            <p class="reason">{strings.roots.reason(r.state_code)}</p>
          {/if}
        </li>
      {/each}
    </ul>
  {/if}
</section>

{#if adding && browser && !added}
  <AddDialog {adding} {browser} onClose={() => (adding = null)} />
{/if}

<style>
  .page {
    width: min(880px, 100%);
    margin: 0 auto;
    padding: 24px 14px;
    overflow: auto;
  }
  h1 {
    font-size: 15px;
    margin: 0;
  }
  .top {
    display: flex;
    gap: 10px;
    align-items: center;
    justify-content: space-between;
    margin: 0 0 12px;
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    background: var(--panel);
    border: 1px solid var(--line);
    border-radius: 6px;
  }
  li {
    padding: 10px 12px;
  }
  li + li {
    border-top: 1px solid var(--line);
  }
  li.current {
    box-shadow: inset 3px 0 0 var(--accent);
  }
  .row {
    display: flex;
    gap: 10px;
    align-items: center;
  }
  .path {
    overflow-wrap: anywhere;
  }
  .spacer {
    flex: 1;
  }
  button {
    white-space: nowrap;
  }
  .state {
    font-size: 12px;
    padding: 0 6px;
    border: 1px solid currentColor;
    border-radius: 10px;
    white-space: nowrap;
  }
  .state.watching {
    color: var(--added);
  }
  .state.degraded {
    color: var(--warn);
  }
  .state.unavailable {
    color: var(--danger);
  }
  .reason {
    margin: 4px 0 0;
    color: var(--muted);
  }
</style>
