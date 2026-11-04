<script lang="ts">
  /*
   * The question a path through a link needs, adding a directory or recording a Move, as `keepster watch`
   * and `keepster move` ask it: is the Root the link, followed when it is pointed elsewhere, or the real
   * directory it leads to now? Drawn inside a dialog in place of the button that asked it, so the keyboard
   * goes on from the first answer. Backing out is the dialog's to handle, as it knows where the keyboard
   * goes next.
   */
  import { strings } from '../lib/strings'
  import type { PathMode } from '../lib/wire'

  let {
    path,
    realPath,
    onChoose,
    onCancel,
  }: {
    /** The path as it was sent. */
    path: string
    /** Where the link leads now. */
    realPath: string
    /** Called with the person's answer: the link, or the real directory. */
    onChoose: (mode: PathMode) => void
    /** Called when the person backs out of the question. */
    onCancel: () => void
  } = $props()
</script>

<p class="question">{strings.roots.link.question(path, realPath)}</p>
<div class="actions">
  <button type="button" onclick={onCancel}>{strings.cancel}</button>
  <button type="button" onclick={() => onChoose('canonical')}>{strings.roots.link.asReal}</button>
  <!-- The question replaces the button that asked it: the keyboard goes on from the first answer. -->
  <button type="button" class="go" onclick={() => onChoose('link')} {@attach (node) => node.focus()}>
    {strings.roots.link.asLink}
  </button>
</div>

<style>
  .question {
    margin: 8px 0 0;
    overflow-wrap: anywhere;
  }
</style>
