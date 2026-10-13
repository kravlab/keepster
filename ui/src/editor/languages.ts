/**
 * The curated set of highlighted languages, with the extension point (ticket 09).
 *
 * Adding a language is one row here, and costs one lazily loaded chunk: a grammar is fetched only when a
 * file of its language is shown, which keeps grammars out of the eager bundle (ticket 64's budget).
 */
import { StreamLanguage } from '@codemirror/language'
import type { Extension } from '@codemirror/state'

/** One highlighted language: which files it covers, and how its grammar is loaded. */
export interface LanguageEntry {
  /** file extensions (no dot) and exact basenames that select this language */
  match: string[]
  /** lazily imported CodeMirror language support — one chunk per language */
  load: () => Promise<Extension>
}

/**
 * A legacy-mode grammar, wrapped as CodeMirror language support. `@codemirror/language` itself is imported
 * statically: the editor chunk holds it already, and only the mode is worth a chunk of its own.
 */
const stream = async (mod: Promise<{ [k: string]: unknown }>, name: string): Promise<Extension> => {
  const m = await mod
  return StreamLanguage.define(m[name] as Parameters<typeof StreamLanguage.define>[0])
}

/** The curated set, first match wins. */
const LANGUAGES: LanguageEntry[] = [
  { match: ['rs'], load: async () => (await import('@codemirror/lang-rust')).rust() },
  {
    match: ['ts', 'tsx', 'mts', 'cts'],
    load: async () => (await import('@codemirror/lang-javascript')).javascript({ typescript: true }),
  },
  { match: ['js', 'mjs', 'cjs', 'jsx'], load: async () => (await import('@codemirror/lang-javascript')).javascript() },
  { match: ['json'], load: async () => (await import('@codemirror/lang-json')).json() },
  { match: ['toml'], load: () => stream(import('@codemirror/legacy-modes/mode/toml'), 'toml') },
  { match: ['yaml', 'yml'], load: async () => (await import('@codemirror/lang-yaml')).yaml() },
  { match: ['md', 'markdown'], load: async () => (await import('@codemirror/lang-markdown')).markdown() },
  { match: ['py'], load: async () => (await import('@codemirror/lang-python')).python() },
  { match: ['go'], load: async () => (await import('@codemirror/lang-go')).go() },
  { match: ['sql'], load: async () => (await import('@codemirror/lang-sql')).sql() },
  { match: ['sh', 'bash', 'zsh'], load: () => stream(import('@codemirror/legacy-modes/mode/shell'), 'shell') },
  { match: ['html', 'htm'], load: async () => (await import('@codemirror/lang-html')).html() },
  { match: ['css'], load: async () => (await import('@codemirror/lang-css')).css() },
]

/** The language a path is highlighted as, by its extension or exact basename; none for the rest. */
export function lookupLanguage(filename: string): LanguageEntry | undefined {
  const base = filename.split('/').pop() ?? filename
  const ext = base.includes('.') ? base.split('.').pop()!.toLowerCase() : base.toLowerCase()
  return LANGUAGES.find((l) => l.match.includes(ext))
}
