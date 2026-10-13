/**
 * The file column's tree: a view option beside the flat list of changes (ticket 15). It may widen to all a
 * Snapshot holds, so a file the Snapshot never touched can be chosen, and it is read a directory at a time
 * (`entries` is shallow by default, since a 50 000-file tree is expanded lazily anyway).
 *
 * A directory's children are what the Snapshot holds there, plus what it deleted there: a deleted path is
 * not held, yet it is exactly what the user came to find. Every node says how it changed, and a directory
 * whether anything beneath it did.
 */
import type { ShownFile } from './files'
import { dirName } from './paths'
import type { Entry } from './wire'

/** One file or directory of the tree. */
export interface TreeNode {
  /** Root-relative, `/`-separated. */
  path: string
  name: string
  dir: boolean
  /** How the file changed in the Snapshot; `null` for an unchanged file and for a directory. */
  change: ShownFile['change'] | null
  /** For a directory, whether anything beneath it changed. */
  changed: boolean
  /** What the Snapshot holds at a file's path; absent for one it deleted. */
  entry?: Entry
}

/** One rendered row: a node at its depth, and whether it is an expanded directory still loading. */
export interface TreeRow {
  node: TreeNode
  depth: number
  loading: boolean
}

/** Directories first, then by name in code-point order, as the daemon orders paths. */
const order = (a: TreeNode, b: TreeNode) =>
  Number(b.dir) - Number(a.dir) || (a.name < b.name ? -1 : a.name > b.name ? 1 : 0)

/**
 * The children of directory `dir` (`''` for the Root's top), from what the Snapshot holds directly in it
 * and the Snapshot's changes.
 */
export function children(dir: string, entries: readonly Entry[], changes: readonly ShownFile[]): TreeNode[] {
  const prefix = dir === '' ? '' : `${dir}/`
  const below = changes.filter((c) => c.path.startsWith(prefix))
  const changeOf = new Map(below.map((c) => [c.path, c.change]))
  const nodes = new Map<string, TreeNode>()

  for (const entry of entries) {
    if (dirName(entry.path) !== dir) continue
    const isDir = entry.kind === 'dir'
    nodes.set(entry.path, {
      path: entry.path,
      name: entry.path.slice(prefix.length),
      dir: isDir,
      change: isDir ? null : (changeOf.get(entry.path) ?? null),
      changed: isDir && below.some((c) => c.path.startsWith(`${entry.path}/`)),
      ...(isDir ? {} : { entry }),
    })
  }

  // What the Snapshot no longer holds: deleted files here, and directories that held only deleted files.
  for (const c of below) {
    const rest = c.path.slice(prefix.length)
    const slash = rest.indexOf('/')
    const path = slash === -1 ? c.path : prefix + rest.slice(0, slash)
    if (nodes.has(path)) continue
    nodes.set(path, {
      path,
      name: path.slice(prefix.length),
      dir: slash !== -1,
      change: slash === -1 ? c.change : null,
      changed: slash !== -1,
    })
  }

  return [...nodes.values()].sort(order)
}

/** The rows to draw: the top level, and beneath each expanded directory its children once loaded. */
export function treeRows(
  top: readonly TreeNode[],
  expanded: ReadonlySet<string>,
  loaded: ReadonlyMap<string, readonly TreeNode[]>,
): TreeRow[] {
  const rows: TreeRow[] = []
  const walk = (nodes: readonly TreeNode[], depth: number) => {
    for (const node of nodes) {
      const open = node.dir && expanded.has(node.path)
      const inside = open ? loaded.get(node.path) : undefined
      rows.push({ node, depth, loading: open && inside === undefined })
      if (inside) walk(inside, depth + 1)
    }
  }
  walk(top, 0)
  return rows
}
