<script lang="ts">
  /*
   * Chooses a directory on the daemon's machine (#29): the directory listed is the one chosen. The
   * path field shows it and takes an absolute path typed or pasted; Up goes to the parent; a folder is
   * gone into by its button, except one the daemon cannot list, which is shown as such and disabled.
   * Hidden folders are left out until asked for. Adding a directory and recording a Move use it, and so
   * will choosing where an Export writes; where it starts, what is listed, what the path field holds and
   * why a place was refused is `lib/folders`.
   */
  import { onMount } from 'svelte'
  import type { FolderBrowser } from '../lib/folders.svelte'
  import { strings } from '../lib/strings'

  let {
    browser,
    disabled = false,
  }: {
    /** The browser to draw and drive; its `path` is the directory chosen. */
    browser: FolderBrowser
    /** Whether choosing is held, as while the directory chosen is being sent. */
    disabled?: boolean
  } = $props()

  onMount(() => {
    if (browser.listing === null) void browser.begin()
  })

  /** Lists the path typed or pasted, without the whitespace around it, unless the field is empty. */
  function openTyped(e: SubmitEvent) {
    e.preventDefault()
    const path = browser.typed.trim()
    if (path !== '') void browser.open(path)
  }
</script>

<div class="picker">
  <form class="bar" onsubmit={openTyped}>
    <button type="button" disabled={disabled || !browser.canGoUp} onclick={() => void browser.up()}>
      {strings.folders.up}
    </button>
    <input
      class="path"
      type="text"
      aria-label={strings.folders.path}
      spellcheck="false"
      autocomplete="off"
      {disabled}
      bind:value={browser.typed}
    />
    <button type="submit" {disabled}>{strings.folders.go}</button>
  </form>

  {#if browser.error}
    <p class="error" role="alert">{browser.error}</p>
  {/if}

  {#if browser.listing === null}
    {#if !browser.error}<p class="muted">{strings.loading}</p>{/if}
  {:else if browser.directories.length === 0}
    <p class="muted empty">{strings.folders.empty}</p>
  {:else}
    <ul aria-label={strings.folders.label}>
      {#each browser.directories as directory (directory.path)}
        <li>
          <button
            type="button"
            class:hidden={directory.hidden}
            disabled={disabled || !directory.readable}
            onclick={() => void browser.enter(directory)}
          >
            <span class="name">{directory.name}</span>
            {#if !directory.readable}<span class="unreadable">{strings.folders.unreadable}</span>{/if}
          </button>
        </li>
      {/each}
    </ul>
  {/if}

  <label class="toggle">
    <input type="checkbox" {disabled} bind:checked={browser.showHidden} />
    {strings.folders.showHidden}
  </label>
</div>

<style>
  .picker {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .bar {
    display: flex;
    gap: 6px;
  }
  .path {
    flex: 1;
    min-width: 0;
    font: inherit;
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    color: inherit;
    background: var(--bg);
    border: 1px solid var(--line);
    border-radius: 6px;
    padding: 3px 8px;
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 4px;
    height: min(280px, 40vh);
    overflow: auto;
    background: var(--bg);
    border: 1px solid var(--line);
    border-radius: 6px;
  }
  li button {
    display: flex;
    width: 100%;
    gap: 8px;
    align-items: baseline;
    text-align: left;
    border-color: transparent;
    background: transparent;
    padding: 2px 8px;
  }
  li button:not(:disabled):hover {
    background: var(--selected);
  }
  /* The list scrolls, so the global ring, drawn outside, would be clipped at its edges. */
  li button:focus-visible {
    outline-offset: -2px;
  }
  li button:disabled {
    cursor: not-allowed;
  }
  .name {
    overflow-wrap: anywhere;
  }
  .hidden .name {
    color: var(--muted);
  }
  .unreadable {
    margin-left: auto;
    font-size: 12px;
    color: var(--muted);
    white-space: nowrap;
  }
  li button:disabled .name {
    color: var(--muted);
  }
  .empty {
    margin: 0;
    padding: 10px;
    border: 1px dashed var(--line);
    border-radius: 6px;
  }
  .error,
  .muted {
    margin: 0;
  }
  .toggle {
    display: flex;
    gap: 6px;
    align-items: center;
  }
</style>
