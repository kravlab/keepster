<script lang="ts">
  /*
   * The Restore dialog, the one face every view mode gives Restore (ticket 15). It opens on a dry run
   * that names what will be overwritten or created, asks every confirmation and resolution the daemon's
   * plan needs, and states the Safety Snapshot promise on the same panel; the Restore button is live only
   * once everything is answered. What the questions are and when the request may go is `lib/restore.ts`.
   *
   * Two plans are kept. The questions come from a dry run made without answers, so a question stays on
   * screen once answered; each change of the answers runs the dry run again with them, which is what the
   * list of files shows. A refusal of the real request, when the Working Tree changed meanwhile, is shown
   * at once, and the questions are asked afresh from a new dry run without answers: the refusal's own plan
   * was made with the answers, so it no longer names what they settled, and would drop them from the next
   * request. The answers given are kept.
   */
  import { onMount } from 'svelte'
  import { restore } from '../lib/api'
  import { failureText } from '../lib/failures'
  import { momentOf } from '../lib/ids'
  import { shownPath } from '../lib/paths'
  import {
    emptyAnswers,
    outcomeCounts,
    questions,
    ready,
    restoreRequest,
    unrecoverableReasons,
    type Answers,
  } from '../lib/restore'
  import { strings } from '../lib/strings'
  import type { Confirmation, ModesChoice, NameCaseChoice, Restored, RestoredPath } from '../lib/wire'

  let {
    rootId,
    from,
    path,
    beforeDeletion,
    onClose,
    onSessionEnded,
  }: {
    rootId: string
    /** The Snapshot whose version is written back. */
    from: string
    path: string
    /** Whether `from` is the Snapshot before the one that deleted the file. */
    beforeDeletion: boolean
    onClose: () => void
    onSessionEnded: () => void
  } = $props()

  let dialog = $state<HTMLDialogElement>()
  /* The plan without answers: what the questions are. */
  let preflight = $state<RestoredPath[] | null>(null)
  /* The plan with the answers given so far: what the file list shows. */
  let plan = $state<RestoredPath[] | null>(null)
  let answers = $state<Answers>(emptyAnswers())
  let changedSince = $state(false)
  let running = $state(false)
  let result = $state<Restored | null>(null)
  let error = $state('')

  const asked = $derived(questions(preflight ?? []))
  const canGo = $derived(
    preflight !== null &&
      plan !== null &&
      ready(preflight, answers) &&
      !plan.some((file) => file.outcome === 'blocked') &&
      !running,
  )
  const moment = $derived(momentOf(from))

  /** Asks the daemon what Restore would do with the answers `given`. */
  async function dryRun(given: Answers): Promise<RestoredPath[]> {
    const answer = await restore(rootId, restoreRequest(from, [path], preflight ?? [], given, true))
    // A dry run never refuses for want of answers; this branch only keeps the types honest.
    return answer.kind === 'done' ? answer.restored.paths : answer.paths
  }

  onMount(() => {
    dialog?.showModal()
    void (async () => {
      try {
        preflight = await dryRun(emptyAnswers())
      } catch (e) {
        error = failureText(e, onSessionEnded)
      }
    })()
  })

  /*
   * The plan with the answers: asked once the questions are known, and again on every change of the
   * answers; a newer change supersedes an older one still on its way.
   */
  $effect(() => {
    const given: Answers = $state.snapshot(answers)
    if (preflight === null || result !== null) return
    let superseded = false
    void dryRun(given)
      .then((paths) => {
        if (!superseded) plan = paths
      })
      .catch((e: unknown) => {
        if (!superseded) error = failureText(e, onSessionEnded)
      })
    return () => {
      superseded = true
    }
  })

  /** Sends the real request; a refusal for want of answers shows the plan as it stands now. */
  async function go() {
    if (!preflight) return
    running = true
    error = ''
    try {
      const answer = await restore(rootId, restoreRequest(from, [path], preflight, answers, false))
      if (answer.kind === 'done') {
        result = answer.restored
      } else {
        plan = answer.paths
        changedSince = true
        preflight = await dryRun(emptyAnswers())
      }
    } catch (e) {
      error = failureText(e, onSessionEnded)
    } finally {
      running = false
    }
  }

  /** Ticks or unticks a confirmation, which holds for every file that needs it. */
  function toggleConfirm(confirm: Confirmation, on: boolean) {
    answers.confirm = on ? [...answers.confirm, confirm] : answers.confirm.filter((c) => c !== confirm)
  }

  /** Sets one path's answer, replacing any earlier one. */
  function resolve(target: string, resolution: Answers['resolutions'][string]) {
    answers.resolutions = { ...answers.resolutions, [target]: resolution }
  }
</script>

<dialog bind:this={dialog} onclose={onClose} aria-labelledby="restore-title">
  <h2 id="restore-title">{strings.restore.title(shownPath(path))}</h2>
  <p>{beforeDeletion ? strings.restore.beforeDeletion(moment) : strings.restore.version(moment)}</p>

  {#if error}
    <p class="error">{strings.failed} {error}</p>
  {/if}

  {#if result}
    <p>{strings.restore.done}</p>
    <ul class="files">
      {#each result.paths as file (file.path)}
        <li>
          <code>{shownPath(file.path)}</code>
          {strings.restore.outcome[file.outcome]}
          {#if file.target}<span class="muted">{strings.restore.writtenAs(shownPath(file.target))}</span>{/if}
          {#if file.error}<span class="error">{file.error}</span>{/if}
        </li>
      {/each}
    </ul>
    <p class="muted">{strings.restore.summary(outcomeCounts(result.paths))}</p>
    <p class="safety">
      {result.safety_snapshot
        ? strings.restore.undo(momentOf(result.safety_snapshot), result.safety_snapshot)
        : strings.restore.noSafety}
    </p>
    <div class="actions">
      <button onclick={() => dialog?.close()}>{strings.restore.close}</button>
    </div>
  {:else}
    {#if plan === null}
      {#if !error}<p class="muted">{strings.restore.planning}</p>{/if}
    {:else}
      {#if changedSince}<p class="warn">{strings.restore.changedSince}</p>{/if}
      <p>{strings.restore.dryRun}</p>
      <ul class="files">
        {#each plan as file (file.path)}
          <li class:blocked={file.outcome === 'blocked'}>
            <code>{shownPath(file.path)}</code>
            {strings.restore.planned[file.outcome]}
            {#if file.target}<span class="muted">{strings.restore.writtenAs(shownPath(file.target))}</span>{/if}
          </li>
        {/each}
      </ul>
    {/if}

    {#each asked as question (question.kind === 'resolution' ? `${question.path}:${question.resolution}` : question.kind === 'confirm' ? question.confirm : 'unrestorable')}
      <fieldset>
        {#if question.kind === 'confirm'}
          <label>
            <input
              type="checkbox"
              checked={answers.confirm.includes(question.confirm)}
              onchange={(e) => toggleConfirm(question.confirm, e.currentTarget.checked)}
            />
            {strings.restore.confirm[question.confirm]}
          </label>
          <p class="muted"><code>{strings.list(question.paths.map(shownPath))}</code></p>
        {:else if question.kind === 'unrestorable'}
          <p>{strings.restore.unrestorable} <code>{strings.list(question.paths.map(shownPath))}</code></p>
          <label>
            <input type="checkbox" bind:checked={answers.skipUnresolvable} />
            {strings.restore.skip}
          </label>
        {:else if question.resolution === 'name_case'}
          <p>{strings.restore.nameCase(shownPath(question.issue.existing ?? question.path))}</p>
          {#each Object.entries(strings.restore.nameCaseChoice) as [choice, label] (choice)}
            <label>
              <input
                type="radio"
                name="case-{question.path}"
                checked={answers.resolutions[question.path]?.name_case === choice}
                onchange={() => resolve(question.path, { name_case: choice as NameCaseChoice })}
              />
              {label}
            </label>
          {/each}
        {:else if question.resolution === 'modes'}
          <p>
            <code>{shownPath(question.path)}</code>
            {question.issue.current_mode != null && question.issue.snapshot_mode != null
              ? strings.restore.modes(question.issue.current_mode, question.issue.snapshot_mode)
              : strings.restore.modesUnknown}
          </p>
          {#each Object.entries(strings.restore.modesChoice) as [choice, label] (choice)}
            <label>
              <input
                type="radio"
                name="modes-{question.path}"
                checked={answers.resolutions[question.path]?.modes === choice}
                onchange={() => resolve(question.path, { modes: choice as ModesChoice })}
              />
              {label}
            </label>
          {/each}
        {:else if question.resolution === 'rename_to'}
          <label>
            <code>{shownPath(question.path)}</code>
            {strings.restore.renameTo}
            <input
              type="text"
              value={answers.resolutions[question.path]?.rename_to ?? ''}
              oninput={(e) => resolve(question.path, { rename_to: e.currentTarget.value })}
            />
          </label>
        {:else if question.resolution === 'write'}
          <label>
            <input
              type="checkbox"
              checked={answers.resolutions[question.path]?.write === true}
              onchange={(e) => resolve(question.path, { write: e.currentTarget.checked })}
            />
            <code>{shownPath(question.path)}</code>
            {strings.restore.write((question.issue.paths ?? []).map(shownPath))}
          </label>
        {/if}
        {#if question.kind === 'confirm' && question.confirm === 'overwrite_unrecoverable'}
          {@const reasons = unrecoverableReasons(preflight ?? []).map(strings.restore.unrecoverable)}
          {#if reasons.length}<p class="warn">{strings.list(reasons)}</p>{/if}
        {/if}
      </fieldset>
    {/each}

    <p class="safety">{strings.restore.safety}</p>

    <div class="actions">
      <button onclick={() => dialog?.close()}>{strings.restore.cancel}</button>
      <button class="go" disabled={!canGo} onclick={go}>{running ? strings.restore.working : strings.restore.go}</button
      >
    </div>
  {/if}
</dialog>

<style>
  dialog {
    width: min(560px, calc(100vw - 32px));
    border: 1px solid var(--line);
    border-radius: 8px;
    padding: 16px 18px;
    background: var(--panel);
    color: var(--fg);
  }
  dialog::backdrop {
    background: rgb(0 0 0 / 0.35);
  }
  h2 {
    margin: 0 0 6px;
    font-size: 15px;
    overflow-wrap: anywhere;
  }
  p {
    margin: 6px 0;
  }
  .files {
    margin: 6px 0;
    padding-left: 18px;
  }
  .files code,
  fieldset code {
    overflow-wrap: anywhere;
  }
  .blocked {
    color: var(--warn);
  }
  fieldset {
    margin: 8px 0;
    border: 1px solid var(--line);
    border-radius: 6px;
    padding: 6px 10px;
  }
  label {
    display: flex;
    gap: 6px;
    align-items: baseline;
    margin: 4px 0;
  }
  .safety {
    padding: 8px 10px;
    border-left: 3px solid var(--accent);
    background: var(--bg);
  }
  .warn {
    color: var(--warn);
  }
  .actions {
    display: flex;
    gap: 8px;
    justify-content: flex-end;
    margin-top: 12px;
  }
  .go:not(:disabled) {
    border-color: var(--danger);
    color: var(--danger);
  }
  .go:disabled {
    cursor: not-allowed;
    opacity: 0.5;
  }
</style>
