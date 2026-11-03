<script lang="ts">
  /*
   * The add dialog (ticket 93): a modal, as Restore's is, that draws the add flow of `lib/adding`. The
   * person chooses a directory with the folder picker and adds it; a refusal is shown with the daemon's
   * reason and the dialog stays open, to try again, choose another directory, or back out. A path through
   * a link asks which the Root is, as `keepster watch` does, and cancelling that question adds nothing.
   *
   * The page owns the flow, so the dialog leaves once the directory is added and the page shows the
   * outcome; closing it before then is backing out. While the request is on its way it cannot be closed:
   * the directory may be added whatever the dialog does, and the page must still say so.
   */
  import { onMount } from 'svelte'
  import type { AddingRoot } from '../lib/adding.svelte'
  import type { FolderBrowser } from '../lib/folders.svelte'
  import { strings } from '../lib/strings'
  import FolderPicker from '../folders/FolderPicker.svelte'

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
  const step = $derived(adding.step)
  const registering = $derived(step.kind === 'registering')

  onMount(() => {
    dialog?.showModal()
  })

  /**
   * Adds the directory the path field shows: a path typed over the listing is listed first, and nothing is
   * added when that is refused, so the directory added is never other than the one shown.
   */
  async function addChosen() {
    const path = await browser.chosenPath()
    if (path !== null) await adding.add(path)
  }
</script>

<dialog
  bind:this={dialog}
  onclose={onClose}
  oncancel={(e) => {
    if (registering) e.preventDefault()
  }}
  aria-labelledby="add-title"
>
  <h2 id="add-title">{strings.roots.add}</h2>

  {#if step.kind === 'askingLink'}
    <p class="question">{strings.adding.link.question(step.path, step.realPath)}</p>
    <div class="actions">
      <button type="button" onclick={() => adding.cancel()}>{strings.adding.cancel}</button>
      <button type="button" onclick={() => void adding.choose('canonical')}>{strings.adding.link.canonical}</button>
      <!-- The question replaces the button that asked it: the keyboard goes on from the first answer. -->
      <button type="button" class="go" onclick={() => void adding.choose('link')} {@attach (node) => node.focus()}>
        {strings.adding.link.link}
      </button>
    </div>
  {/if}
  <!-- Hidden, not removed, while the link question is asked: the path field keeps what was typed. -->
  <div hidden={step.kind === 'askingLink'}>
    <FolderPicker {browser} disabled={registering} />

    {#if browser.path !== null}
      <p class="chosen">{strings.adding.chosen} <code>{browser.path}</code></p>
    {/if}
    {#if step.kind === 'failed'}
      <!-- Focused as it appears: the button that sent the request is still where the keyboard was. -->
      <p class="error" role="alert" tabindex="-1" {@attach (node) => node.focus()}>
        {strings.adding.failed}
        {step.reason}
      </p>
    {/if}

    <div class="actions">
      <button type="button" disabled={registering} onclick={() => dialog?.close()}>{strings.adding.cancel}</button>
      <button type="button" class="go" disabled={registering || !browser.canChoose} onclick={() => void addChosen()}>
        {registering ? strings.adding.working : strings.adding.go}
      </button>
    </div>
  </div>
</dialog>

<style>
  dialog {
    width: min(600px, calc(100vw - 32px));
    border: 1px solid var(--line);
    border-radius: 8px;
    padding: 16px 18px;
    background: var(--panel);
    color: var(--fg);
  }
  dialog::backdrop {
    background: rgb(0 0 0 / 0.35);
  }
  h2 {
    margin: 0 0 10px;
    font-size: 15px;
  }
  p {
    margin: 8px 0 0;
  }
  .chosen code,
  .question {
    overflow-wrap: anywhere;
  }
  .chosen {
    padding: 6px 10px;
    border-left: 3px solid var(--accent);
    background: var(--bg);
  }
  .actions {
    display: flex;
    gap: 8px;
    justify-content: flex-end;
    margin-top: 12px;
  }
  .go:not(:disabled) {
    border-color: var(--accent);
    color: var(--accent);
  }
  button:disabled {
    cursor: not-allowed;
    opacity: 0.5;
  }
</style>
