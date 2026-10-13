/**
 * What a Snapshot's identity says by itself. It is a ULID (`keepster_core::SnapshotId`): its first ten
 * Crockford base32 characters are the millisecond it was taken, which is the same moment the timeline
 * serves as `taken_at`. A component that holds only an identity reads the moment from it rather than
 * being handed the whole Snapshot.
 */

/** Crockford's base32 alphabet, in value order: no I, L, O or U. */
const CROCKFORD = '0123456789ABCDEFGHJKMNPQRSTVWXYZ'
const ULID = /^[0-9A-HJKMNP-TV-Z]{26}$/i

/** The moment a Snapshot identity carries; a `RangeError` for anything that is not one. */
export function momentOf(id: string): Date {
  if (!ULID.test(id)) throw new RangeError(`${id} is not a Snapshot identity`)
  let ms = 0
  for (const char of id.slice(0, 10).toUpperCase()) ms = ms * 32 + CROCKFORD.indexOf(char)
  return new Date(ms)
}
