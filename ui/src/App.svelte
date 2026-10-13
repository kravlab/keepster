<script lang="ts">
  /*
   * The application shell: the header with the Root picker, and the History screen for that Root.
   *
   * The browser arrives here already signed in: `keepster ui` opened `/?bootstrap=…`, the daemon swapped
   * the code for the session cookie and redirected to `/` (ticket 60), so the address bar holds no secret.
   * When a request finds the session gone, every screen gives way to one that says how to get back.
   */
  import History from './history/History.svelte'
  import { listRoots } from './lib/api'
  import { failureText } from './lib/failures'
  import { strings } from './lib/strings'
  import type { Root } from './lib/wire'

  let roots = $state<Root[] | null>(null)
  let rootId = $state<string | null>(null)
  let error = $state('')
  let ended = $state(false)

  const root = $derived(roots?.find((r) => r.id === rootId) ?? null)

  /** Every screen calls this when the daemon answers 401. */
  function endSession() {
    ended = true
  }

  $effect(() => {
    listRoots()
      .then((list) => {
        roots = list
        rootId = list[0]?.id ?? null
      })
      .catch((e: unknown) => {
        error = failureText(e, endSession)
      })
  })
</script>

<div class="app">
  <header>
    <b>{strings.appName}</b>
    {#if roots && roots.length > 0 && !ended}
      <label>
        {strings.rootLabel}
        <select bind:value={rootId}>
          {#each roots as r (r.id)}
            <option value={r.id}>{r.path}</option>
          {/each}
        </select>
      </label>
      {#if root}<span class="muted">{root.state}</span>{/if}
    {/if}
  </header>

  {#if ended}
    <p class="message">
      {strings.sessionEnded.before} <code>{strings.sessionEnded.command}</code>{strings.sessionEnded.after}
    </p>
  {:else if error}
    <p class="message error">{strings.failed} {error}</p>
  {:else if roots === null}
    <p class="message">{strings.loading}</p>
  {:else if root}
    <!-- Keyed: another Root is another history, and nothing selected in one means anything in the other. -->
    {#key root.id}
      <History rootId={root.id} onSessionEnded={endSession} />
    {/key}
  {:else}
    <p class="message">{strings.noRoots}</p>
  {/if}
</div>

<style>
  .app {
    height: 100vh;
    display: flex;
    flex-direction: column;
  }
  header {
    display: flex;
    gap: 12px;
    align-items: center;
    padding: 8px 14px;
    border-bottom: 1px solid var(--line);
  }
  label {
    display: flex;
    gap: 6px;
    align-items: center;
  }
  .message {
    padding: 24px;
  }
</style>
