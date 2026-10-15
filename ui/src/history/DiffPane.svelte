<script lang="ts">
  /*
   * One file of the selected Snapshot: its diff against the Snapshot before it, from the daemon's hunks
   * (ticket 09), or the file alone in a Root's first Snapshot. The pane has its own bar, holding the
   * wrap toggle (off by default, tickets 09 and 15) and Restore, whose dialog is the same in every view
   * mode. A card stands in for the editor when there is no text to compare. The editor module is imported
   * only when a file is shown, keeping it out of the eager bundle.
   *
   * Wrapping and the long-line limit are the daemon's preferences (ticket 66): the toggle and the limit's
   * field in the highlighting note write them, and every pane follows.
   */
  import { untrack } from 'svelte'
  import RestoreDialog from './RestoreDialog.svelte'
  import type { Shown } from '../editor/cm6'
  import { fileContent, fileDiff } from '../lib/api'
  import { failureText } from '../lib/failures'
  import type { DiffTarget } from '../lib/files'
  import { hasLongLine } from '../lib/lines'
  import { baseName, shownPath } from '../lib/paths'
  import { SYNTAX_LINE_LIMITS } from '../lib/preferences'
  import { usePreferences } from '../lib/preferences.svelte'
  import { strings } from '../lib/strings'

  let {
    rootId,
    target,
    onSessionEnded,
  }: {
    rootId: string
    /** The Snapshot, its predecessor and the file, given together; `null` while none is known yet. */
    target: DiffTarget | null
    onSessionEnded: () => void
  } = $props()

  /* Read off the one value, so the three always belong to the same selection (ticket 83). */
  const from = $derived(target?.from ?? null)
  const to = $derived(target?.to ?? null)
  const file = $derived(target?.file ?? null)

  const preferences = usePreferences()

  let host = $state<HTMLDivElement>()
  let status = $state<'idle' | 'loading' | 'ready' | 'error'>('idle')
  let error = $state('')
  let plain = $state(false)
  let restoring = $state(false)
  let shown: Shown | null = null

  /*
   * The version Restore writes back is the one the pane shows on its newer side, except for a deleted
   * file, which the Snapshot that deleted it does not hold: that one comes from the Snapshot before.
   */
  const deleted = $derived(file?.change === 'deleted')
  const restoreFrom = $derived(deleted && from !== null ? from : to)

  /* Mounts the editor for the selected file; a newer selection cancels an older one still loading. */
  $effect(() => {
    const el = host
    const chosen = file && !file.card ? file : null
    const older = from
    const newer = to
    /* Tracked: a changed limit decides afresh whether the file is highlighted. */
    const limit = preferences.current.syntax_line_limit
    if (!el || !chosen || newer === null) {
      status = 'idle'
      return
    }
    let cancelled = false
    status = 'loading'
    error = ''

    void (async () => {
      try {
        const editor = import('../editor/cm6')
        const wrapped = untrack(() => preferences.current.wrap_lines)
        let mount: (el: HTMLElement) => Promise<Shown>
        if (older === null || chosen.change === 'unchanged') {
          const doc = await fileContent(rootId, newer, chosen.path)
          plain = hasLongLine(doc, limit)
          const { showFile } = await editor
          mount = (el) => showFile({ el, path: chosen.path, doc, wrap: wrapped, highlight: !plain })
        } else {
          const [diff, a, b] = await Promise.all([
            fileDiff(rootId, older, newer, chosen.path),
            chosen.change === 'added' ? '' : fileContent(rootId, older, chosen.oldPath ?? chosen.path),
            chosen.change === 'deleted' ? '' : fileContent(rootId, newer, chosen.path),
          ])
          plain = hasLongLine(a, limit) || hasLongLine(b, limit)
          const { showDiff } = await editor
          mount = (el) => showDiff({ el, path: chosen.path, a, b, hunks: diff.hunks, wrap: wrapped, highlight: !plain })
        }
        if (cancelled) return
        const mounted = await mount(el)
        if (cancelled) {
          mounted.destroy()
          return
        }
        shown = mounted
        status = 'ready'
      } catch (e) {
        if (cancelled) return
        status = 'error'
        error = failureText(e, onSessionEnded)
      }
    })()

    return () => {
      cancelled = true
      shown?.destroy()
      shown = null
      el.replaceChildren()
    }
  })

  /* Wrapping is switched in place: remounting would lose the reader's scroll position. */
  $effect(() => {
    const on = preferences.current.wrap_lines
    untrack(() => shown?.setWrap(on))
  })
</script>

<section class="pane">
  <div class="bar">
    {#if file}
      <code class="path">{shownPath(file.path)}</code>
      {#if file.oldPath}<span class="muted">{strings.renamedFrom(shownPath(file.oldPath))}</span>{/if}
    {/if}
    <span class="spacer"></span>
    {#if file && file.card !== 'absent'}
      <button onclick={() => (restoring = true)}>{strings.restore.open}</button>
    {/if}
    <label class="wrap">
      <input
        type="checkbox"
        checked={preferences.current.wrap_lines}
        onchange={(e) => void preferences.change({ wrap_lines: e.currentTarget.checked })}
      />
      {strings.wrapLines}
    </label>
  </div>

  {#if file?.change === 'unchanged' && file.card !== 'absent'}
    <p class="note muted">{strings.unchangedHere(shownPath(baseName(file.path)))}</p>
  {/if}
  {#if status === 'ready' && plain}
    <p class="note">
      {strings.highlightOff.before}
      <input
        class="limit"
        type="number"
        min={SYNTAX_LINE_LIMITS.min}
        max={SYNTAX_LINE_LIMITS.max}
        aria-label={strings.highlightOff.field}
        value={preferences.current.syntax_line_limit}
        onchange={(e) => void preferences.change({ syntax_line_limit: e.currentTarget.valueAsNumber })}
      />
      {strings.highlightOff.after}
    </p>
  {/if}

  {#if !file}
    <p class="empty">{strings.selectFile}</p>
  {:else if file.card}
    <p class="empty card">{strings.cards[file.card]}</p>
  {:else if status === 'loading'}
    <p class="empty">{strings.loading}</p>
  {:else if status === 'error'}
    <p class="empty error">{strings.failed} {error}</p>
  {/if}
  <div class="host" bind:this={host} hidden={!file || !!file.card || status === 'error'}></div>
</section>

{#if restoring && file && restoreFrom}
  <RestoreDialog
    {rootId}
    from={restoreFrom}
    path={file.path}
    beforeDeletion={deleted}
    onClose={() => (restoring = false)}
    {onSessionEnded}
  />
{/if}

<style>
  .pane {
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
  }
  .bar {
    display: flex;
    gap: 8px;
    align-items: center;
    padding: 6px 10px;
    border-bottom: 1px solid var(--line);
    background: var(--panel);
  }
  .path {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .spacer {
    flex: 1;
  }
  .wrap {
    display: flex;
    gap: 4px;
    align-items: center;
    white-space: nowrap;
  }
  .note {
    margin: 0;
    padding: 4px 10px;
    border-bottom: 1px solid var(--line);
    color: var(--warn);
  }
  .limit {
    width: 9ch;
    font: inherit;
    color: inherit;
    background: var(--panel);
    border: 1px solid var(--line);
    border-radius: 4px;
    padding: 0 4px;
  }
  .host {
    flex: 1;
    min-height: 0;
    overflow: hidden;
  }
  .empty {
    padding: 24px;
    color: var(--muted);
  }
  .card {
    color: var(--fg);
  }
</style>
