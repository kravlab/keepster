<script lang="ts">
  /* PROTOTYPE (ticket 09): mounts one engine against one fixture and reports what it cost. */
  import type { CmDiffSource, Engine, Mounted, ViewMode } from './editors/types'
  import type { Fixture } from './fixtures'

  let {
    engine,
    fixture,
    mode,
    cmdiff,
    wrap,
    lang,
  }: { engine: Engine; fixture: Fixture | null; mode: ViewMode; cmdiff: CmDiffSource; wrap: boolean; lang: boolean } =
    $props()

  /* The engine chunk is fetched once per session; the first fetch is the number worth seeing, so it
     is remembered here rather than re-measured against the module cache. */
  const firstLoadMs = new Map<Engine, number>()

  let host: HTMLDivElement
  let mounted: Mounted | null = null
  let status = $state<'idle' | 'loading' | 'ready' | 'error'>('idle')
  let error = $state('')
  let metrics = $state<{ paintMs: number; constructMs: number; hunksMs?: number; language: string; chunkMs: number } | null>(
    null,
  )

  $effect(() => {
    const f = fixture
    const m = mode
    const e = engine
    const c = cmdiff
    const w = wrap
    const l = lang
    let cancelled = false

    void (async () => {
      mounted?.destroy()
      mounted = null
      metrics = null
      host.replaceChildren()
      if (!f || f.binary) {
        status = 'idle'
        return
      }
      status = 'loading'
      error = ''
      try {
        const t0 = performance.now()
        const mod = e === 'cm6' ? await import('./editors/cm6') : await import('./editors/monaco')
        if (!firstLoadMs.has(e)) firstLoadMs.set(e, performance.now() - t0)
        if (cancelled) return
        const res = await mod.mount({ el: host, fixture: f, mode: m, cmdiff: c, wrap: w, lang: l })
        if (cancelled) {
          res.destroy()
          return
        }
        mounted = res
        metrics = { ...res, chunkMs: firstLoadMs.get(e)! }
        status = 'ready'
      } catch (err) {
        error = err instanceof Error ? err.stack || err.message : String(err)
        status = 'error'
      }
    })()

    return () => {
      cancelled = true
      mounted?.destroy()
      mounted = null
    }
  })
</script>

<section class="pane">
  <header>
    <strong>{engine === 'cm6' ? 'CodeMirror 6 — MergeView' : 'Monaco — DiffEditor'}</strong>
    {#if metrics}
      <span class="mono numbers">
        chunk {metrics.chunkMs.toFixed(0)} ms ·
        {#if metrics.hunksMs !== undefined}hunks {metrics.hunksMs.toFixed(0)} ms off-thread ·{/if}
        construct {metrics.constructMs.toFixed(0)} ms · paint
        {metrics.paintMs.toFixed(0)} ms · lang {metrics.language}
      </span>
    {:else if status === 'loading'}
      <span class="numbers">loading…</span>
    {/if}
  </header>

  {#if fixture?.binary}
    <div class="placeholder">
      <p><strong>Binary file — no text diff.</strong></p>
      <p>
        Neither engine is asked to render this. Detection and presentation are ours: size before/after,
        and the actions that still make sense (restore, open in editor).
      </p>
    </div>
  {:else if status === 'error'}
    <pre class="placeholder mono err">{error}</pre>
  {/if}

  <div class="host" bind:this={host}></div>
</section>

<style>
  .pane { display: flex; flex-direction: column; min-width: 0; border: 1px solid var(--line); border-radius: 8px; overflow: hidden; background: var(--panel); }
  header { display: flex; gap: 12px; align-items: baseline; padding: 6px 10px; border-bottom: 1px solid var(--line); }
  .numbers { color: var(--muted); font-size: 12px; }
  .host { flex: 1; min-height: 0; overflow: hidden; }
  .host:empty { display: none; }
  .placeholder { padding: 16px; color: var(--muted); }
  .err { white-space: pre-wrap; color: #ff7b72; }
</style>
