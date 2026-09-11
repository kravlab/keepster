<script lang="ts">
  /*
   * PROTOTYPE (wayfinder ticket 09) — editor bake-off.
   *
   * The question: CodeMirror 6 MergeView vs Monaco DiffEditor for the Keepster web UI, judged on
   * bundle size, syntax highlighting quality and behaviour on a large file.
   * Everything here is throwaway; the UX variants of the timeline are a separate pass.
   */
  import EditorPane from './EditorPane.svelte'
  import { FIXTURES, materialise, type Fixture } from './fixtures'
  import type { CmDiffSource, Engine, ViewMode } from './editors/types'

  const params = new URLSearchParams(location.search)

  let engines = $state<Engine[]>(
    ((params.get('engine') ?? 'both') === 'both' ? ['cm6', 'monaco'] : [params.get('engine') as Engine]),
  )
  let mode = $state<ViewMode>((params.get('mode') as ViewMode) ?? 'diff')
  let cmdiff = $state<CmDiffSource>((params.get('cmdiff') as CmDiffSource) ?? 'builtin')
  let wrap = $state(params.get('wrap') === 'on')
  let lang = $state(params.get('lang') !== 'off')
  let selectedId = $state(params.get('file') ?? FIXTURES[0].id)
  /* Heavy fixtures are armed by an explicit click: a 4 MB diff mounted on hover is how a tab dies. */
  let armed = $state(new Set<string>())

  const meta = $derived(FIXTURES.find((f) => f.id === selectedId) ?? FIXTURES[0])
  const ready = $derived(!meta.heavy || armed.has(meta.id))
  const fixture = $derived<Fixture | null>(ready ? materialise(meta) : null)

  $effect(() => {
    const q = new URLSearchParams({
      file: selectedId,
      mode,
      cmdiff,
      wrap: wrap ? 'on' : 'off',
      lang: lang ? 'on' : 'off',
      engine: engines.length === 2 ? 'both' : engines[0],
    })
    history.replaceState(null, '', '?' + q)
  })

  function toggleEngine(e: Engine) {
    engines = engines.includes(e) ? engines.filter((x) => x !== e) : [...engines, e].sort()
  }

  const fmtBytes = (n: number) => (n < 1024 ? n + ' B' : n < 1024 * 1024 ? (n / 1024).toFixed(1) + ' KiB' : (n / 1048576).toFixed(2) + ' MiB')
</script>

<div class="layout">
  <aside>
    <h1>Editor bake-off <span class="tag">PROTOTYPE · ticket 09</span></h1>
    <ul>
      {#each FIXTURES as f (f.id)}
        <li>
          <button class="fixture" aria-pressed={f.id === selectedId} onclick={() => (selectedId = f.id)}>
            <span class="name mono">{f.label}</span>
            <span class="note">{f.note}</span>
          </button>
        </li>
      {/each}
    </ul>
    <p class="hint">
      Sizes and languages come from the fixture; bundle weight is measured separately with
      <code>pnpm measure</code>.
    </p>
  </aside>

  <main>
    <div class="toolbar">
      <span class="group">
        <button aria-pressed={mode === 'diff'} onclick={() => (mode = 'diff')}>Δ Previous</button>
        <button aria-pressed={mode === 'full'} onclick={() => (mode = 'full')}>Full file</button>
      </span>
      <span class="group">
        <button aria-pressed={engines.includes('cm6')} onclick={() => toggleEngine('cm6')}>CodeMirror 6</button>
        <button aria-pressed={engines.includes('monaco')} onclick={() => toggleEngine('monaco')}>Monaco</button>
      </span>
      <span class="group" title="Where CodeMirror's hunks come from; Monaco always diffs in its own worker">
        <button aria-pressed={cmdiff === 'builtin'} onclick={() => (cmdiff = 'builtin')}>CM6 hunks: builtin</button>
        <button aria-pressed={cmdiff === 'daemon'} onclick={() => (cmdiff = 'daemon')}>CM6 hunks: daemon</button>
      </span>
      <span class="group" title="CodeMirror only: wrap long lines; Monaco is unchanged">
        <button aria-pressed={wrap} onclick={() => (wrap = !wrap)}>CM6 wrap: {wrap ? 'on' : 'off'}</button>
      </span>
      <span class="group" title="CodeMirror only: attach the language package (Lezer parse); Monaco is unchanged">
        <button aria-pressed={lang} onclick={() => (lang = !lang)}>CM6 lang: {lang ? 'on' : 'off'}</button>
      </span>
      <span class="meta mono">
        {meta.filename}
        {#if fixture}· {fmtBytes(fixture.bytes)} · {fixture.lines.toLocaleString()} lines{/if}
      </span>
    </div>

    {#if meta.heavy && !ready}
      <div class="pending">
        <p>{meta.note}</p>
        <p class="hint">Generated on demand, then mounted in every selected engine at once.</p>
        <button class="arm" onclick={() => (armed = new Set(armed).add(meta.id))}>Mount heavy fixture</button>
      </div>
    {:else}
      <div class="panes" style="--cols: {engines.length}">
        {#each engines as e (e)}
          <EditorPane engine={e} {fixture} {mode} {cmdiff} {wrap} {lang} />
        {/each}
        {#if engines.length === 0}<p class="pending">No engine selected.</p>{/if}
      </div>
    {/if}
  </main>
</div>

<style>
  .layout { display: grid; grid-template-columns: 320px 1fr; height: 100vh; }
  aside { border-right: 1px solid var(--line); padding: 12px; overflow: auto; }
  h1 { font-size: 14px; margin: 0 0 12px; }
  .tag { color: var(--muted); font-weight: 400; font-size: 11px; }
  ul { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 6px; }
  .fixture { display: block; width: 100%; text-align: left; padding: 8px; }
  .name { display: block; color: var(--accent); }
  .note { display: block; color: var(--muted); font-size: 11px; margin-top: 2px; }
  .hint { color: var(--muted); font-size: 11px; }
  main { display: flex; flex-direction: column; min-width: 0; padding: 12px; gap: 10px; }
  .toolbar { display: flex; gap: 16px; align-items: center; flex-wrap: wrap; }
  .group { display: flex; gap: 4px; }
  .meta { color: var(--muted); font-size: 12px; }
  .arm { border-color: var(--accent); }
  .panes { flex: 1; min-height: 0; display: grid; grid-template-columns: repeat(var(--cols), minmax(0, 1fr)); gap: 10px; }
  .pending { color: var(--muted); padding: 24px; }
</style>
