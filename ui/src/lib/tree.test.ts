import { describe, expect, it } from 'vitest'
import { children, treeRows, type TreeNode } from './tree'
import type { Entry } from './wire'
import type { ShownFile } from './files'

const file = (path: string, over: Partial<Entry> = {}): Entry => ({
  path,
  kind: 'file',
  size: 3,
  binary: false,
  ...over,
})
const dir = (path: string): Entry => ({ path, kind: 'dir' })
const change = (path: string, kind: ShownFile['change']): ShownFile => ({
  path,
  oldPath: null,
  change: kind,
  card: null,
})
const names = (nodes: TreeNode[]) =>
  nodes.map((n) => `${n.dir ? 'd' : 'f'}:${n.name}:${n.change ?? (n.changed ? '*' : '')}`)

describe('children', () => {
  it("lists a directory's children, directories first, each with how it changed", () => {
    const nodes = children(
      '',
      [file('b.txt'), dir('src'), file('a.txt'), dir('docs')],
      [change('a.txt', 'modified'), change('src/lib.rs', 'added')],
    )

    expect(names(nodes)).toEqual(['d:docs:', 'd:src:*', 'f:a.txt:modified', 'f:b.txt:'])
  })

  it('adds what the Snapshot deleted, which it no longer holds', () => {
    const nodes = children(
      'src',
      [file('src/kept.rs')],
      [change('src/gone.rs', 'deleted'), change('src/old/x.rs', 'deleted')],
    )

    expect(names(nodes)).toEqual(['d:old:*', 'f:gone.rs:deleted', 'f:kept.rs:'])
    expect(nodes.map((n) => n.path)).toEqual(['src/old', 'src/gone.rs', 'src/kept.rs'])
  })

  it('keeps each file entry, so an unchanged file can be shown as the Snapshot holds it', () => {
    const big = file('big.bin', { binary: true })

    expect(children('', [big], [])[0]?.entry).toEqual(big)
  })

  it('does not mistake a sibling sharing a prefix for a parent', () => {
    const nodes = children('', [dir('src'), dir('src2')], [change('src2/a.rs', 'modified')])

    expect(names(nodes)).toEqual(['d:src:', 'd:src2:*'])
  })
})

describe('treeRows', () => {
  const root = children('', [dir('src'), file('a.txt')], [change('src/lib.rs', 'modified')])
  const src = children('src', [file('src/lib.rs')], [change('src/lib.rs', 'modified')])

  it('shows only the top level while nothing is expanded', () => {
    expect(treeRows(root, new Set(), new Map()).map((r) => [r.node.path, r.depth])).toEqual([
      ['src', 0],
      ['a.txt', 0],
    ])
  })

  it("shows an expanded directory's children beneath it, one level deeper", () => {
    const rows = treeRows(root, new Set(['src']), new Map([['src', src]]))

    expect(rows.map((r) => [r.node.path, r.depth])).toEqual([
      ['src', 0],
      ['src/lib.rs', 1],
      ['a.txt', 0],
    ])
  })

  it('shows an expanded directory whose children are not loaded yet as loading', () => {
    const rows = treeRows(root, new Set(['src']), new Map())

    expect(rows.map((r) => [r.node.path, r.loading])).toEqual([
      ['src', true],
      ['a.txt', false],
    ])
  })
})
