<script lang="ts">
  /*
   * A Snapshot's files as a tree, read a directory at a time (ticket 15). Each file says how the
   * Snapshot changed it, and a directory whether anything beneath it changed; what the Snapshot deleted is
   * shown where it was. Directories open on demand, and the ones above the selected file open by
   * themselves, so a file carried in from elsewhere is in view. Which directories are open survives a
   * change of Snapshot; what they hold is read again for the new one.
   */
  import { untrack } from 'svelte'
  import { SvelteMap, SvelteSet } from 'svelte/reactivity'
  import { directoryEntries } from '../lib/api'
  import { failureText } from '../lib/failures'
  import type { ShownFile } from '../lib/files'
  import { strings } from '../lib/strings'
  import { children, treeRows, type TreeNode } from '../lib/tree'

  let {
    rootId,
    snapshotId,
    changes,
    path = $bindable(),
    onSessionEnded,
  }: {
    rootId: string
    /** The Snapshot whose files are shown. */
    snapshotId: string
    /** Its changes, which mark the tree and add what it deleted. */
    changes: readonly ShownFile[]
    /** The selected file. */
    path: string | null
    onSessionEnded: () => void
  } = $props()

  let top = $state<TreeNode[] | null>(null)
  let error = $state('')
  const expanded = new SvelteSet<string>()
  const loaded = new SvelteMap<string, TreeNode[]>()
  /* Counts the Snapshots shown, so a directory read for an earlier one is dropped. */
  let generation = 0

  const rows = $derived(top ? treeRows(top, expanded, loaded) : [])

  /** Reads directory `dir` of the Snapshot shown into `loaded`, unless the Snapshot changed meanwhile. */
  async function load(dir: string, mine: number) {
    try {
      const nodes = children(dir, await directoryEntries(rootId, snapshotId, dir), changes)
      if (mine === generation) loaded.set(dir, nodes)
    } catch (e) {
      if (mine === generation) error = failureText(e, onSessionEnded)
    }
  }

  /* Each Snapshot is read afresh: its top, every open directory, and the directories above the selection. */
  $effect(() => {
    void snapshotId
    void changes
    // Only those two: choosing a file or opening a directory must not read the tree again.
    untrack(() => reread())
  })

  /** Reads the Snapshot shown afresh, opening the directories above the selected file first. */
  function reread() {
    const mine = ++generation
    top = null
    error = ''
    loaded.clear()
    if (path) {
      const parts = path.split('/').slice(0, -1)
      parts.forEach((_, i) => expanded.add(parts.slice(0, i + 1).join('/')))
    }
    void (async () => {
      try {
        const nodes = children('', await directoryEntries(rootId, snapshotId, ''), changes)
        if (mine !== generation) return
        top = nodes
        await Promise.all([...expanded].map((dir) => load(dir, mine)))
      } catch (e) {
        if (mine === generation) error = failureText(e, onSessionEnded)
      }
    })()
  }

  /** Opens or closes a directory, reading it the first time it opens. */
  function toggle(dir: string) {
    if (expanded.delete(dir)) return
    expanded.add(dir)
    if (!loaded.has(dir)) void load(dir, generation)
  }
</script>

{#if error}
  <p class="empty error">{strings.failed} {error}</p>
{:else if top === null}
  <p class="empty">{strings.loading}</p>
{/if}
<ul>
  {#each rows as { node, depth, loading } (node.path)}
    <li>
      {#if node.dir}
        <button
          class="node"
          style:padding-left="{10 + depth * 14}px"
          aria-expanded={expanded.has(node.path)}
          onclick={() => toggle(node.path)}
          title={node.path}
        >
          <span class="twisty">{expanded.has(node.path) ? '▾' : '▸'}</span>
          <span class="name" class:changed={node.changed}>{node.name}/</span>
          {#if loading}<span class="muted">{strings.loading}</span>{/if}
        </button>
      {:else}
        <button
          class="node"
          style:padding-left="{10 + depth * 14}px"
          aria-current={node.path === path}
          onclick={() => (path = node.path)}
          title={node.path}
        >
          <span
            class="mark {node.change ?? ''}"
            aria-label={node.change ? strings.changes[node.change].label : undefined}
          >
            {node.change ? strings.changes[node.change].mark : ''}
          </span>
          <span class="name">{node.name}</span>
        </button>
      {/if}
    </li>
  {/each}
</ul>

<style>
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .node {
    display: grid;
    grid-template-columns: 12px 1fr auto;
    gap: 6px;
    width: 100%;
    text-align: left;
    border: 0;
    border-radius: 0;
    background: none;
    padding: 4px 10px;
  }
  .node[aria-current='true'] {
    background: var(--selected);
  }
  .twisty {
    color: var(--muted);
  }
  .name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .name.changed {
    font-weight: 600;
  }
  .mark.added {
    color: var(--added);
  }
  .mark.deleted {
    color: var(--danger);
  }
  .mark.modified,
  .mark.renamed {
    color: var(--accent);
  }
  .empty {
    padding: 12px 10px;
    margin: 0;
    color: var(--muted);
  }
</style>
