<script lang="ts">
  /*
   * PROTOTYPE (ticket 15) — Variant A: the left rail.
   *
   * The conventional local-history shape: time is a permanent vertical column, the Snapshot is the unit you
   * select first, and the file list is scoped to it. Tests whether a rail stays legible at hundreds of
   * Snapshots, and makes "a Snapshot that only deleted files" a first-class row rather than an empty diff.
   *
   * Restore is deliberately two steps and never adjacent to navigation: it opens a preflight panel modelled
   * on ticket 10 (dry run, per-reason confirmation) before anything claims to write.
   */
  import DiffPane from './DiffPane.svelte'
  import FileTree from './FileTree.svelte'
  import { selection } from './selection.svelte'
  import { SNAPSHOTS, ROOT, NOW, TREE, changeIn, type Snapshot } from '../fixtures/timeline'
  import { KIND, TRIGGER, ago, clock, day, deletionOnly, shortId, summarise, type Mode } from './shared'

  /*
   * A is Snapshot-first, so a file arriving from a file-first variant has to be resolved to a Snapshot.
   * In order: the Snapshot the other variant was showing; failing that, the newest one that actually touched
   * the carried file; failing that, the newest of all. Without the middle step A opens on a Snapshot whose
   * file column cannot contain the file the user just chose, which reads as a reset.
   */
  let selected = $state<Snapshot>(
    SNAPSHOTS.find((s) => s.id === selection.snapshotId) ??
      (selection.path ? SNAPSHOTS.find((s) => s.changes.some((c) => c.path === selection.path)) : undefined) ??
      SNAPSHOTS[0],
  )
  let path = $state<string | null>(selection.path ?? SNAPSHOTS[0].changes[0]?.path ?? null)
  /* The flat list is A's baseline — it is the Snapshot's changes, nothing else. The tree is a view *option*
     over the same column, and only it needs the "widen to the whole Root" escape hatch. */
  let view = $state<'list' | 'tree'>('list')
  let onlyChanged = $state(true)

  $effect(() => {
    selection.path = path
    selection.snapshotId = selected.id
  })
  let mode = $state<Mode>('current')
  let wrap = $state(false)
  let menu = $state(false)
  let preflight = $state(false)
  let armed = $state(new Set<string>())
  let filterToFile = $state(false)

  const list = $derived(
    filterToFile && path ? SNAPSHOTS.filter((s) => s.changes.some((c) => c.path === path)) : SNAPSHOTS,
  )

  /* Day headings are what keep 400 rows readable; without them the rail is an undifferentiated column. */
  const rows = $derived(
    list.map((s, i) => ({ s, head: i === 0 || day(s.at) !== day(list[i - 1].at) ? day(s.at) : null })),
  )

  /* With the rail filtered to one file, the selected Snapshot can fall out of the list entirely — that is
     what picking a file the current Snapshot never touched does. Leaving it selected highlights a row that
     is not on screen, so the selection moves to the newest Snapshot the filter does contain. Only while the
     filter is on: unfiltered, an off-list selection is impossible. */
  $effect(() => {
    if (filterToFile && list.length > 0 && !list.some((s) => s.id === selected.id)) selected = list[0]
  })

  /* The selected path survives a Snapshot change: the tree shows the whole Root, so there is no longer a
     reason to jump the user to some other file just because this Snapshot did not touch theirs. */
  function select(s: Snapshot) {
    selected = s
  }
</script>

<div class="wrap">
  <header class="top">
    <b>{ROOT.name}</b>
    <code class="mono muted">{ROOT.path}</code>
    <span class="muted">{ROOT.state} · {SNAPSHOTS.length} snapshots</span>
  </header>

  <div class="cols">
    <aside class="rail">
      <label class="filter" title={path ?? ''}>
        <input type="checkbox" bind:checked={filterToFile} disabled={!path} />
        <span class="filterlabel">
          only where <code class="mono">{path?.split('/').pop() ?? '…'}</code> changed
        </span>
      </label>
      <ol>
        {#each rows as { s, head } (s.id)}
          {#if head}<li class="day">{head}</li>{/if}
          <li>
            <button class="row" aria-pressed={s.id === selected.id} onclick={() => select(s)}>
              <span class="time mono">{clock(s.at)}</span>
              <span class="what">
                {#if deletionOnly(s)}
                  <span class="danger">{s.changes.length} deleted</span>
                {:else}
                  {s.changes.length} changed
                {/if}
                {#if s.pinned}<span title="pinned">📌</span>{/if}
                {#if s.incomplete}<span class="warn" title="incomplete">!</span>{/if}
              </span>
              <span class="trig muted">{TRIGGER[s.trigger]}</span>
            </button>
          </li>
        {/each}
      </ol>
    </aside>

    <aside class="files">
      <p class="meta">
        <b class="mono">{shortId(selected.id)}</b><br />
        <span class="muted">{ago(selected.at, NOW)} · {summarise(selected.changes)}</span>
      </p>
      {#if selected.incomplete}
        <p class="banner warn">Incomplete: some included files could not be read.</p>
      {/if}
      <div class="viewtoggle">
        <button aria-pressed={view === 'list'} onclick={() => (view = 'list')}>List</button>
        <button aria-pressed={view === 'tree'} onclick={() => (view = 'tree')}>Tree</button>
      </div>
      {#if view === 'tree'}
        <FileTree
          paths={TREE}
          changes={selected.changes}
          selected={path}
          onselect={(p) => (path = p)}
          bind:onlyChanged
        />
      {:else}
        <ul class="flat">
          {#if path && !selected.changes.some((c) => c.path === path)}
            <!-- The selected file is not in this Snapshot's changes: say so rather than leaving a column
                 whose contents look unrelated to what the editor is showing. -->
            <li>
              <button class="file absent" aria-pressed={true}>
                <span>·</span>
                <span class="name mono">{path.split('/').pop()}</span>
                <span class="dir muted">unchanged here</span>
              </button>
            </li>
          {/if}
          {#each selected.changes as c (c.path)}
            <li>
              <button class="file" aria-pressed={c.path === path} onclick={() => (path = c.path)}>
                <span style:color={KIND[c.kind].colour}>{KIND[c.kind].glyph}</span>
                <span class="name mono">{c.path.split('/').pop()}</span>
                <span class="dir muted mono">{c.path.split('/').slice(0, -1).join('/')}</span>
              </button>
            </li>
          {/each}
        </ul>
      {/if}
      {#if changeIn(selected, path ?? '')?.reason}
        <p class="reason muted">withheld — {changeIn(selected, path ?? '')?.reason}</p>
      {/if}
    </aside>

    <section class="main">
      <div class="bar">
        <div class="modes">
          {#each [['current', 'Δ Current'], ['prev', 'Δ Previous'], ['full', 'Full file']] as [k, label]}
            <button aria-pressed={mode === k} onclick={() => (mode = k as Mode)}>{label}</button>
          {/each}
        </div>
        <span class="spacer"></span>
        <button onclick={() => (preflight = true)} disabled={!path}>Restore…</button>
        <button onclick={() => (menu = !menu)} aria-label="more">⋯</button>
        {#if menu}
          <div class="menu">
            <label><input type="checkbox" bind:checked={wrap} /> Wrap long lines</label>
            <button class="plain" onclick={() => (menu = false)}>Open in editor</button>
          </div>
        {/if}
      </div>

      {#if changeIn(selected, path ?? '')?.kind === 'deleted'}
        <p class="banner danger">
          This Snapshot deleted <code class="mono">{path}</code>. Showing the last version that existed.
        </p>
      {/if}

      <DiffPane {path} {mode} {wrap} bind:armed />

      <footer class="state mono muted">
        snapshot={shortId(selected.id)} path={path} mode={mode} wrap={wrap ? 'on' : 'off'}
      </footer>
    </section>
  </div>

  {#if preflight}
    <div class="scrim">
      <div class="dialog">
        <h2>Restore 1 file</h2>
        <p class="muted">Dry run — nothing has been written yet.</p>
        <ul class="mono">
          <li>overwrite <b>{path}</b></li>
        </ul>
        <p class="muted">
          A Safety Snapshot of the current state is taken first, so this restore is itself undoable.
        </p>
        <div class="actions">
          <button onclick={() => (preflight = false)}>Cancel</button>
          <button class="go" onclick={() => (preflight = false)}>Restore</button>
        </div>
      </div>
    </div>
  {/if}
</div>

<style>
  .wrap { height: 100vh; display: flex; flex-direction: column; }
  .top { display: flex; gap: 12px; align-items: baseline; padding: 10px 14px; border-bottom: 1px solid var(--line); }
  .cols { flex: 1; min-height: 0; display: grid; grid-template-columns: 260px 260px 1fr; }
  .rail { border-right: 1px solid var(--line); overflow: auto; background: var(--panel); }
  .files { border-right: 1px solid var(--line); background: var(--panel); display: flex; flex-direction: column; min-height: 0; }
  /* Fixed height and a truncated name: a long filename used to wrap onto a second line and shove the whole
     Snapshot list down by a row. */
  .filter { display: flex; align-items: center; gap: 6px; height: 33px; padding: 0 10px; border-bottom: 1px solid var(--line); color: var(--muted); overflow: hidden; }
  .filter input { flex: none; margin: 0; }
  .filterlabel { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  ol, ul { list-style: none; margin: 0; padding: 0; }
  .day { position: sticky; top: 0; padding: 4px 10px; background: var(--bg); color: var(--muted); border-bottom: 1px solid var(--line); }
  .row { display: grid; grid-template-columns: 46px 1fr auto; gap: 8px; width: 100%; text-align: left; border: 0; border-radius: 0; background: none; padding: 5px 10px; }
  .row[aria-pressed='true'] { background: #222835; }
  .time { color: var(--muted); }
  .trig { font-size: 11px; }
  .meta { padding: 10px; margin: 0; border-bottom: 1px solid var(--line); }
  .viewtoggle { display: flex; gap: 4px; padding: 6px 10px; border-bottom: 1px solid var(--line); }
  .flat { overflow: auto; flex: 1; }
  .file { display: grid; grid-template-columns: 12px auto 1fr; gap: 6px; width: 100%; text-align: left; border: 0; border-radius: 0; background: none; padding: 4px 10px; }
  .file[aria-pressed='true'] { background: #222835; }
  .absent { opacity: 0.75; font-style: italic; }
  .name { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .dir { font-size: 11px; overflow: hidden; text-overflow: ellipsis; }
  .reason { margin: 0; padding: 6px 10px; border-top: 1px solid var(--line); font-size: 11px; }
  .main { display: flex; flex-direction: column; min-width: 0; }
  .bar { position: relative; display: flex; gap: 6px; align-items: center; padding: 8px 10px; border-bottom: 1px solid var(--line); }
  .modes { display: flex; gap: 4px; }
  .spacer { flex: 1; }
  .menu { position: absolute; right: 10px; top: 40px; background: var(--panel); border: 1px solid var(--line); border-radius: 8px; padding: 8px; display: grid; gap: 6px; z-index: 10; }
  .plain { border: 0; background: none; text-align: left; padding: 2px; }
  .banner { margin: 0; padding: 6px 10px; border-bottom: 1px solid var(--line); }
  .warn { color: #d29922; }
  .danger { color: #ff7b72; }
  .state { padding: 4px 10px; border-top: 1px solid var(--line); font-size: 11px; }
  .scrim { position: fixed; inset: 0; background: #0009; display: grid; place-items: center; z-index: 40; }
  .dialog { background: var(--panel); border: 1px solid var(--line); border-radius: 10px; padding: 16px; width: 460px; }
  .dialog h2 { margin: 0 0 6px; font-size: 15px; }
  .actions { display: flex; justify-content: flex-end; gap: 8px; margin-top: 14px; }
  .go { border-color: #ff7b72; color: #ff7b72; }
</style>
