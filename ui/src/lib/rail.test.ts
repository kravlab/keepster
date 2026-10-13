import { describe, expect, it } from 'vitest'
import { railRows, timeOfDay } from './rail'

const at = (taken_at: string) => ({ id: taken_at, taken_at })
const opts = { now: new Date('2026-10-12T12:00:00Z'), locale: 'en-US', timeZone: 'UTC' }
const days = (rows: { day: string | null }[]) => rows.map((r) => r.day)

describe('railRows', () => {
  it('heads the first Snapshot of each day, newest first', () => {
    const rows = railRows(
      [at('2026-10-12T09:30:00.000Z'), at('2026-10-12T08:00:00.000Z'), at('2026-10-11T23:59:00.000Z')],
      opts,
    )

    expect(days(rows)).toEqual(['Mon, Oct 12', null, 'Sun, Oct 11'])
    expect(rows.map((r) => r.snapshot.id)).toEqual([
      '2026-10-12T09:30:00.000Z',
      '2026-10-12T08:00:00.000Z',
      '2026-10-11T23:59:00.000Z',
    ])
  })

  it('splits days at local midnight, not UTC midnight', () => {
    const rows = railRows([at('2026-10-12T02:00:00.000Z'), at('2026-10-11T22:00:00.000Z')], {
      ...opts,
      timeZone: 'Europe/Berlin',
    })

    // 04:00 and 00:00 in Berlin: the same day.
    expect(days(rows)).toEqual(['Mon, Oct 12', null])
  })

  it('names the year of a day outside the current one', () => {
    const rows = railRows([at('2026-01-02T10:00:00.000Z'), at('2025-01-02T10:00:00.000Z')], opts)

    expect(days(rows)).toEqual(['Fri, Jan 2', 'Thu, Jan 2, 2025'])
  })

  it('heads a day again when the same weekday and date come round in another year', () => {
    const rows = railRows([at('2020-10-12T10:00:00.000Z'), at('2015-10-12T10:00:00.000Z')], opts)

    expect(days(rows)).toEqual(['Mon, Oct 12, 2020', 'Mon, Oct 12, 2015'])
  })

  it('has no rows for no Snapshots', () => {
    expect(railRows([], opts)).toEqual([])
  })
})

describe('timeOfDay', () => {
  it('gives the local hour and minute a Snapshot was taken', () => {
    expect(timeOfDay('2026-10-12T21:05:09.123Z', { locale: 'en-GB', timeZone: 'Europe/Berlin' })).toBe('23:05')
  })
})
