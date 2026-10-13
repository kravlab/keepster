import { describe, expect, it } from 'vitest'
import { hasLongLine } from './lines'

describe('hasLongLine', () => {
  it('allows a line exactly at the limit', () => {
    expect(hasLongLine('x'.repeat(10), 10)).toBe(false)
  })

  it('finds a line one past the limit', () => {
    expect(hasLongLine('x'.repeat(11), 10)).toBe(true)
  })

  it('finds a long line between short ones', () => {
    expect(hasLongLine(`a\n${'x'.repeat(11)}\nb\n`, 10)).toBe(true)
  })

  it('measures lines, not the whole text', () => {
    expect(hasLongLine(`${'x'.repeat(10)}\n`.repeat(5), 10)).toBe(false)
  })

  it('finds a long last line without a newline', () => {
    expect(hasLongLine(`a\n${'x'.repeat(11)}`, 10)).toBe(true)
  })

  it('finds nothing in an empty text', () => {
    expect(hasLongLine('', 10)).toBe(false)
  })
})
