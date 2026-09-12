<script lang="ts">
  /*
   * PROTOTYPE (ticket 15) — a plain CodeMirror pane with none of ticket 09's measuring chrome.
   *
   * Ticket 09 settled the engine and that hunks always come from the daemon, so this mounts cm6 with
   * `cmdiff: 'daemon'` and never lets MergeView diff on its own. The three modes the ticket asks about map
   * onto the adapter's two: `full` is the whole file, both deltas are a diff (a fixture has no Working Tree,
   * so `Δ Current` is modelled, not real).
   */
  import type { Mounted } from '../editors/types'
  import { contentFor, fixtureId, type Mode } from './shared'

  let {
    path,
    mode,
    wrap,
    armed = $bindable(),
  }: { path: string | null; mode: Mode; wrap: boolean; armed: Set<string> } = $props()

  let host: HTMLDivElement
  let mounted: Mounted | null = null
  let status = $state<'idle' | 'loading' | 'ready' | 'error'>('idle')
  let error = $state('')

  const content = $derived(contentFor(path, armed))

  $effect(() => {
    const f = content.fixture
    const m = mode
    const w = wrap
    let cancelled = false

    void (async () => {
      mounted?.destroy()
      mounted = null
      host?.replaceChildren()
      if (!f || f.binary) {
        status = 'idle'
        return
      }
      status = 'loading'
      error = ''
      try {
        const mod = await import('../editors/cm6')
        if (cancelled) return
        const res = await mod.mount({
          el: host,
          fixture: f,
          mode: m === 'full' ? 'full' : 'diff',
          cmdiff: 'daemon',
          wrap: w,
          lang: true,
        })
        if (cancelled) {
          res.destroy()
          return
        }
        mounted = res
        status = 'ready'
      } catch (e) {
        if (!cancelled) {
          status = 'error'
          error = String(e)
        }
      }
    })()

    return () => {
      cancelled = true
      mounted?.destroy()
      mounted = null
    }
  })

  function arm() {
    const id = fixtureId(path)
    if (id) armed = new Set(armed).add(id)
  }
</script>

<div class="pane">
  {#if !path}
    <p class="empty">Select a file to see its version.</p>
  {:else if content.heavy}
    <div class="empty">
      <p><b>{content.label}</b> is a heavy fixture.</p>
      <button onclick={arm}>Load it</button>
    </div>
  {:else if !content.fixture}
    <p class="empty">
      <code>{path}</code> — {content.label}. The timeline still knows it changed; only the content is absent
      from this prototype.
    </p>
  {:else if content.fixture.binary}
    <div class="empty">
      <p><b>{content.label}</b> is binary. Ticket 09 left what to show here to us, not to the engine.</p>
    </div>
  {/if}
  <div class="host" bind:this={host} hidden={!content.fixture || content.fixture.binary}></div>
  {#if status === 'error'}<p class="err">{error}</p>{/if}
</div>

<style>
  .pane {
    display: flex;
    flex-direction: column;
    min-height: 0;
    height: 100%;
    background: var(--bg);
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
  .err {
    padding: 12px;
    color: #ff7b72;
  }
</style>
