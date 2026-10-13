/**
 * The CodeMirror 6 editor behind the diff pane (ticket 09), loaded lazily with its grammars.
 *
 * Read-only throughout, and deliberately not `basicSetup`: a history viewer needs no autocomplete, search
 * panel or edit keymap. MergeView never diffs on its own: its chunks come from the daemon's hunks through
 * `diffConfig.override` (see `../lib/hunks.ts`, whose line contract `lineSeparator` below keeps).
 */
import { Compartment, EditorState, type Extension } from '@codemirror/state'
import { EditorView, highlightSpecialChars, lineNumbers } from '@codemirror/view'
import { defaultHighlightStyle, syntaxHighlighting } from '@codemirror/language'
import { MergeView } from '@codemirror/merge'
import { daemonChanges } from '../lib/hunks'
import type { DiffHunk } from '../lib/wire'
import { lookupLanguage } from './languages'

/**
 * CodeMirror's own special characters without `\r`. Documents are split on `\n` only, so a CRLF file's
 * lines end in `\r`; marking it would put a placeholder at the end of every line of such a file. The `g`
 * flag is CodeMirror's requirement.
 */
const SPECIAL_CHARS =
  // eslint-disable-next-line no-control-regex -- control characters are exactly what it matches
  /[\u0000-\u0008\u000a-\u000c\u000e-\u001f\u007f-\u009f\u00ad\u061c\u200b\u200e\u200f\u2028\u2029\u202d\u202e\u2066\u2067\u2069\ufeff\ufff9-\ufffc]/gu

const base: Extension = [
  lineNumbers(),
  highlightSpecialChars({ specialChars: SPECIAL_CHARS }),
  syntaxHighlighting(defaultHighlightStyle, { fallback: true }),
  EditorState.lineSeparator.of('\n'),
  EditorState.readOnly.of(true),
  EditorView.editable.of(false),
  EditorView.theme({ '&': { height: '100%' }, '.cm-scroller': { overflow: 'auto' } }),
]

/** What the pane asks for, whichever view it is. */
interface EditorRequest {
  /** Where the editor goes; emptied by `destroy`. */
  el: HTMLElement
  /** The file's path, which picks the grammar. */
  path: string
  /** Wrap long lines; off by default (ticket 09), switched later with `setWrap`. */
  wrap: boolean
  /** Attach the grammar; the pane turns it off for a file with a line over the limit (`../lib/lines.ts`). */
  highlight: boolean
}

/** A mounted editor. */
export interface Shown {
  /** Switches line wrapping in place, keeping the scroll position. */
  setWrap(on: boolean): void
  /** Takes the editor down and out of its element. */
  destroy(): void
}

/** The extensions for one side: the grammar, loaded only now, and wrapping in its compartment. */
async function extensions({ path, wrap, highlight }: EditorRequest, wrapping: Compartment): Promise<Extension[]> {
  const language = highlight ? lookupLanguage(path) : undefined
  return [base, language ? await language.load() : [], wrapping.of(wrap ? EditorView.lineWrapping : [])]
}

/** Switches wrapping on every view of one mount. */
function wrapper(views: readonly EditorView[], wrapping: Compartment): (on: boolean) => void {
  return (on) => {
    for (const view of views) view.dispatch({ effects: wrapping.reconfigure(on ? EditorView.lineWrapping : []) })
  }
}

/** Two versions side by side, their changes drawn from the daemon's `hunks`. */
export async function showDiff(
  request: EditorRequest & { a: string; b: string; hunks: readonly DiffHunk[] },
): Promise<Shown> {
  const wrapping = new Compartment()
  const ext = await extensions(request, wrapping)
  const view = new MergeView({
    a: { doc: request.a, extensions: ext },
    b: { doc: request.b, extensions: ext },
    parent: request.el,
    highlightChanges: true,
    gutter: true,
    collapseUnchanged: { margin: 3, minSize: 6 },
    diffConfig: { override: daemonChanges(request.hunks) },
  })
  return { setWrap: wrapper([view.a, view.b], wrapping), destroy: () => view.destroy() }
}

/** One version alone: a file in a Root's first Snapshot, which has nothing before it to differ from. */
export async function showFile(request: EditorRequest & { doc: string }): Promise<Shown> {
  const wrapping = new Compartment()
  const view = new EditorView({
    doc: request.doc,
    extensions: await extensions(request, wrapping),
    parent: request.el,
  })
  return { setWrap: wrapper([view], wrapping), destroy: () => view.destroy() }
}
