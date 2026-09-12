<script lang="ts">
  /*
   * PROTOTYPE (wayfinder ticket 15).
   *
   * Three variants of the Keepster history browser — timeline, the three view modes, and how Restore is
   * presented — switchable via `?variant=` on this one throwaway page. There is no real page to host them
   * in yet: the only existing route is ticket 09's engine bake-off, which is a measuring instrument.
   */
  import VariantA from './VariantA.svelte'
  import VariantB from './VariantB.svelte'
  import VariantC from './VariantC.svelte'
  import Switcher from './Switcher.svelte'

  const VARIANTS = [
    { key: 'A', name: 'Left rail' },
    { key: 'B', name: 'File first' },
    { key: 'C', name: 'Activity feed' },
  ]

  let variant = $state((new URLSearchParams(location.search).get('variant') ?? 'A').toUpperCase())

  function set(key: string) {
    variant = key
    const q = new URLSearchParams(location.search)
    q.set('variant', key)
    history.replaceState(null, '', '?' + q)
  }
</script>

{#if variant === 'B'}
  <VariantB />
{:else if variant === 'C'}
  <VariantC />
{:else}
  <VariantA />
{/if}

<Switcher variants={VARIANTS} current={variant} onchange={set} />
