<script lang="ts">
  /*
   * PROTOTYPE (ticket 15) — the floating variant switcher.
   *
   * Deliberately loud and un-designed: it must never be mistaken for part of the design being judged.
   * Hidden in a production build so a stray merge cannot ship it.
   */
  let {
    variants,
    current,
    onchange,
  }: { variants: { key: string; name: string }[]; current: string; onchange: (key: string) => void } = $props()

  const index = $derived(Math.max(0, variants.findIndex((v) => v.key === current)))

  function step(delta: number) {
    const next = variants[(index + delta + variants.length) % variants.length]
    onchange(next.key)
  }

  function onkeydown(e: KeyboardEvent) {
    const el = document.activeElement
    if (el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement || (el as HTMLElement)?.isContentEditable)
      return
    if (e.key === 'ArrowLeft') step(-1)
    else if (e.key === 'ArrowRight') step(1)
  }
</script>

<svelte:window on:keydown={onkeydown} />

{#if !import.meta.env.PROD}
  <div class="bar">
    <button onclick={() => step(-1)} aria-label="previous variant">←</button>
    <span class="label">{current} ({variants[index]?.name})</span>
    <button onclick={() => step(1)} aria-label="next variant">→</button>
  </div>
{/if}

<style>
  .bar {
    position: fixed;
    bottom: 18px;
    left: 50%;
    transform: translateX(-50%);
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 6px 10px;
    border-radius: 999px;
    background: #f5f6f8;
    color: #14161a;
    box-shadow: 0 6px 24px rgb(0 0 0 / 55%);
    z-index: 50;
  }
  .bar button {
    background: #14161a;
    color: #f5f6f8;
    border: 0;
    border-radius: 999px;
    width: 28px;
    height: 28px;
    line-height: 1;
    cursor: pointer;
  }
  .label {
    font-weight: 600;
    white-space: nowrap;
  }
</style>
