/**
 * PROTOTYPE (wayfinder ticket 09) — CodeMirror 6 adapter.
 *
 * Deliberately NOT `basicSetup`: a history viewer needs no autocomplete, no search panel and no
 * edit keymap, and bundling them would flatter Monaco in the size comparison for no reason.
 *
 * Hunk source (`cmdiff`): `builtin` is MergeView's own main-thread diff; `daemon` fetches precise hunks
 * from a worker first and passes them through `diffConfig.override`, which is how v1 would render the
 * hunks ticket 07's daemon computes.
 */
import { EditorState, type Extension } from '@codemirror/state'
import { EditorView, lineNumbers, highlightSpecialChars } from '@codemirror/view'
import { syntaxHighlighting, defaultHighlightStyle, foldGutter } from '@codemirror/language'
import { Change, MergeView, type DiffConfig } from '@codemirror/merge'
import { oneDark } from '@codemirror/theme-one-dark'
import { lookupLanguage } from './languages'
import { nextPaint, type MountRequest, type Mounted } from './types'
import DiffWorker from './diff.worker?worker'
import type { HunkRange } from './diff.worker'

const base: Extension = [
  lineNumbers(),
  foldGutter(),
  highlightSpecialChars(),
  syntaxHighlighting(defaultHighlightStyle, { fallback: true }),
  oneDark,
  EditorState.readOnly.of(true),
  EditorView.editable.of(false),
  EditorView.theme({ '&': { height: '100%' }, '.cm-scroller': { overflow: 'auto' } }),
]

/** Obtains precise hunks off the main thread, standing in for the daemon's diff endpoint. */
function daemonHunks(before: string, after: string): Promise<Change[]> {
  return new Promise((resolve, reject) => {
    const worker = new DiffWorker()
    worker.onmessage = (e: MessageEvent<HunkRange[]>) => {
      worker.terminate()
      resolve(e.data.map((h) => new Change(h.fromA, h.toA, h.fromB, h.toB)))
    }
    worker.onerror = (err) => {
      worker.terminate()
      reject(err)
    }
    worker.postMessage({ before, after })
  })
}

export async function mount({ el, fixture, mode, cmdiff, wrap, lang }: MountRequest): Promise<Mounted> {
  const start = performance.now()
  const entry = lookupLanguage(fixture.filename)
  // The language chunk is fetched only for the language actually shown; this await is the cost of
  // the curated-set-plus-extension-point decision, and it is what the "lazy lang chunk" column measures.
  const langSupport = entry && lang ? [await entry.cm()] : []
  const extensions = [base, ...langSupport, ...(wrap ? [EditorView.lineWrapping] : [])]

  let hunksMs: number | undefined
  let diffConfig: DiffConfig | undefined
  if (mode === 'diff' && cmdiff === 'daemon') {
    const h0 = performance.now()
    const hunks = await daemonHunks(fixture.before, fixture.after)
    hunksMs = performance.now() - h0
    diffConfig = { override: () => hunks }
  }

  const t0 = performance.now()
  let view: { destroy: () => void }
  if (mode === 'diff') {
    view = new MergeView({
      a: { doc: fixture.before, extensions },
      b: { doc: fixture.after, extensions },
      parent: el,
      revertControls: undefined, // read-only: reverting a hunk is Restore's job, not the editor's
      highlightChanges: true,
      gutter: true,
      collapseUnchanged: { margin: 3, minSize: 6 },
      diffConfig,
    })
  } else {
    view = new EditorView({ doc: fixture.after, extensions, parent: el })
  }
  const constructMs = performance.now() - t0

  await nextPaint()
  return {
    destroy: () => view.destroy(),
    constructMs,
    hunksMs,
    paintMs: performance.now() - start,
    language: entry && lang ? entry.id : 'plaintext',
  }
}
