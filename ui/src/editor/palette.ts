/**
 * The editor's colors (ticket 66), all read from the page's palette variables (`../app.css`), so the
 * editor and the diff change with the theme and never carry a palette of their own.
 *
 * The rules override `@codemirror/merge`'s base theme selector for selector: a theme's rules come after
 * the base theme's at equal specificity, so they win.
 */
import { HighlightStyle } from '@codemirror/language'
import { EditorView } from '@codemirror/view'
import { tags as t } from '@lezer/highlight'

/** A color mixed into transparency: how strongly a changed line or its changed text is tinted. */
const tint = (color: string, percent: number) => `color-mix(in srgb, var(${color}) ${percent}%, transparent)`

/** Syntax colors by token role. */
export const highlightStyle = HighlightStyle.define([
  { tag: [t.keyword, t.operatorKeyword, t.modifier, t.controlKeyword], color: 'var(--code-keyword)' },
  { tag: [t.string, t.special(t.string), t.regexp, t.character], color: 'var(--code-string)' },
  { tag: [t.number, t.bool, t.null, t.atom, t.unit], color: 'var(--code-number)' },
  { tag: [t.comment, t.lineComment, t.blockComment, t.docComment], color: 'var(--code-comment)', fontStyle: 'italic' },
  {
    tag: [t.function(t.variableName), t.function(t.propertyName), t.definition(t.function(t.variableName))],
    color: 'var(--code-function)',
  },
  { tag: [t.typeName, t.className, t.namespace, t.tagName], color: 'var(--code-type)' },
  { tag: [t.propertyName, t.attributeName, t.labelName], color: 'var(--code-property)' },
  { tag: [t.meta, t.processingInstruction, t.annotation], color: 'var(--code-meta)' },
  { tag: t.heading, color: 'var(--code-keyword)', fontWeight: 'bold' },
  { tag: t.strong, fontWeight: 'bold' },
  { tag: t.emphasis, fontStyle: 'italic' },
  { tag: t.strikethrough, textDecoration: 'line-through' },
  { tag: t.link, color: 'var(--accent)', textDecoration: 'underline' },
  { tag: t.invalid, color: 'var(--danger)' },
])

/** The editor's surface, gutters and the diff's changed lines and text. */
export const editorTheme = EditorView.theme({
  '&': { height: '100%', backgroundColor: 'var(--panel)', color: 'var(--fg)' },
  '.cm-scroller': { overflow: 'auto' },
  '.cm-gutters': { backgroundColor: 'var(--bg)', color: 'var(--muted)', borderRight: '1px solid var(--line)' },
  '.cm-specialChar': { color: 'var(--danger)' },
  '&.cm-merge-a .cm-changedLine, .cm-deletedChunk': { backgroundColor: tint('--danger', 12) },
  '&.cm-merge-b .cm-changedLine, .cm-inlineChangedLine': { backgroundColor: tint('--added', 12) },
  '&.cm-merge-a .cm-changedText, .cm-deletedChunk .cm-deletedText': { background: tint('--danger', 32) },
  '&.cm-merge-b .cm-changedText': { background: tint('--added', 32) },
  '&.cm-merge-b .cm-deletedText': { background: tint('--danger', 32) },
  '&.cm-merge-a .cm-changedLineGutter, .cm-deletedLineGutter': { background: 'var(--danger)' },
  '&.cm-merge-b .cm-changedLineGutter': { background: 'var(--added)' },
  '.cm-collapsedLines': { color: 'var(--muted)', background: 'var(--bg)' },
})
