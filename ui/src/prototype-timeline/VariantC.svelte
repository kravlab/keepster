<script lang="ts">
  /*
   * PROTOTYPE (ticket 15) — Variant C: the activity feed.
   *
   * Neither a rail nor a scrubber: one centre column of Snapshot cards, each showing its changed paths
   * inline. Navigation is by *change*, not by file, which is the shape that answers "what happened to this
   * Root while I was away" rather than "show me this file's history". A Snapshot that only deleted files is
   * the loudest card on the page instead of an empty diff.
   *
   * The diff is an overlay, so the destructive control cannot sit next to the browsing controls at all: it
   * exists only inside the overlay, behind a disclosure.
   */
  import DiffPane from './DiffPane.svelte'
  import { selection } from './selection.svelte'
  import { SNAPSHOTS, ROOT, NOW, type Snapshot } from '../fixtures/timeline'
  import { KIND, TRIGGER, additionOnly, ago, day, deletionOnly, shortId, summarise, type Mode } from './shared'

  let open = $state<{ s: Snapshot; path: string } | null>(null)
  let mode = $state<Mode>('current')
  let wrap = $state(false)
  let armed = $state(new Set<string>())
  let confirming = $state(false)
  let limit = $state(40)
  let onlyNoise = $state(false)

  /* A feed cannot show 400 cards, so it pages — and the filter is the feed's answer to density: hide the
     ordinary watcher churn and keep what a human would call an event. */
  const feed = $derived(
    (onlyNoise
      ? SNAPSHOTS.filter((s) => deletionOnly(s) || additionOnly(s) || s.incomplete || s.pinned || s.trigger !== 'watcher')
      : SNAPSHOTS
    ).slice(0, limit),
  )

  const rows = $derived(
    feed.map((s, i) => ({ s, head: i === 0 || day(s.at) !== day(feed[i - 1].at) ? day(s.at) : null })),
  )

  function close() {
    open = null
    confirming = false
  }
</script>

<div class="wrap">
  <header class="top">
    <b>{ROOT.name}</b>
    <span class="muted">{ROOT.state} · {SNAPSHOTS.length} snapshots</span>
    <span class="spacer"></span>
    <label class="muted"><input type="checkbox" bind:checked={onlyNoise} /> hide routine watcher snapshots</label>
  </header>

  <div class="feed">
    {#each rows as { s, head } (s.id)}
      {#if head}<h3 class="day">{head}</h3>{/if}
      <article class:alarm={deletionOnly(s)} class:quiet={!deletionOnly(s)}>
        <div class="head">
          <b class="mono">{shortId(s.id)}</b>
          <span class="muted">{ago(s.at, NOW)} · {TRIGGER[s.trigger]}</span>
          <span class="spacer"></span>
          {#if s.pinned}<span title="pinned">📌 pinned</span>{/if}
          {#if s.incomplete}<span class="warn">incomplete</span>{/if}
        </div>
        {#if deletionOnly(s)}
          <p class="lead danger">{s.changes.length} files deleted, nothing added or edited.</p>
        {:else if additionOnly(s)}
          <p class="lead ok">{s.changes.length} new files, nothing edited.</p>
        {:else}
          <p class="lead muted">{summarise(s.changes)}</p>
        {/if}
        <ul>
          {#each s.changes as c (c.path)}
            <li>
              <!-- C publishes but never consumes: arriving in a feed should not auto-open an overlay. -->
              <button
                class="path mono"
                onclick={() => {
                  open = { s, path: c.path }
                  selection.path = c.path
                  selection.snapshotId = s.id
                }}
              >
                <span style:color={KIND[c.kind].colour}>{KIND[c.kind].glyph}</span>
                {c.path}
              </button>
              {#if c.reason}<span class="muted"> — withheld, {c.reason}</span>{/if}
            </li>
          {/each}
        </ul>
      </article>
    {/each}
    {#if limit < SNAPSHOTS.length}
      <button class="more" onclick={() => (limit += 40)}>Show 40 older snapshots</button>
    {/if}
  </div>

  {#if open}
    <div class="overlay">
      <header class="obar">
        <b class="mono">{open.path}</b>
        <span class="muted">{shortId(open.s.id)} · {ago(open.s.at, NOW)}</span>
        <span class="spacer"></span>
        <div class="modes">
          {#each [['current', 'Δ Current'], ['prev', 'Δ Previous'], ['full', 'Full file']] as [k, label]}
            <button aria-pressed={mode === k} onclick={() => (mode = k as Mode)}>{label}</button>
          {/each}
        </div>
        <label class="muted"><input type="checkbox" bind:checked={wrap} /> wrap</label>
        <button onclick={close} aria-label="close">✕</button>
      </header>

      <DiffPane path={open.path} {mode} {wrap} bind:armed />

      <footer class="obottom">
        {#if confirming}
          <span class="danger">Overwrite the working file with this version?</span>
          <span class="muted">A Safety Snapshot is taken first, so this is undoable.</span>
          <button onclick={() => (confirming = false)}>Cancel</button>
          <button class="go" onclick={() => (confirming = false)}>Overwrite</button>
        {:else}
          <button onclick={() => (confirming = true)}>Restore…</button>
          <button>Open in editor</button>
          <span class="spacer"></span>
          <span class="state mono muted">mode={mode} wrap={wrap ? 'on' : 'off'}</span>
        {/if}
      </footer>
    </div>
  {/if}
</div>

<style>
  .wrap { height: 100vh; display: flex; flex-direction: column; }
  .top { display: flex; gap: 12px; align-items: baseline; padding: 10px 14px; border-bottom: 1px solid var(--line); }
  .spacer { flex: 1; }
  .feed { flex: 1; overflow: auto; padding: 14px 0 80px; }
  .day { position: sticky; top: 0; margin: 0; padding: 6px 0; width: 640px; max-width: 92vw; margin-inline: auto; background: var(--bg); color: var(--muted); font-size: 12px; font-weight: 500; }
  article { width: 640px; max-width: 92vw; margin: 0 auto 10px; border: 1px solid var(--line); border-radius: 10px; padding: 10px 12px; background: var(--panel); }
  article.alarm { border-color: #5a2a2a; background: #201a1a; }
  .head { display: flex; gap: 10px; align-items: baseline; }
  .lead { margin: 6px 0; }
  ul { list-style: none; margin: 0; padding: 0; }
  .path { border: 0; background: none; padding: 2px 0; text-align: left; }
  .more { display: block; margin: 10px auto; }
  .ok { color: #7ee787; }
  .warn { color: #d29922; }
  .danger { color: #ff7b72; }
  .overlay { position: fixed; inset: 0; z-index: 30; display: flex; flex-direction: column; background: var(--bg); }
  .obar, .obottom { display: flex; gap: 10px; align-items: center; padding: 8px 14px; border-bottom: 1px solid var(--line); }
  .obottom { border-bottom: 0; border-top: 1px solid var(--line); }
  .modes { display: flex; gap: 4px; }
  .go { border-color: #ff7b72; color: #ff7b72; }
  .state { font-size: 11px; }
</style>
