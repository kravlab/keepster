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
   *
   * A Root's row can stop watching it too (#31), through the unwatch dialog. The page stays as it is once
   * the Root is unwatched: the live list drops it, and the dialog, which the page holds, closes by itself.
   *
   * A Root's row records a Move too (#32), through the Move dialog, its folder browser starting in the
   * directory above the Root's recorded path. The row's actions come in the order `lib/actions` gives: a
   * Root whose directory is missing (`root_missing`) offers the Move first, since that is most likely
   * why; otherwise it comes after Open history and before Unwatch…, which stays last. The page stays as
   * it is once the Move is recorded: the live list shows the new path and the state the daemon reports.
   */
  import { onMount } from 'svelte'
  import { rootActions } from '../lib/actions'
  import { AddingRoot } from '../lib/adding.svelte'
  import { failureText } from '../lib/failures'
  import { FolderBrowser, parentDirectory } from '../lib/folders.svelte'
  import { MovingRoot } from '../lib/moving.svelte'
  import { strings } from '../lib/strings'
  import { UnwatchingRoot } from '../lib/unwatching.svelte'
  import type { Root } from '../lib/wire'
  import AddDialog from './AddDialog.svelte'
  import AddedBanner from './AddedBanner.svelte'
  import MoveDialog from './MoveDialog.svelte'
  import UnwatchDialog from './UnwatchDialog.svelte'

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
  /* The unwatch under way, by the Root's id; held here, as the live list drops the Root once unwatched. */
  let unwatching = $state<UnwatchingRoot | null>(null)
  /**
   * The Root the unwatch dialog names: the live one, so that a Root moved meanwhile shows where it is
   * now, and the last one seen once the list drops it, so the dialog stays while a refusal or the close
   * it waits for is still to come.
   */
  const unwatchedRoot = $derived(
    unwatching ? (roots.find((r) => r.id === unwatching!.id) ?? unwatching.lastSeen) : null,
  )
  const added = $derived(adding?.step.kind === 'done' ? adding.step : null)
  /* The Move under way, and where its new path is chosen; a browser of its own, starting near the Root. */
  let moving = $state.raw<{ flow: MovingRoot; browser: FolderBrowser } | null>(null)
  /** The Root the Move dialog names: the live one, or the one the Move began with once the list drops it. */
  const movedRoot = $derived(moving ? (roots.find((r) => r.id === moving!.flow.root.id) ?? moving.flow.root) : null)

  /* Keeps the unwatched Root's last seen state current, for the dialog once the list has dropped it. */
  $effect(() => {
    if (!unwatching) return
    const id = unwatching.id
    const live = roots.find((r) => r.id === id)
    if (live) unwatching.lastSeen = live
  })

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

  /** Opens the unwatch dialog for `root`, which is the Root the dialog names until the list changes it. */
  function startUnwatching(root: Root) {
    unwatching = new UnwatchingRoot(root, (e) => failureText(e, onSessionEnded))
  }

  /**
   * Opens the Move dialog for `root`, its folder browser starting in the directory above the Root's
   * recorded path, which a Root whose directory is missing still has, or the nearest one above it that
   * can be listed, or at home when none can. The Move names an overlapped Root by its path from the live list.
   */
  function startMoving(root: Root) {
    const failure = (e: unknown) => failureText(e, onSessionEnded)
    moving = {
      flow: new MovingRoot(root, failure, (id) => roots.find((r) => r.id === id)),
      browser: new FolderBrowser(failure, parentDirectory(root.path) ?? undefined),
    }
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
            {#each rootActions(r) as action, at (action)}
              {#if action === 'move'}
                <button type="button" class:first={at === 0} onclick={() => startMoving(r)}>
                  {strings.roots.move}
                </button>
              {:else if action === 'history'}
                <button type="button" onclick={() => onOpen(r.id)}>{strings.roots.open}</button>
              {:else}
                <button type="button" onclick={() => startUnwatching(r)}>{strings.roots.unwatch}</button>
              {/if}
            {/each}
          </div>
          {#if r.state_code}
            <p class="reason">{strings.roots.reason(r.state_code)}</p>
          {/if}
        </li>
      {/each}
    </ul>
  {/if}
</section>

{#if unwatching && unwatchedRoot}
  <UnwatchDialog {unwatching} root={unwatchedRoot} onClose={() => (unwatching = null)} />
{/if}

{#if moving && movedRoot}
  <MoveDialog moving={moving.flow} root={movedRoot} browser={moving.browser} onClose={() => (moving = null)} />
{/if}

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
  /* The Move offered first, for a Root whose directory is missing: the way out of that state. */
  button.first {
    border-color: var(--accent);
    color: var(--accent);
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
