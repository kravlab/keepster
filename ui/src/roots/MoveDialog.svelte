<script lang="ts">
  /*
   * The Move dialog (#32): a modal, as the add and unwatch dialogs are, that draws one Root's Move from
   * `lib/moving.svelte`, recording where the Root's directory is now as `keepster move` does. It says first
   * that Keepster moves no files and only records where the directory now is; the new path is chosen with
   * the folder picker, browsed or typed. Nothing is checked about what the directory holds: a Move is the
   * person's word, never inferred.
   *
   * A path through a link asks which the Root is, as the CLI does, and cancelling that question records
   * nothing. A refusal is shown with the daemon's reason and the dialog stays open, the path as it was. On
   * success the dialog closes and the page stays where it is: the live list shows the new path and the
   * state the daemon reports. While the request is on its way the dialog cannot be closed, or a refusal
   * would go unseen.
   */
  import { tick } from 'svelte'
  import type { FolderBrowser } from '../lib/folders.svelte'
  import type { MovingRoot } from '../lib/moving.svelte'
  import { strings } from '../lib/strings'
  import type { PathMode, Root } from '../lib/wire'
  import FailureAlert from '../dialogs/FailureAlert.svelte'
  import ModalDialog from '../dialogs/ModalDialog.svelte'
  import FolderPicker from '../folders/FolderPicker.svelte'
  import LinkQuestion from './LinkQuestion.svelte'

  let {
    moving,
    root,
    browser,
    onClose,
  }: {
    moving: MovingRoot
    /** The Root as the live list has it now, or as it was when the Move began once the list dropped it. */
    root: Root
    /** Where the new path is chosen; what its path field shows, once listed, is what is recorded. */
    browser: FolderBrowser
    /** Called when the dialog is closed, moved or not. */
    onClose: () => void
  } = $props()

  let dialog = $state<HTMLDialogElement>()
  let goButton = $state<HTMLButtonElement>()
  const step = $derived(moving.step)
  /** Whether the request is on its way: choosing, and closing, are held. */
  const sending = $derived(step.kind === 'moving')

  /** Closes the dialog once the Move is recorded; the live list brings the new path. */
  function closeIfDone() {
    if (moving.step.kind === 'done') dialog?.close()
  }

  /**
   * Records the path the path field shows: a path typed over the listing is listed first, and nothing is
   * sent when that is refused, so the path recorded is never other than the one shown.
   */
  async function moveChosen() {
    const path = await browser.chosenPath()
    if (path === null) return
    await moving.move(path)
    closeIfDone()
  }

  /** Answers the link question and sends the path again with the answer. */
  async function choose(mode: PathMode) {
    await moving.choose(mode)
    closeIfDone()
  }

  /** Declines the link question and hands the keyboard back to the button that asked it. */
  async function backOut() {
    moving.cancel()
    await tick()
    goButton?.focus()
  }
</script>

<ModalDialog bind:dialog title={strings.move.title(root.path)} width={600} titleGap={10} held={sending} {onClose}>
  <p class="note">{strings.move.note}</p>

  {#if step.kind === 'askingLink'}
    <LinkQuestion
      path={step.path}
      realPath={step.realPath}
      onChoose={(mode) => void choose(mode)}
      onCancel={() => void backOut()}
    />
  {/if}
  <!-- Hidden, not removed, while the link question is asked: the path field keeps what was typed. -->
  <div class="choosing" hidden={step.kind === 'askingLink'}>
    <FolderPicker {browser} disabled={sending} />

    {#if browser.path !== null}
      <p class="chosen">{strings.move.chosen} <code>{browser.path}</code></p>
    {/if}
    {#if step.kind === 'failed'}
      <FailureAlert lead={strings.move.failed} reason={step.reason} />
    {/if}

    <div class="actions">
      <button type="button" disabled={sending} onclick={() => dialog?.close()}>{strings.cancel}</button>
      <button
        type="button"
        class="go"
        bind:this={goButton}
        disabled={sending || !browser.canChoose}
        onclick={() => void moveChosen()}
      >
        {sending ? strings.move.working : strings.move.go}
      </button>
    </div>
  </div>
</ModalDialog>

<style>
  p {
    margin: 8px 0 0;
  }
  .note {
    margin: 0 0 10px;
    padding: 8px 10px;
    border-left: 3px solid var(--muted);
    background: var(--bg);
  }
  .chosen code {
    overflow-wrap: anywhere;
  }
  .chosen {
    padding: 6px 10px;
    border-left: 3px solid var(--accent);
    background: var(--bg);
  }
</style>
