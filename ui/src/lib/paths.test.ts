import { describe, expect, it } from 'vitest'
import { baseName, dirName, exactPath, exactPaths, pathQuery, shownPath, wirePath, wirePaths } from './paths'

describe('a Root-relative path', () => {
  it('names its file after the last slash', () => {
    expect(baseName('src/lib/main.rs')).toBe('main.rs')
  })

  it('is its own name at the top of the Root', () => {
    expect(baseName('README.md')).toBe('README.md')
  })

  it('has the directory before the last slash', () => {
    expect(dirName('src/lib/main.rs')).toBe('src/lib')
  })

  it("has the Root's top, '', as the directory of a top-level file", () => {
    expect(dirName('README.md')).toBe('')
  })
})

// Names that are not UTF-8 (ticket 74): the wire carries the lossy text plus the exact bytes in base64url.
describe('a path that is not UTF-8', () => {
  it('is held whole, from the bytes its companion carries', () => {
    const odd = exactPath('\u{fffd}.txt', '_y50eHQ')
    expect(odd).not.toBe('\u{fffd}.txt')
    expect(wirePath(odd)).toEqual({ text: '\u{fffd}.txt', bytes: '_y50eHQ' })
    expect(wirePath(exactPath('src/\u{fffd}', 'c3JjL_4'))).toEqual({ text: 'src/\u{fffd}', bytes: 'c3JjL_4' })
  })

  it('keeps apart two names whose text is the same', () => {
    expect(exactPath('\u{fffd}', '_w')).not.toBe(exactPath('\u{fffd}', '_g'))
  })

  it('splits at its slashes like any other path', () => {
    const inside = exactPath('\u{fffd}/inner.txt', '_i9pbm5lci50eHQ')
    expect(baseName(inside)).toBe('inner.txt')
    expect(dirName(inside)).toBe(exactPath('\u{fffd}', '_g'))
  })

  it('is shown as its lossy text', () => {
    expect(shownPath(exactPath('\u{fffd}.txt', '_y50eHQ'))).toBe('\u{fffd}.txt')
  })

  it('travels in a path parameter as every one of its bytes', () => {
    expect(pathQuery(exactPath('src/\u{fffd} a.txt', 'c3JjL_8gYS50eHQ'))).toBe('src/%FF%20a.txt')
  })

  it('takes valid UTF-8 between its stray bytes as the characters it spells', () => {
    // `é` (C3 A9), a stray FF, then `😀` (F0 9F 98 80), which UTF-16 holds as a surrogate pair.
    const mixed = exactPath('\u{e9}\u{fffd}\u{1f600}', 'w6n_8J-YgA')
    expect(shownPath(mixed)).toBe('\u{e9}\u{fffd}\u{1f600}')
    expect(wirePath(mixed).bytes).toBe('w6n_8J-YgA')
  })
})

describe('a UTF-8 path', () => {
  it('is its text, and travels without bytes', () => {
    expect(exactPath('src/main.rs', undefined)).toBe('src/main.rs')
    expect(exactPath('src/main.rs', null)).toBe('src/main.rs')
    expect(wirePath('src/\u{1f600}.rs')).toEqual({ text: 'src/\u{1f600}.rs', bytes: undefined })
    expect(shownPath('src/\u{1f600}.rs')).toBe('src/\u{1f600}.rs')
  })

  it('travels in a path parameter as its UTF-8, percent-encoded', () => {
    expect(pathQuery('docs/notes é.md')).toBe('docs/notes%20%C3%A9.md')
  })
})

describe('a list of paths on the wire', () => {
  it('has bytes only when one of them needs them, then one per path', () => {
    expect(wirePaths(['a.txt', 'b.txt'])).toEqual({ texts: ['a.txt', 'b.txt'], bytes: undefined })
    expect(wirePaths(['a.txt', exactPath('\u{fffd}', '_w')])).toEqual({
      texts: ['a.txt', '\u{fffd}'],
      bytes: [null, '_w'],
    })
  })

  it('is read back whole from its companion', () => {
    expect(exactPaths(['a.txt', '\u{fffd}'], [null, '_w'])).toEqual(['a.txt', exactPath('\u{fffd}', '_w')])
    expect(exactPaths(['a.txt'], undefined)).toEqual(['a.txt'])
  })
})
