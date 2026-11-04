<script lang="ts">
  /*
   * The add dialog (#29): a modal, as Restore's is, that draws the add flow of `lib/adding`. The
   * person chooses a directory with the folder picker and adds it; a refusal is shown with the daemon's
   * reason and the dialog stays open, to try again, choose another directory, or back out. A path through
   * a link asks which the Root is, as `keepster watch` does, and cancelling that question adds nothing.
   *
   * Before registering, the Capture Preview walks the directory (#30): the dialog shows the walk is
   * alive with its count of paths so far, and Cancel or Escape stops it. A directory past what one Root is
   * claimed to hold shows the preview's whole summary, as `keepster watch` does, and asks; declining adds
   * nothing. Leaving the dialog any other way while the walk runs stops it too, so no walk outlives the
   * dialog that started it.
   *
   * The page owns the flow, so the dialog leaves once the directory is added and the page shows the
   * outcome; closing it before then is backing out. While the request is on its way it cannot be closed:
   * the directory may be added whatever the dialog does, and the page must still say so.
   */
  import { onMount } from 'svelte'
  import type { AddingRoot } from '../lib/adding.svelte'
  import type { FolderBrowser } from '../lib/folders.svelte'
  import { shownPath } from '../lib/paths'
  import { strings } from '../lib/strings'
  import type { PreviewSize } from '../lib/wire'
  import FailureAlert from '../dialogs/FailureAlert.svelte'
  import { cancelThenFocus } from '../dialogs/focus'
  import ModalDialog from '../dialogs/ModalDialog.svelte'
  import FolderPicker from '../folders/FolderPicker.svelte'
  import LinkQuestion from './LinkQuestion.svelte'

  let {
    adding,
    browser,
    onClose,
  }: {
    adding: AddingRoot
    /** Where the directory is chosen; what its path field shows, once listed, is what is added. */
    browser: FolderBrowser
    /** Called when the person backs out. */
    onClose: () => void
  } = $props()

  let dialog = $state<HTMLDialogElement>()
  let addButton = $state<HTMLButtonElement>()
  const step = $derived(adding.step)
  const registering = $derived(step.kind === 'registering')
  const previewing = $derived(step.kind === 'previewing')
  /** Whether choosing is held: a preview or a request is on its way. */
  const held = $derived(registering || previewing)

  onMount(() => {
    return () => {
      if (adding.step.kind === 'previewing') adding.cancel()
    }
  })

  /**
   * Stops the walk, or declines the claim or link question, and hands the keyboard back to Add: the
   * controls that had it are gone once the person is choosing again.
   */
  function backOut() {
    return cancelThenFocus(
      () => adding.cancel(),
      () => addButton,
    )
  }

  /**
   * Adds the directory the path field shows: a path typed over the listing is listed first, and nothing is
   * added when that is refused, so the directory added is never other than the one shown.
   */
  async function addChosen() {
    const path = await browser.chosenPath()
    if (path !== null) await adding.add(path)
  }
</script>

{#snippet largest(title: string, files: PreviewSize[])}
  {#if files.length > 0}
    <h3>{title}</h3>
    <ul class="largest">
      {#each files as file (file.path)}
        <li><span class="size">{strings.byteSize(file.bytes)}</span> <code>{shownPath(file.path)}</code></li>
      {/each}
    </ul>
  {/if}
{/snippet}

<ModalDialog
  bind:dialog
  title={strings.roots.add}
  width={600}
  titleGap={10}
  {held}
  onCancel={() => {
    if (previewing) void backOut()
  }}
  {onClose}
>
  {#if step.kind === 'askingLink'}
    <LinkQuestion
      path={step.path}
      realPath={step.realPath}
      onChoose={(mode) => void adding.choose(mode)}
      onCancel={() => void backOut()}
    />
  {:else if step.kind === 'askingClaim'}
    {@const summary = step.summary}
    <p class="dialog-text question">{strings.adding.claim.question(step.path, summary.past_ceiling)}</p>
    <dl class="summary">
      <dt>{strings.adding.claim.captured}</dt>
      <dd>{strings.adding.claim.capturedTotals(summary.captured)}</dd>
      <dt>{strings.adding.claim.excluded}</dt>
      <dd>{strings.adding.claim.excludedTotals(summary.excluded)}</dd>
      {#each Object.entries(summary.withheld) as [reason, totals] (reason)}
        <dt>{strings.adding.claim.withheld}</dt>
        <dd>{strings.adding.claim.withheldTotals(reason, totals)}</dd>
      {/each}
      {#if summary.unreadable > 0}
        <dt>{strings.adding.claim.unreadable}</dt>
        <dd>{strings.adding.claim.unreadableCount(summary.unreadable)}</dd>
      {/if}
    </dl>
    {#if summary.groups.some((group) => group.rule)}
      <h3>{strings.adding.claim.groups}</h3>
      <ul class="groups">
        {#each summary.groups as group, at (at)}
          {#if group.rule}
            {#if group.files + group.directories > 0}
              <li>{strings.adding.claim.groupTotals(group, group.rule)}</li>
            {/if}
            {#each group.pruned ?? [] as path (path)}
              <li>{strings.adding.claim.groupPruned(group.class, shownPath(path), group.rule)}</li>
            {/each}
          {/if}
        {/each}
      </ul>
    {/if}
    {@render largest(strings.adding.claim.largest, summary.largest_captured)}
    {@render largest(strings.adding.claim.largestLeftOut, summary.largest_left_out)}
    <p class="dialog-text note">{strings.adding.claim.noGrowth}</p>
    <div class="actions">
      <!-- Declining is the safe answer, so the keyboard starts there. -->
      <button type="button" onclick={() => void backOut()} {@attach (node) => node.focus()}>
        {strings.adding.claim.decline}
      </button>
      <button type="button" class="go" onclick={() => void adding.watchAnyway()}>{strings.adding.claim.watch}</button>
    </div>
  {/if}
  <!-- Hidden, not removed, while a question is asked: the path field keeps what was typed. -->
  <div hidden={step.kind === 'askingLink' || step.kind === 'askingClaim'}>
    <FolderPicker {browser} disabled={held} />

    {#if browser.path !== null}
      <p class="dialog-text chosen">{strings.adding.chosen} <code>{browser.path}</code></p>
    {/if}
    {#if step.kind === 'failed'}
      <FailureAlert lead={strings.adding.failed} reason={step.reason} />
    {:else if step.kind === 'previewBusy'}
      <FailureAlert lead={strings.adding.failed} reason={strings.adding.preview.busy} />
    {/if}

    {#if step.kind === 'previewing'}
      <div class="walking">
        <p class="dialog-text" role="status">{strings.adding.preview.looking(step.path)}</p>
        <progress aria-hidden="true"></progress>
        <p class="dialog-text count">{strings.adding.preview.walked(step.walked)}</p>
      </div>
      <div class="actions">
        <!-- Add, which started the walk, is gone: the keyboard goes on from what stops it. -->
        <button type="button" onclick={() => void backOut()} {@attach (node) => node.focus()}>
          {strings.cancel}
        </button>
      </div>
    {:else}
      <div class="actions">
        <button type="button" disabled={registering} onclick={() => dialog?.close()}>{strings.cancel}</button>
        <button
          type="button"
          class="go"
          bind:this={addButton}
          disabled={registering || !browser.canChoose}
          onclick={() => void addChosen()}
        >
          {registering ? strings.adding.working : strings.adding.go}
        </button>
      </div>
    {/if}
  </div>
</ModalDialog>

<style>
  .chosen code,
  .question,
  .walking p,
  .summary dd,
  .groups li,
  .largest code {
    overflow-wrap: anywhere;
  }
  .walking progress {
    display: block;
    width: 100%;
    height: 4px;
    margin-top: 10px;
    accent-color: var(--accent);
  }
  .count {
    color: var(--muted);
    font-variant-numeric: tabular-nums;
  }
  .summary {
    display: grid;
    grid-template-columns: max-content 1fr;
    gap: 4px 12px;
    margin: 10px 0 0;
    padding: 8px 10px;
    border-left: 3px solid var(--accent);
    background: var(--bg);
  }
  .summary dt {
    color: var(--muted);
  }
  .summary dd {
    margin: 0;
  }
  h3 {
    margin: 12px 0 4px;
    font-size: 13px;
  }
  .groups,
  .largest {
    max-height: 160px;
    overflow-y: auto;
    margin: 0;
    padding: 0;
    list-style: none;
  }
  .groups li {
    padding: 2px 0;
  }
  .largest li {
    display: flex;
    gap: 10px;
    padding: 2px 0;
  }
  .size {
    flex: none;
    min-width: 7ch;
    color: var(--muted);
    text-align: right;
    font-variant-numeric: tabular-nums;
  }
  .note {
    color: var(--muted);
  }
  .chosen {
    padding: 6px 10px;
    border-left: 3px solid var(--accent);
    background: var(--bg);
  }
</style>
