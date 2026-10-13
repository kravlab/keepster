import { describe, expect, it } from 'vitest'
import { baseName, dirName } from './paths'

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
