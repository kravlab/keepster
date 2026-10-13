<script lang="ts">
  /*
   * File first, the History screen's second view mode (ticket 15, variant B): "this file — what happened
   * to it?". The Root's tree, as its newest Snapshot holds it, picks the file; a horizontal scrubber of the
   * Snapshots that changed that file picks the version, oldest on the left; the version's diff against the
   * Snapshot before it fills the rest, with Restore in its bar as in every mode.
   *
   * The file and the Snapshot are the screen's, bound in from `History`. A Snapshot carried in that did not
   * change the file shows the version the file had in it, and stays the screen's selection until the user
   * picks another version: the scrubber holds only versions, but a visit here must not move the Snapshot
   * the other modes return to.
   */
  import { tick, untrack } from 'svelte'
  import DiffPane from './DiffPane.svelte'
  import FileTree from './FileTree.svelte'
  import { changedFiles, entryAt, previousSnapshot, timeline } from '../lib/api'
  import { failureText } from '../lib/failures'
  import { fromDiff, fromEntries, type ShownFile } from '../lib/files'
  import { momentOf } from '../lib/ids'
  import { TimelinePages } from '../lib/pages.svelte'
  import { versionAt } from '../lib/selection'
  import { strings } from '../lib/strings'
  import type { Snapshot } from '../lib/wire'

  let {
    rootId,
    path = $bindable(),
    snapshotId = $bindable(),
    onSessionEnded,
  }: {
    /** Fixed for the life of the component: `History` mounts a new one for another Root. */
    rootId: string
    /** The selected file, carried across view modes. */
    path: string | null
    /** The selected Snapshot, carried across view modes. */
    snapshotId: string | null
    onSessionEnded: () => void
  } = $props()

  let newest = $state<string | null>(null)
  let error = $state('')
  let fromId = $state<string | null>(null)
  let file = $state<ShownFile | null>(null)
  let scrubber = $state<HTMLDivElement>()

  /** The text of a failure; a dead session also goes to the shell. */
  const failure = (e: unknown) => failureText(e, onSessionEnded)

  /* The chosen file's versions, newest first: a list of their own for each file, so none mixes two. */
  const pages = $derived.by(() => {
    const target = path
    return target === null ? null : new TimelinePages(rootId, failure, () => target)
  })
  const versions = $derived(pages?.snapshots ?? [])
  /* Whether the file's versions are all in that will be: a count or "no versions" says nothing before. */
  const settled = $derived(pages !== null && pages.loaded && !pages.loading)
  /* Oldest first, left to right, as time runs on a scrubber. */
  const ticks = $derived([...versions].reverse())
  /* The version shown: the one the selected Snapshot holds, judged once the versions are all in. */
  const selected = $derived.by(() => {
    if (!settled) return null
    const id = versionAt(versions, snapshotId)
    return versions.find((v) => v.id === id) ?? null
  })
  const toId = $derived(selected?.id ?? null)
  /* The shown version's place on the scrubber, oldest first; -1 while none is shown. */
  const at = $derived(ticks.findIndex((v) => v.id === toId))
  const hasNewer = $derived(at !== -1 && at < ticks.length - 1)
  const hasOlder = $derived(at > 0 || (at !== -1 && pages?.cursor !== undefined))

  /* The tree is the Root as its newest Snapshot holds it. */
  $effect(() => {
    timeline(rootId, undefined)
      .then((page) => (newest = page.snapshots[0]?.id ?? null))
      .catch((e: unknown) => (error = failure(e)))
  })

  /* Another file: its versions are loaded back to the version the selected Snapshot holds. */
  $effect(() => {
    const list = pages
    if (list) untrack(() => void list.reload(snapshotId))
  })

  /* New versions of the file go on the right as they are taken; `resync` loads them afresh. */
  $effect(() => pages?.follow({ until: () => snapshotId }))

  /*
   * The selected version as the diff pane shows it: against the Snapshot before it in the Root's history,
   * or whole for a Root's first Snapshot.
   */
  $effect(() => {
    const to = toId
    const target = path
    file = null
    if (to === null || target === null) return
    let cancelled = false
    void (async () => {
      try {
        const from = await previousSnapshot(rootId, to)
        let shown: ShownFile | undefined
        if (from) {
          shown = fromDiff(await changedFiles(rootId, from, to)).find((f) => f.path === target)
        } else {
          const entry = await entryAt(rootId, to, target)
          shown = entry ? fromEntries([entry])[0] : undefined
        }
        if (cancelled) return
        fromId = from
        file = shown ?? null
      } catch (e) {
        if (!cancelled) error = failure(e)
      }
    })()
    return () => {
      cancelled = true
    }
  })

  /**
   * Shows the next version along the scrubber, newer (1) or older (-1), loading the next older page past
   * its left end. The version chosen becomes the screen's Snapshot.
   */
  async function step(by: 1 | -1) {
    if (by === -1 && at === 0 && pages) await pages.older()
    // Read again: an older page moved every version along.
    const next = ticks[ticks.findIndex((v) => v.id === toId) + by]
    if (next) snapshotId = next.id
  }

  /** Steps along the scrubber with ← and →, focus following; the scrubber is one tab stop. */
  async function onScrubKey(event: KeyboardEvent) {
    const by = event.key === 'ArrowRight' ? 1 : event.key === 'ArrowLeft' ? -1 : 0
    if (by === 0) return
    event.preventDefault()
    await step(by)
    await tick()
    scrubber?.querySelector<HTMLElement>('[aria-current="true"]')?.focus()
  }

  /** How a version changed the file; a timeline filtered by path always says, `modified` stands in if not. */
  const changeOf = (v: Snapshot) => v.change ?? 'modified'
  /** A tick's name: the version's moment and how it changed the file. */
  const label = (v: Snapshot) => strings.versionTick(momentOf(v.id), changeOf(v))
</script>

<div class="cols">
  <aside class="tree" aria-label={strings.treeLabel}>
    {#if newest}
      <FileTree {rootId} snapshotId={newest} changes={[]} bind:path {onSessionEnded} />
    {:else if !error}
      <p class="empty">{strings.loading}</p>
    {/if}
  </aside>

  <section class="main">
    {#if error || pages?.error}<p class="empty error">{strings.failed} {error || pages?.error}</p>{/if}
    {#if path === null}
      <p class="empty">{strings.chooseFile}</p>
    {:else}
      <div class="head">
        <code class="path">{path}</code>
        {#if settled}<span class="muted">{strings.versions(versions.length, pages?.cursor !== undefined)}</span>{/if}
      </div>
      <div class="scrubber" role="group" aria-label={strings.scrubberLabel} bind:this={scrubber}>
        <button onclick={() => step(-1)} disabled={!hasOlder || pages?.paging}>{strings.olderVersion}</button>
        <div class="track">
          {#each ticks as v (v.id)}
            <button
              class="tick {changeOf(v)}"
              aria-current={v.id === toId}
              tabindex={v.id === toId ? 0 : -1}
              aria-label={label(v)}
              title={label(v)}
              onclick={() => (snapshotId = v.id)}
              onkeydown={onScrubKey}
            ></button>
          {/each}
        </div>
        <button onclick={() => step(1)} disabled={!hasNewer}>{strings.newerVersion}</button>
      </div>
      {#if selected}
        <p class="version">
          <b>{strings.at(momentOf(selected.id))}</b>
          <span class="mark {changeOf(selected)}">{strings.changes[changeOf(selected)].label}</span>
          <span class="muted">{selected.trigger}</span>
        </p>
      {:else if settled && versions.length === 0}
        <p class="empty">{strings.noVersions}</p>
      {/if}
      {#if toId}
        <DiffPane {rootId} from={fromId} to={toId} {file} {onSessionEnded} />
      {/if}
    {/if}
  </section>
</div>

<style>
  .cols {
    flex: 1;
    min-height: 0;
    display: grid;
    grid-template-columns: 280px 1fr;
  }
  .tree {
    border-right: 1px solid var(--line);
    background: var(--panel);
    overflow: auto;
    min-height: 0;
  }
  .main {
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
  }
  .head,
  .version {
    display: flex;
    gap: 10px;
    align-items: baseline;
    margin: 0;
    padding: 6px 12px;
  }
  .path {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .scrubber {
    display: flex;
    gap: 8px;
    align-items: center;
    padding: 6px 12px 10px;
    border-bottom: 1px solid var(--line);
  }
  .track {
    flex: 1;
    display: flex;
    gap: 3px;
    align-items: center;
    overflow-x: auto;
    padding: 4px 2px;
  }
  .tick {
    flex: 0 0 auto;
    width: 12px;
    height: 22px;
    padding: 0;
    border: 0;
    border-radius: 3px;
    opacity: 0.55;
  }
  .tick[aria-current='true'] {
    opacity: 1;
    outline: 2px solid var(--fg);
    outline-offset: 1px;
  }
  .tick:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
  }
  .tick.added {
    background: var(--added);
  }
  .tick.modified {
    background: var(--accent);
  }
  .tick.deleted {
    background: var(--danger);
  }
  .version {
    border-bottom: 1px solid var(--line);
  }
  .mark.added {
    color: var(--added);
  }
  .mark.deleted {
    color: var(--danger);
  }
  .mark.modified {
    color: var(--accent);
  }
  .empty {
    padding: 12px;
    margin: 0;
    color: var(--muted);
  }
</style>
