/**
 * The actions a Root's row on the Root management page offers, in the order the row shows them (#32).
 * Unwatch… is always last. A Root whose directory is missing (`root_missing`) offers the Move first, since
 * a Move is the most likely reason and recording it is the way out of that state; any other Root offers
 * its history first and the Move after it.
 */
import type { Root } from './wire'

/** One action of a Root's row: record a Move, open its history, or stop watching it. */
export type RootAction = 'move' | 'history' | 'unwatch'

/** The actions of `root`'s row, in the order they are shown. */
export function rootActions(root: Root): RootAction[] {
  return root.state_code === 'root_missing' ? ['move', 'history', 'unwatch'] : ['history', 'move', 'unwatch']
}
