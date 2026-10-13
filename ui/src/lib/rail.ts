/**
 * The Snapshot rail's day headings: what keeps hundreds of rows readable (ticket 15, variant A).
 *
 * Days are calendar days where the user is, so a heading changes at local midnight. A day is told apart by
 * its full date, not by its label: the label leaves out the current year, and a weekday and date come round
 * again in other years.
 */

/** One rail row: a Snapshot, and the heading of its day when it is that day's first (newest) row. */
export interface RailRow<S> {
  snapshot: S
  day: string | null
}

/** How days are told apart and named; the defaults are the browser's locale and time zone. */
export interface DayOptions {
  /** Decides which year counts as the current one, left out of labels. */
  now: Date
  locale?: string
  timeZone?: string
}

/** The rows of a page of Snapshots ordered newest first, as the timeline returns them. */
export function railRows<S extends { taken_at: string }>(snapshots: readonly S[], options: DayOptions): RailRow<S>[] {
  const { now, locale, timeZone } = options
  const key = new Intl.DateTimeFormat('en-CA', { timeZone, year: 'numeric', month: '2-digit', day: '2-digit' })
  const year = new Intl.DateTimeFormat('en-CA', { timeZone, year: 'numeric' })
  const label = new Intl.DateTimeFormat(locale, { timeZone, weekday: 'short', month: 'short', day: 'numeric' })
  const labelWithYear = new Intl.DateTimeFormat(locale, {
    timeZone,
    weekday: 'short',
    month: 'short',
    day: 'numeric',
    year: 'numeric',
  })
  const thisYear = year.format(now)

  let previous: string | null = null
  return snapshots.map((snapshot) => {
    const at = new Date(snapshot.taken_at)
    const day = key.format(at)
    const first = day !== previous
    previous = day
    if (!first) return { snapshot, day: null }
    return { snapshot, day: (year.format(at) === thisYear ? label : labelWithYear).format(at) }
  })
}

/** When a Snapshot was taken, as the hour and minute where the user is: a rail row's time. */
export function timeOfDay(takenAt: string, options: Omit<DayOptions, 'now'> = {}): string {
  const { locale, timeZone } = options
  return new Date(takenAt).toLocaleTimeString(locale, { timeZone, hour: '2-digit', minute: '2-digit' })
}
