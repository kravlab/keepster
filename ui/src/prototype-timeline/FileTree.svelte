<script lang="ts">
  /*
   * PROTOTYPE (ticket 15) — a file tree over a Root's paths.
   *
   * Shared by variants A and B on purpose: a tree is a widget, like a header, and sharing one does not blur
   * the structural disagreement the variants exist to expose — *where* the tree sits and what it is scoped to
   * still differs. Variant C deliberately has none: a feed navigates by change, and a tree would turn it into A.
   *
   * Rendering is flat-with-indent rather than recursive components: collapsing is then a filter over rows, and
   * a 24-path fixture (or a 50,000-path Root) costs one array instead of a component per directory.
   */
  import type { Change } from '../fixtures/timeline'
  import { KIND } from './shared'

  let {
    paths,
    changes = [],
    selected,
    onselect,
    onlyChanged = $bindable(false),
    counts = null,
    scopeLabel = 'changed in this snapshot',
  }: {
    paths: string[]
    changes?: Change[]
    selected: string | null
    onselect: (path: string) => void
    onlyChanged?: boolean
    counts?: Record<string, number> | null
    scopeLabel?: string
  } = $props()

  interface Node {
    name: string
    path: string
    dir: boolean
    children: Node[]
  }

  const byPath = $derived(new Map(changes.map((c) => [c.path, c])))

  function build(list: string[]): Node[] {
    const root: Node = { name: '', path: '', dir: true, children: [] }
    for (const p of list) {
      const parts = p.split('/')
      let cur = root
      parts.forEach((name, i) => {
        const isFile = i === parts.length - 1
        const path = parts.slice(0, i + 1).join('/')
        let next = cur.children.find((c) => c.name === name && c.dir === !isFile)
        if (!next) {
          next = { name, path, dir: !isFile, children: [] }
          cur.children.push(next)
        }
        cur = next
      })
    }
    const sort = (n: Node) => {
      n.children.sort((a, b) => (a.dir === b.dir ? a.name.localeCompare(b.name) : a.dir ? -1 : 1))
      n.children.forEach(sort)
    }
    sort(root)
    return root.children
  }

  let collapsed = $state(new Set<string>())

  const visiblePaths = $derived(onlyChanged ? paths.filter((p) => byPath.has(p)) : paths)
  const tree = $derived(build(visiblePaths))

  interface Row {
    n: Node
    depth: number
  }

  function flatten(nodes: Node[], hidden: Set<string>, depth = 0, out: Row[] = []): Row[] {
    for (const n of nodes) {
      out.push({ n, depth })
      if (n.dir && !hidden.has(n.path)) flatten(n.children, hidden, depth + 1, out)
    }
    return out
  }

  const rows = $derived(flatten(tree, collapsed))

  function toggle(path: string) {
    const next = new Set(collapsed)
    if (next.has(path)) next.delete(path)
    else next.add(path)
    collapsed = next
  }

  /** A directory is marked when anything beneath it changed — otherwise a collapsed tree hides the news. */
  function dirMarked(path: string): boolean {
    for (const c of byPath.keys()) if (c.startsWith(path + '/')) return true
    return false
  }
</script>

<div class="tree">
  {#if changes.length > 0}
    <label class="filter">
      <input type="checkbox" bind:checked={onlyChanged} />
      only {scopeLabel}
    </label>
  {/if}
  <ul>
    {#each rows as { n, depth } (n.path)}
      {@const change = byPath.get(n.path)}
      <li style:padding-left="{6 + depth * 12}px">
        {#if n.dir}
          <button class="node dir" onclick={() => toggle(n.path)}>
            <span class="twisty">{collapsed.has(n.path) ? '▸' : '▾'}</span>
            <span class="name">{n.name}</span>
            {#if dirMarked(n.path) && collapsed.has(n.path)}<span class="dot">•</span>{/if}
          </button>
        {:else}
          <button class="node file" aria-pressed={n.path === selected} onclick={() => onselect(n.path)}>
            <span class="glyph" style:color={change ? KIND[change.kind].colour : 'transparent'}>
              {change ? KIND[change.kind].glyph : '·'}
            </span>
            <span class="name mono" class:dim={changes.length > 0 && !change}>{n.name}</span>
            {#if counts?.[n.path]}<span class="count">{counts[n.path]}</span>{/if}
          </button>
        {/if}
      </li>
    {/each}
  </ul>
  {#if rows.length === 0}
    <p class="empty">Nothing changed in this Snapshot.</p>
  {/if}
</div>

<style>
  .tree {
    display: flex;
    flex-direction: column;
    min-height: 0;
    height: 100%;
  }
  /* Fixed height for the same reason as variant A's rail filter: a wrapping label shifts everything below it. */
  .filter {
    display: flex;
    align-items: center;
    gap: 6px;
    height: 33px;
    padding: 0 10px;
    border-bottom: 1px solid var(--line);
    color: var(--muted);
    white-space: nowrap;
    overflow: hidden;
    flex: none;
  }
  .filter input {
    flex: none;
    margin: 0;
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 4px 0;
    overflow: auto;
    flex: 1;
  }
  .node {
    display: flex;
    gap: 6px;
    align-items: baseline;
    width: 100%;
    text-align: left;
    border: 0;
    border-radius: 0;
    background: none;
    padding: 2px 8px 2px 0;
  }
  .node[aria-pressed='true'] {
    background: #222835;
  }
  .dir .name {
    color: var(--muted);
  }
  .twisty {
    width: 10px;
    color: var(--muted);
  }
  .glyph {
    width: 10px;
    flex: none;
  }
  .name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .dim {
    color: #5c6472;
  }
  .count {
    margin-left: auto;
    padding-left: 8px;
    color: var(--muted);
    font-size: 11px;
  }
  .dot {
    color: var(--accent);
  }
  .empty {
    padding: 12px 10px;
    color: var(--muted);
  }
</style>
