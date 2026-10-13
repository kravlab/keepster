import { describe, expect, it } from 'vitest'
import { momentOf } from './ids'

describe('momentOf', () => {
  it("reads the moment a Snapshot's identity carries, as the daemon reports it", () => {
    // Both pairs are a daemon's own answer: an identity and its `taken_at`.
    expect(momentOf('01M4S82QFVD608VGXJFCBGS594').toISOString()).toBe('2026-10-12T23:45:01.435Z')
    expect(momentOf('01M4S82QCAT6S92580M1K8V9WJ').toISOString()).toBe('2026-10-12T23:45:01.322Z')
  })

  it('reads an identity in lower case as the same moment', () => {
    expect(momentOf('01m4s82qfvd608vgxjfcbgs594').toISOString()).toBe('2026-10-12T23:45:01.435Z')
  })

  it('refuses what is not an identity', () => {
    expect(() => momentOf('latest')).toThrow(RangeError)
    expect(() => momentOf('01M4S82QFU')).toThrow(RangeError)
  })
})
