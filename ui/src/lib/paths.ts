/**
 * Paths as `/v1` writes them: relative to the Root, `/`-separated whatever the platform, with `''` for the
 * Root's top.
 */

/** The last part of `path`: a file's name. */
export function baseName(path: string): string {
  return path.slice(path.lastIndexOf('/') + 1)
}

/** The directory `path` is in; `''` for one at the Root's top. */
export function dirName(path: string): string {
  return path.slice(0, Math.max(0, path.lastIndexOf('/')))
}
