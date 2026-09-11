/**
 * PROTOTYPE (wayfinder ticket 09) — Monaco adapter.
 *
 * `monaco-editor`'s package entry pulls every Monarch grammar and every basic-language contribution;
 * that is the honest shape of "Monaco with highlighting", so the measurement uses it as-is.
 * The diff itself is computed in a web worker, which is why `constructMs` here is NOT comparable to
 * CodeMirror's: Monaco returns before the diff exists.
 */
import EditorWorker from 'monaco-editor/editor/editor.worker?worker'
import * as monaco from 'monaco-editor'
import { lookupLanguage } from './languages'
import { nextPaint, type MountRequest, type Mounted } from './types'

self.MonacoEnvironment = { getWorker: () => new EditorWorker() }

export async function mount({ el, fixture, mode }: MountRequest): Promise<Mounted> {
  const start = performance.now()
  const entry = lookupLanguage(fixture.filename)
  const language = entry?.monaco ?? 'plaintext'

  const t0 = performance.now()
  const models: monaco.editor.ITextModel[] = []
  let dispose: () => void

  if (mode === 'diff') {
    const original = monaco.editor.createModel(fixture.before, language)
    const modified = monaco.editor.createModel(fixture.after, language)
    models.push(original, modified)
    const editor = monaco.editor.createDiffEditor(el, {
      theme: 'vs-dark',
      readOnly: true,
      originalEditable: false,
      automaticLayout: true,
      renderSideBySide: true,
      hideUnchangedRegions: { enabled: true },
      scrollBeyondLastLine: false,
    })
    editor.setModel({ original, modified })
    dispose = () => editor.dispose()
  } else {
    const model = monaco.editor.createModel(fixture.after, language)
    models.push(model)
    const editor = monaco.editor.create(el, {
      model,
      theme: 'vs-dark',
      readOnly: true,
      automaticLayout: true,
      scrollBeyondLastLine: false,
    })
    dispose = () => editor.dispose()
  }
  const constructMs = performance.now() - t0

  await nextPaint()
  return {
    destroy: () => {
      dispose()
      for (const m of models) m.dispose()
    },
    constructMs,
    paintMs: performance.now() - start,
    language: entry ? `${entry.id} → ${language}` : 'plaintext',
  }
}
