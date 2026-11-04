<script lang="ts">
  /*
   * The skeleton every banner shares: the one once a directory is added, and the one once the shown Root's
   * history is erased. A banner says what just happened over a screen that stays where it is: a panel
   * edged on the left in its tone's color, its message and its buttons on one row, the buttons at the
   * row's end, and anything more below that row. It is a status, read out as it appears.
   */
  import type { Snippet } from 'svelte'

  let {
    tone,
    margin,
    focused = false,
    message,
    actions,
    details,
  }: {
    /** The color of the left edge: `added` for what was added, `warn` for what is no longer kept. */
    tone: 'added' | 'warn'
    /** The space around the banner, as CSS `margin` gives it: where it sits is its screen's business. */
    margin: string
    /** Whether the banner takes the focus as it appears, for when what the keyboard was in is gone. */
    focused?: boolean
    /** The message, inside the row's paragraph. */
    message: Snippet
    /** The buttons at the row's end. */
    actions: Snippet
    /** Anything more, below the row. */
    details?: Snippet
  } = $props()
</script>

<!-- Made focusable, out of the tab order, only when it takes the focus. -->
<div
  class={['banner', tone]}
  style:margin
  role="status"
  {@attach focused &&
    ((node) => {
      node.tabIndex = -1
      node.focus()
    })}
>
  <div class="row">
    <p>{@render message()}</p>
    <span class="spacer"></span>
    {@render actions()}
  </div>
  {@render details?.()}
</div>

<style>
  .banner {
    padding: 8px 12px;
    background: var(--panel);
    border: 1px solid var(--line);
    border-left: 3px solid;
    border-radius: 6px;
  }
  .added {
    border-left-color: var(--added);
  }
  .warn {
    border-left-color: var(--warn);
  }
  .banner:focus-visible {
    outline-offset: 0;
  }
  .row {
    display: flex;
    gap: 10px;
    align-items: center;
  }
  p {
    margin: 0;
    overflow-wrap: anywhere;
  }
  .spacer {
    flex: 1;
  }
  /* The actions are the banner's caller's, drawn into its row. */
  .row > :global(button) {
    white-space: nowrap;
  }
</style>
