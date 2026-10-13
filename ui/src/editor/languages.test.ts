import { describe, expect, it } from 'vitest'
import { lookupLanguage } from './languages'

describe('lookupLanguage', () => {
  it('picks a language by the extension of the last path segment', () => {
    expect(lookupLanguage('src/main.rs')).toBeDefined()
    expect(lookupLanguage('src/main.rs')).toBe(lookupLanguage('lib.rs'))
    expect(lookupLanguage('a.rs')).not.toBe(lookupLanguage('a.ts'))
  })

  it('treats every extension of a language alike', () => {
    expect(lookupLanguage('a.tsx')).toBe(lookupLanguage('a.ts'))
    expect(lookupLanguage('a.yml')).toBe(lookupLanguage('a.yaml'))
  })

  it('ignores the case of the extension', () => {
    expect(lookupLanguage('README.MD')).toBe(lookupLanguage('notes.md'))
  })

  it('reads the extension of the file, not of a directory', () => {
    expect(lookupLanguage('src.rs/Makefile')).toBeUndefined()
  })

  it('highlights nothing it does not know', () => {
    expect(lookupLanguage('picture.png')).toBeUndefined()
    expect(lookupLanguage('LICENSE')).toBeUndefined()
  })

  it('loads the grammar only when asked', async () => {
    const grammar = await lookupLanguage('data.json')?.load()

    expect(grammar).toBeDefined()
  })
})
