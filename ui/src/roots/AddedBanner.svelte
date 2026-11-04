<script lang="ts">
  /*
   * The banner once a directory is added (ticket 93): it says so, offers the way to the new Root's history,
   * and, while store maintenance cannot run, what `keepster watch` would warn of. The page stays where it
   * is; the new Root shows in its list as the live list brings it. It is edged in the color a watched
   * Root's state has.
   */
  import Banner from '../banners/Banner.svelte'
  import { maintenanceNotice } from '../lib/maintenance'
  import { strings } from '../lib/strings'
  import type { GitStatus, Root } from '../lib/wire'

  let {
    root,
    maintenance,
    onOpen,
    onDismiss,
  }: {
    /** The Root the directory now is. */
    root: Root
    /** The daemon's `git` while store maintenance is unavailable; `null` leaves the notice out. */
    maintenance: GitStatus | null
    /** Called with the Root's id when its history is asked for. */
    onOpen: (id: string) => void
    /** Called when the person dismisses the banner. */
    onDismiss: () => void
  } = $props()
</script>

<!-- Focused as it appears: the dialog the keyboard was in is gone. -->
<Banner tone="added" margin="0 0 12px" focused>
  {#snippet message()}{strings.adding.added(root.path)}{/snippet}
  {#snippet actions()}
    <button type="button" onclick={() => onOpen(root.id)}>{strings.adding.open}</button>
    <button type="button" onclick={onDismiss}>{strings.adding.dismiss}</button>
  {/snippet}
  {#snippet details()}
    {#if maintenance}
      {@const notice = maintenanceNotice(maintenance)}
      <div class="maintenance">
        <p><strong>{notice.title}</strong></p>
        {#if notice.git}<p>{notice.git}</p>{/if}
        {#if notice.why}<p>{notice.why}</p>{/if}
        {#if notice.gitSaid}<p>{notice.gitSaid}</p>{/if}
        {#if notice.install}
          {#if notice.install.command}
            <p>{strings.maintenance.install} <code>{notice.install.command}</code></p>
          {/if}
          {#if notice.install.note}<p>{notice.install.note}</p>{/if}
          <p><a href={notice.install.link} target="_blank" rel="noreferrer">{notice.install.link}</a></p>
        {/if}
      </div>
    {/if}
  {/snippet}
</Banner>

<style>
  /* Drawn here, below the banner's row: the banner's own paragraph rule does not reach it. */
  .maintenance p {
    margin: 0;
    overflow-wrap: anywhere;
  }
  .maintenance {
    margin-top: 8px;
    padding-top: 8px;
    border-top: 1px solid var(--line);
    color: var(--warn);
  }
  .maintenance p + p {
    margin-top: 2px;
  }
  .maintenance code,
  .maintenance a {
    color: var(--fg);
  }
</style>
