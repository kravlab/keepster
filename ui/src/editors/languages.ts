/**
 * PROTOTYPE (wayfinder ticket 09) — the curated 12-language set, with the extension point.
 *
 * The decision this encodes: v1 highlights a curated set and can be extended. Adding a 13th language
 * is one row here; for CodeMirror that costs one lazily-loaded chunk, for Monaco nothing, because
 * Monaco already shipped every Monarch grammar whether or not this table names it.
 */
import type { Extension } from '@codemirror/state'

export interface LanguageEntry {
  /** our own id, used in the UI */
  id: string
  /** file extensions (no dot) and exact basenames that select this language */
  match: string[]
  /** lazily imported CodeMirror language support — one chunk per language */
  cm: () => Promise<Extension>
  /**
   * Monaco's language id. Monaco ships no TOML grammar, so TOML falls back to `ini`:
   * a finding, not an oversight.
   */
  monaco: string
}

const stream = async (mod: Promise<{ [k: string]: unknown }>, name: string): Promise<Extension> => {
  const { StreamLanguage } = await import('@codemirror/language')
  const m = await mod
  return StreamLanguage.define(m[name] as Parameters<typeof StreamLanguage.define>[0])
}

export const LANGUAGES: LanguageEntry[] = [
  { id: 'rust', match: ['rs'], monaco: 'rust', cm: async () => (await import('@codemirror/lang-rust')).rust() },
  {
    id: 'typescript',
    match: ['ts', 'tsx', 'mts', 'cts'],
    monaco: 'typescript',
    cm: async () => (await import('@codemirror/lang-javascript')).javascript({ typescript: true }),
  },
  {
    id: 'javascript',
    match: ['js', 'mjs', 'cjs', 'jsx'],
    monaco: 'javascript',
    cm: async () => (await import('@codemirror/lang-javascript')).javascript(),
  },
  { id: 'json', match: ['json'], monaco: 'json', cm: async () => (await import('@codemirror/lang-json')).json() },
  {
    id: 'toml',
    match: ['toml'],
    monaco: 'ini',
    cm: () => stream(import('@codemirror/legacy-modes/mode/toml'), 'toml'),
  },
  { id: 'yaml', match: ['yaml', 'yml'], monaco: 'yaml', cm: async () => (await import('@codemirror/lang-yaml')).yaml() },
  {
    id: 'markdown',
    match: ['md', 'markdown'],
    monaco: 'markdown',
    cm: async () => (await import('@codemirror/lang-markdown')).markdown(),
  },
  {
    id: 'python',
    match: ['py'],
    monaco: 'python',
    cm: async () => (await import('@codemirror/lang-python')).python(),
  },
  { id: 'go', match: ['go'], monaco: 'go', cm: async () => (await import('@codemirror/lang-go')).go() },
  { id: 'sql', match: ['sql'], monaco: 'sql', cm: async () => (await import('@codemirror/lang-sql')).sql() },
  {
    id: 'shell',
    match: ['sh', 'bash', 'zsh'],
    monaco: 'shell',
    cm: () => stream(import('@codemirror/legacy-modes/mode/shell'), 'shell'),
  },
  { id: 'html', match: ['html', 'htm'], monaco: 'html', cm: async () => (await import('@codemirror/lang-html')).html() },
  { id: 'css', match: ['css'], monaco: 'css', cm: async () => (await import('@codemirror/lang-css')).css() },
]

export function lookupLanguage(filename: string): LanguageEntry | undefined {
  const base = filename.split('/').pop() ?? filename
  const ext = base.includes('.') ? base.split('.').pop()!.toLowerCase() : base.toLowerCase()
  return LANGUAGES.find((l) => l.match.includes(ext))
}
