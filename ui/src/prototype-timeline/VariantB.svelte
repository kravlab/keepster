<script lang="ts">
  /*
   * PROTOTYPE (ticket 15) — Variant B: the file comes first.
   *
   * Inverts A. You name the file, and time collapses into a horizontal scrubber above the diff: a faint band
   * of every Snapshot with the ones that touched this file marked. This is the direct test of the ticket's
   * question "is filtering to 'Snapshots where this file changed' enough navigation".
   *
   * Restore lives in an action bar that appears only while an older version is selected, so the destructive
   * control is never in the same place as the control you use to browse.
   */
  import DiffPane from './DiffPane.svelte'
  import FileTree from './FileTree.svelte'
  import { selection } from './selection.svelte'
  import { SNAPSHOTS, ROOT, NOW, TREE, touching, changeIn, type Snapshot } from '../fixtures/timeline'
  import { KIND, ago, clock, day, shortId, type Mode } from './shared'

  let path = $state<string>(selection.path ?? TREE[0])
  let mode = $state<Mode>('current')
  let wrap = $state(false)
  let armed = $state(new Set<string>())
  let confirming = $state(false)

  /* The tree carries each file's version count, which is the fact this variant is built around: how often a
     given file actually has history, rather than how busy the Root is. */
  const counts = $derived(Object.fromEntries(TREE.map((p) => [p, touching(p).length])))
  const versions = $derived(touching(path))
  /* Resume on the carried Snapshot when it touched this file; otherwise the newest version of it. The reset
     lives in the file-picking handler rather than an effect, so arriving with a cursor does not clobber it. */
  let index = $state(Math.max(0, touching(path).findIndex((s) => s.id === selection.snapshotId)))
  const current = $derived<Snapshot | undefined>(versions[Math.min(index, versions.length - 1)])
  const isLatest = $derived(index === 0)

  $effect(() => {
    selection.path = path
    if (current) selection.snapshotId = current.id
  })

  /* The scrubber shows *all* Snapshots as background density and this file's as ticks, so the user can see
     how rare a file's changes are against the whole history — the thing a filtered list hides. */
  const span = $derived({ from: SNAPSHOTS[SNAPSHOTS.length - 1].at, to: SNAPSHOTS[0].at })
  const pct = (at: number) => ((at - span.from) / (span.to - span.from)) * 100
</script>

<div class="wrap">
  <header class="top">
    <b>{ROOT.name}</b>
    <code class="mono muted">{ROOT.path}</code>
    <span class="spacer"></span>
    <b class="mono">{path}</b>
    <span class="muted">{versions.length} of {SNAPSHOTS.length} snapshots touched it</span>
  </header>

  <div class="cols">
  <aside class="treecol">
    <FileTree
      paths={TREE}
      selected={path}
      onselect={(p) => {
        path = p
        index = 0
      }}
      {counts}
    />
  </aside>

  <section class="main">
  <div class="scrubber">
    <div class="track">
      {#each SNAPSHOTS as s (s.id)}
        <i class="all" style:left="{pct(s.at)}%"></i>
      {/each}
      {#each versions as s, i (s.id)}
        <button
          class="tick"
          class:sel={current?.id === s.id}
          style:left="{pct(s.at)}%"
          style:background={KIND[changeIn(s, path)?.kind ?? 'modified'].colour}
          title="{day(s.at)} {clock(s.at)} — {changeIn(s, path)?.kind}"
          aria-label="version {i + 1}"
          onclick={() => (index = i)}
        ></button>
      {/each}
    </div>
    <div class="legend muted">
      <span>{day(span.from)}</span><span>now</span>
    </div>
  </div>

  <div class="bar">
    <div class="modes">
      {#each [['current', 'Δ Current'], ['prev', 'Δ Previous'], ['full', 'Full file']] as [k, label]}
        <button aria-pressed={mode === k} onclick={() => (mode = k as Mode)}>{label}</button>
      {/each}
    </div>
    <span class="spacer"></span>
    <button onclick={() => (index = Math.min(index + 1, versions.length - 1))}>← older</button>
    <button onclick={() => (index = Math.max(index - 1, 0))} disabled={isLatest}>newer →</button>
    <label class="wrapbox"><input type="checkbox" bind:checked={wrap} /> wrap</label>
  </div>

  {#if current}
    <p class="ctx">
      <b class="mono">{shortId(current.id)}</b>
      <span class="muted">{ago(current.at, NOW)} · {changeIn(current, path)?.kind ?? 'unchanged'}</span>
      {#if current.incomplete}<span class="warn">incomplete</span>{/if}
      {#if changeIn(current, path)?.reason}
        <span class="warn">withheld — {changeIn(current, path)?.reason}</span>
      {/if}
    </p>
  {/if}

  <DiffPane {path} {mode} {wrap} bind:armed />

  {#if !isLatest}
    <div class="action">
      <span>Viewing a version from {ago(current!.at, NOW)}.</span>
      {#if confirming}
        <span class="muted">A Safety Snapshot is taken first.</span>
        <button onclick={() => (confirming = false)}>Cancel</button>
        <button class="go" onclick={() => (confirming = false)}>Overwrite the working file</button>
      {:else}
        <button onclick={() => (confirming = true)}>Restore this version…</button>
        <button>Open in editor</button>
      {/if}
    </div>
  {/if}

  <footer class="state mono muted">
    root={ROOT.name} path={path} version={index + 1}/{versions.length} mode={mode} wrap={wrap ? 'on' : 'off'}
  </footer>
  </section>
  </div>
</div>

<style>
  .wrap { height: 100vh; display: flex; flex-direction: column; }
  .top { display: flex; gap: 14px; align-items: baseline; padding: 10px 14px; border-bottom: 1px solid var(--line); }
  .cols { flex: 1; min-height: 0; display: grid; grid-template-columns: 280px 1fr; }
  .treecol { border-right: 1px solid var(--line); background: var(--panel); min-height: 0; }
  .main { display: flex; flex-direction: column; min-width: 0; min-height: 0; }
  .scrubber { padding: 14px 14px 6px; border-bottom: 1px solid var(--line); }
  .track { position: relative; height: 34px; background: var(--panel); border: 1px solid var(--line); border-radius: 6px; }
  .all { position: absolute; top: 24px; width: 1px; height: 6px; background: #39404d; }
  .tick { position: absolute; top: 5px; width: 3px; height: 15px; padding: 0; border: 0; border-radius: 2px; cursor: pointer; }
  .tick.sel { top: 2px; height: 21px; width: 5px; outline: 2px solid #fff; }
  .legend { display: flex; justify-content: space-between; font-size: 11px; padding-top: 4px; }
  .bar { display: flex; gap: 6px; align-items: center; padding: 8px 14px; border-bottom: 1px solid var(--line); }
  .modes { display: flex; gap: 4px; }
  .spacer { flex: 1; }
  .wrapbox { color: var(--muted); }
  .ctx { display: flex; gap: 10px; align-items: baseline; margin: 0; padding: 6px 14px; border-bottom: 1px solid var(--line); }
  .warn { color: #d29922; }
  .action { display: flex; gap: 10px; align-items: center; padding: 10px 14px; border-top: 1px solid var(--line); background: #241f1a; }
  .go { border-color: #ff7b72; color: #ff7b72; }
  .state { padding: 4px 14px; border-top: 1px solid var(--line); font-size: 11px; }
</style>
