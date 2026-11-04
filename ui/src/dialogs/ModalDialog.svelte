<script lang="ts">
  /*
   * The shell every modal dialog shares (Restore's, the add dialog, the unwatch dialog): a `<dialog>` card
   * opened with `showModal()` as it mounts, labelled by its title, over a dimmed page. Closing it any way —
   * Escape, a dialog's own Cancel through `dialog.close()` — is `onClose`; while `held`, Escape does
   * nothing, so a request on its way cannot be left unseen.
   *
   * The shell's styles for what a dialog draws inside it (its `.actions` row, the `.go` button, a disabled
   * button in that row) weigh no more than an element selector, so a dialog's own style of them wins; a
   * component nested in the dialog, such as the folder picker, keeps its own buttons' looks.
   */
  import { onMount, type Snippet } from 'svelte'

  let {
    title,
    width = 560,
    titleGap = 6,
    held = false,
    onCancel,
    onClose,
    dialog = $bindable(),
    children,
  }: {
    /** The heading, which names the dialog to assistive technology too. */
    title: string
    /** The card's width in pixels, never wider than the window allows. */
    width?: number
    /** The gap under the heading in pixels, for a dialog whose first content wants more air. */
    titleGap?: number
    /** Whether Escape is held back: something is on its way that the dialog must stay open for. */
    held?: boolean
    /** Called when Escape is pressed, held or not, before the dialog closes. */
    onCancel?: () => void
    /** Called when the dialog closes, however it does. */
    onClose: () => void
    /** The `<dialog>` element, for a dialog's own buttons to `close()` it. */
    dialog?: HTMLDialogElement
    children: Snippet
  } = $props()

  const titleId = $props.id()

  onMount(() => dialog?.showModal())
</script>

<dialog
  bind:this={dialog}
  style:width="min({width}px, calc(100vw - 32px))"
  onclose={onClose}
  oncancel={(e) => {
    if (held) e.preventDefault()
    onCancel?.()
  }}
  aria-labelledby={titleId}
>
  <h2 id={titleId} style:margin-bottom="{titleGap}px">{title}</h2>
  {@render children()}
</dialog>

<style>
  dialog {
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
    margin: 0 0 6px;
    font-size: 15px;
    overflow-wrap: anywhere;
  }
  dialog :global(:where(.actions)) {
    display: flex;
    gap: 8px;
    justify-content: flex-end;
    margin-top: 12px;
  }
  dialog :global(:where(.go:not(:disabled))) {
    border-color: var(--accent);
    color: var(--accent);
  }
  dialog :global(:where(.actions button:disabled)) {
    cursor: not-allowed;
    opacity: 0.5;
  }
</style>
