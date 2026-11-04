<script lang="ts">
  /*
   * The unwatch dialog (#31): a modal, as Restore's and the add dialog's are, that draws one Root's unwatch
   * from `lib/unwatching.svelte`. It says what becomes of the history, as `keepster unwatch` does — kept and
   * its last state pinned, or none to keep — and offers to erase it as well; choosing that asks a second,
   * separate acknowledgement that erasing cannot be undone, and the button, worded by the choice, is live
   * only once everything the choice needs is given.
   *
   * A refusal is shown with the daemon's reason and the dialog stays open, the choices as they were. On
   * success the dialog closes and the page stays where it is: the live list drops the Root by itself.
   * While the request is on its way the dialog cannot be closed, or a refusal would go unseen.
   */
  import { onMount } from 'svelte'
  import { strings } from '../lib/strings'
  import { wording } from '../lib/unwatch'
  import type { UnwatchingRoot } from '../lib/unwatching.svelte'
  import type { Root } from '../lib/wire'
  import FailureAlert from '../dialogs/FailureAlert.svelte'
  import ModalDialog from '../dialogs/ModalDialog.svelte'

  let {
    unwatching,
    root,
    onClose,
  }: {
    unwatching: UnwatchingRoot
    /** The Root as the live list has it now, or as it last had it once the list dropped it. */
    root: Root
    /** Called when the dialog is closed, unwatched or not. */
    onClose: () => void
  } = $props()

  let dialog = $state<HTMLDialogElement>()
  const consent = $derived(unwatching.consent)

  onMount(() => void unwatching.readHistory())

  /** Sends the request through the unwatch; closes once the Root is unwatched. */
  async function go() {
    await unwatching.go()
    if (unwatching.done) dialog?.close()
  }
</script>

<ModalDialog bind:dialog title={strings.unwatch.title(root.path)} held={unwatching.running} {onClose}>
  <p class="outlook" class:erasing={consent.erase}>{strings.unwatch.outlook[unwatching.outlook]}</p>

  <fieldset>
    <label>
      <input
        type="checkbox"
        checked={consent.erase}
        disabled={unwatching.running}
        onchange={(e) => unwatching.chooseErasing(e.currentTarget.checked)}
      />
      {strings.unwatch.erase}
    </label>
    {#if consent.erase}
      <label class="warn">
        <input
          type="checkbox"
          checked={consent.acknowledged}
          disabled={unwatching.running}
          onchange={(e) => unwatching.acknowledge(e.currentTarget.checked)}
        />
        {strings.unwatch.acknowledge}
      </label>
    {/if}
  </fieldset>

  {#if unwatching.error}
    <FailureAlert lead={strings.unwatch.failed} reason={unwatching.error} />
  {/if}

  <div class="actions">
    <button type="button" disabled={unwatching.running} onclick={() => dialog?.close()}>
      {strings.unwatch.cancel}
    </button>
    <button
      type="button"
      class="go"
      class:erasing={consent.erase}
      disabled={!unwatching.canGo}
      onclick={() => void go()}
    >
      {unwatching.running ? strings.unwatch.working : strings.unwatch[wording(consent)]}
    </button>
  </div>
</ModalDialog>

<style>
  p {
    margin: 8px 0 0;
  }
  .outlook {
    padding: 8px 10px;
    border-left: 3px solid var(--accent);
    background: var(--bg);
  }
  .outlook.erasing {
    border-left-color: var(--danger);
  }
  fieldset {
    margin: 10px 0 0;
    border: 1px solid var(--line);
    border-radius: 6px;
    padding: 6px 10px;
  }
  label {
    display: flex;
    gap: 6px;
    align-items: baseline;
    margin: 4px 0;
  }
  .warn {
    color: var(--warn);
  }
  .go.erasing:not(:disabled) {
    border-color: var(--danger);
    color: var(--danger);
  }
</style>
