<script lang="ts">
  /*
   * The banner over the History screen once the shown Root's history was erased (#33), from this tab,
   * another or the CLI. The screen stays where it is: what it shows is gone from the daemon, and the banner
   * says so, edged in the warning color. Its one button shows the history afresh while the Root is still
   * watched, its history starting over, and leads to the management page once it is not.
   */
  import Banner from '../banners/Banner.svelte'
  import type { Erasure } from '../lib/erasure'
  import { strings } from '../lib/strings'

  let {
    path,
    erasure,
    onRefresh,
    onManage,
  }: {
    /** The Root's directory, as its last known record names it. */
    path: string
    /** What the banner offers. */
    erasure: Erasure
    /** Called to show the history afresh, while the Root is watched. */
    onRefresh: () => void
    /** Called to open the management page, once the Root is not watched. */
    onManage: () => void
  } = $props()
</script>

<Banner tone="warn" margin="8px 10px 0">
  {#snippet message()}
    <strong>{strings.erased.title(path)}</strong>
    {strings.erased.why[erasure]}
  {/snippet}
  {#snippet actions()}
    <button type="button" onclick={erasure === 'refresh' ? onRefresh : onManage}>{strings.erased.go[erasure]}</button>
  {/snippet}
</Banner>
