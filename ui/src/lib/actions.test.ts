import { describe, expect, it } from 'vitest'
import { rootActions } from './actions'
import type { Root } from './wire'

const root = (over: Partial<Root> = {}): Root => ({ id: 'r1', path: '/home/u/src', state: 'watching', ...over })

describe('the actions of a Root’s row', () => {
  it('offers the Move first for a Root whose directory is missing', () => {
    expect(rootActions(root({ state: 'unavailable', state_code: 'root_missing' }))).toEqual([
      'move',
      'history',
      'unwatch',
    ])
  })

  it.each([
    ['watching', undefined],
    ['watching', null],
    ['degraded', 'root_overlaps'],
    ['unavailable', 'volume_mismatch'],
  ])('offers history first, then the Move, for a %s Root (%s)', (state, state_code) => {
    expect(rootActions(root({ state, state_code }))).toEqual(['history', 'move', 'unwatch'])
  })
})
