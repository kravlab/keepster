/**
 * Paths as `/v1` writes them: relative to the Root, `/`-separated whatever the platform, with `''` for the
 * Root's top.
 *
 * A path is bytes, and a name need not be UTF-8 (ticket 74): the wire then carries its lossy text beside a
 * `_bytes` companion holding the exact bytes in base64url. The page holds every path as one string that
 * keeps those bytes: a UTF-8 path is its text, unchanged, and a byte that is not part of any UTF-8 character
 * becomes the lone surrogate U+DC80 + (byte − 0x80), which no text the daemon sends contains — the
 * "surrogateescape" Python uses for the same job. So two names whose text is the same stay apart, a path
 * still splits at its slashes, and `exactPath` and `wirePath` turn one into the other without loss.
 * `shownPath` is what a person reads; `api.ts` alone meets the wire.
 */

/** The last part of `path`: a file's name. */
export function baseName(path: string): string {
  return path.slice(path.lastIndexOf('/') + 1)
}

/** The directory `path` is in; `''` for one at the Root's top. */
export function dirName(path: string): string {
  return path.slice(0, Math.max(0, path.lastIndexOf('/')))
}

/** The first of the lone surrogates a stray byte (0x80 to 0xFF) is held as. */
const ESCAPE = 0xdc80

/** A held stray byte: a lone surrogate from the escape range, not the second half of a surrogate pair. */
const STRAY = /(?<![\uD800-\uDBFF])[\uDC80-\uDCFF]/

/** The length of the UTF-8 character starting at `at`, or 0 when no valid one does (Unicode table 3-7). */
function characterLength(bytes: Uint8Array, at: number): number {
  const lead = bytes[at] ?? 0
  const within = (offset: number, low = 0x80, high = 0xbf) => {
    const byte = bytes[at + offset]
    return byte !== undefined && byte >= low && byte <= high
  }
  if (lead < 0x80) return 1
  if (lead >= 0xc2 && lead <= 0xdf) return within(1) ? 2 : 0
  if (lead >= 0xe0 && lead <= 0xef) {
    const [low, high] = lead === 0xe0 ? [0xa0, 0xbf] : lead === 0xed ? [0x80, 0x9f] : [0x80, 0xbf]
    return within(1, low, high) && within(2) ? 3 : 0
  }
  if (lead >= 0xf0 && lead <= 0xf4) {
    const [low, high] = lead === 0xf0 ? [0x90, 0xbf] : lead === 0xf4 ? [0x80, 0x8f] : [0x80, 0xbf]
    return within(1, low, high) && within(2) && within(3) ? 4 : 0
  }
  return 0
}

/** `bytes` as the page holds a path: each UTF-8 character as itself, each stray byte escaped. */
function fromBytes(bytes: Uint8Array): string {
  const decoder = new TextDecoder()
  let path = ''
  for (let at = 0; at < bytes.length;) {
    const length = characterLength(bytes, at)
    if (length === 0) {
      path += String.fromCharCode(ESCAPE + (bytes[at] ?? 0) - 0x80)
      at += 1
    } else {
      path += decoder.decode(bytes.subarray(at, at + length))
      at += length
    }
  }
  return path
}

/** The exact bytes of a path the page holds. */
function bytesOf(path: string): Uint8Array {
  const encoder = new TextEncoder()
  const bytes: number[] = []
  for (const character of path) {
    const code = character.codePointAt(0) ?? 0
    // Iterating by code point, a surrogate on its own is one the page escaped: a pair comes as one.
    if (code >= ESCAPE && code <= 0xdcff) bytes.push(code - ESCAPE + 0x80)
    else bytes.push(...encoder.encode(character))
  }
  return Uint8Array.from(bytes)
}

/** `bytes` in base64url without padding (RFC 4648 §5), the spelling of every `_bytes` field. */
function encodeBytes(bytes: Uint8Array): string {
  return btoa(String.fromCharCode(...bytes))
    .replaceAll('+', '-')
    .replaceAll('/', '_')
    .replace(/=+$/, '')
}

/** The bytes a `_bytes` field spells. */
function decodeBytes(text: string): Uint8Array {
  const binary = atob(text.replaceAll('-', '+').replaceAll('_', '/'))
  return Uint8Array.from(binary, (character) => character.charCodeAt(0))
}

/** The path a wire field and its `_bytes` companion name: the companion's bytes when there is one. */
export function exactPath(text: string, bytes: string | null | undefined): string {
  return bytes ? fromBytes(decodeBytes(bytes)) : text
}

/** The paths a wire list and its `_bytes` companion name: each companion entry's bytes where it has one. */
export function exactPaths(texts: readonly string[], bytes: readonly (string | null)[] | null | undefined): string[] {
  return texts.map((text, at) => exactPath(text, bytes?.[at]))
}

/** A path as a wire field carries it: its text — lossy where it is not UTF-8 — and then its bytes. */
export function wirePath(path: string): { text: string; bytes: string | undefined } {
  if (!STRAY.test(path)) return { text: path, bytes: undefined }
  const bytes = bytesOf(path)
  return { text: new TextDecoder().decode(bytes), bytes: encodeBytes(bytes) }
}

/** Paths as a wire list carries them: the texts, and a companion only when one of them needs bytes. */
export function wirePaths(paths: readonly string[]): { texts: string[]; bytes: (string | null)[] | undefined } {
  const wire = paths.map(wirePath)
  return {
    texts: wire.map((path) => path.text),
    bytes: wire.some((path) => path.bytes !== undefined) ? wire.map((path) => path.bytes ?? null) : undefined,
  }
}

/**
 * `path` as a `path=` parameter takes it: every byte percent-encoded but the unreserved set of RFC 3986
 * and `/`, as `keepster_api::encode_path_query` writes it.
 */
export function pathQuery(path: string): string {
  let query = ''
  for (const byte of bytesOf(path)) {
    const character = String.fromCharCode(byte)
    query += /[A-Za-z0-9\-_.~/]/.test(character) ? character : `%${byte.toString(16).toUpperCase().padStart(2, '0')}`
  }
  return query
}

/** What a person reads for `path`: its text, with U+FFFD for what is not UTF-8, as the daemon renders it. */
export function shownPath(path: string): string {
  return STRAY.test(path) ? new TextDecoder().decode(bytesOf(path)) : path
}
