import { describe, expect, it } from 'vitest'
import {
  emptyAnswers,
  outcomeCounts,
  questions,
  ready,
  restoreRequest,
  unrecoverableReasons,
  type Answers,
} from './restore'
import type { RestoredPath } from './wire'

const restored = (path: string): RestoredPath => ({ path, outcome: 'restored' })
const blocked = (path: string, ...issues: RestoredPath['issues'] & object): RestoredPath => ({
  path,
  outcome: 'blocked',
  issues,
})
const answers = (over: Partial<Answers> = {}): Answers => ({ ...emptyAnswers(), ...over })

describe('questions', () => {
  it('asks nothing of a plan that proceeds as it is', () => {
    expect(questions([restored('a.txt'), { path: 'b.txt', outcome: 'created' }])).toEqual([])
  })

  it('asks each confirmation once, naming every path that needs it', () => {
    const asked = questions([
      blocked('a/x.txt', { reason: 'parent_missing', confirm: 'create_parents' }),
      blocked('b/y.txt', { reason: 'parent_missing', confirm: 'create_parents' }),
      blocked('z.txt', { reason: 'hardlinked', confirm: 'break_hardlinks', paths: ['w.txt'] }),
    ])

    expect(asked).toEqual([
      { kind: 'confirm', confirm: 'create_parents', paths: ['a/x.txt', 'b/y.txt'] },
      { kind: 'confirm', confirm: 'break_hardlinks', paths: ['z.txt'] },
    ])
  })

  it('asks each resolution for its own path, with the issue that raised it', () => {
    const issue = { reason: 'name_case', resolution: 'name_case', existing: 'README.MD' } as const

    expect(questions([blocked('README.md', issue)])).toEqual([
      { kind: 'resolution', resolution: 'name_case', path: 'README.md', issue },
    ])
  })

  it('gathers what cannot be restored at all into one question, settled only by skipping', () => {
    const asked = questions([
      blocked('a.txt', { reason: 'type_conflict', at: 'a.txt' }),
      blocked('b.txt', { reason: 'link_check_failed', at: 'b.txt' }),
    ])

    expect(asked).toEqual([{ kind: 'unrestorable', paths: ['a.txt', 'b.txt'] }])
  })

  it('treats a reason this page does not know, with nothing to settle it, as unrestorable', () => {
    expect(questions([blocked('a.txt', { reason: 'from_the_future' })])).toEqual([
      { kind: 'unrestorable', paths: ['a.txt'] },
    ])
  })
})

describe('ready', () => {
  const plan = [
    blocked('a/x.txt', { reason: 'parent_missing', confirm: 'create_parents' }),
    blocked('m.sh', { reason: 'modes_differ', resolution: 'modes', current_mode: 0o755, snapshot_mode: 0o644 }),
  ]

  it('is not ready while any question is unanswered', () => {
    expect(ready(plan, answers())).toBe(false)
    expect(ready(plan, answers({ confirm: ['create_parents'] }))).toBe(false)
    expect(ready(plan, answers({ resolutions: { 'm.sh': { modes: 'keep' } } }))).toBe(false)
  })

  it('is ready once every question is answered', () => {
    expect(ready(plan, answers({ confirm: ['create_parents'], resolutions: { 'm.sh': { modes: 'keep' } } }))).toBe(true)
  })

  it('is ready at once for a plan with no questions, and never for an empty one', () => {
    expect(ready([restored('a.txt')], answers())).toBe(true)
    expect(ready([], answers())).toBe(false)
  })

  it('takes a blank new name as no answer', () => {
    const named = [blocked('a:b', { reason: 'name_invalid', resolution: 'rename_to' })]

    expect(ready(named, answers({ resolutions: { 'a:b': { rename_to: '  ' } } }))).toBe(false)
    expect(ready(named, answers({ resolutions: { 'a:b': { rename_to: 'a-b' } } }))).toBe(true)
  })

  it('settles a name collision by writing any one of the paths that are one file', () => {
    const plan = [
      blocked('A.txt', { reason: 'name_collision', resolution: 'write', paths: ['a.txt'] }),
      blocked('a.txt', { reason: 'name_collision', resolution: 'write', paths: ['A.txt'] }),
    ]

    expect(ready(plan, answers())).toBe(false)
    expect(ready(plan, answers({ resolutions: { 'a.txt': { write: true } } }))).toBe(true)
  })

  it('needs skipping for what cannot be restored, and something left to write', () => {
    const mixed = [restored('a.txt'), blocked('b.txt', { reason: 'type_conflict', at: 'b.txt' })]

    expect(ready(mixed, answers())).toBe(false)
    expect(ready(mixed, answers({ skipUnresolvable: true }))).toBe(true)
    expect(ready([blocked('b.txt', { reason: 'type_conflict' })], answers({ skipUnresolvable: true }))).toBe(false)
  })
})

describe('restoreRequest', () => {
  it('sends the answers given, and only those that a question asked', () => {
    const plan = [blocked('m.sh', { reason: 'modes_differ', resolution: 'modes' })]
    const given = answers({
      confirm: ['create_parents'],
      resolutions: { 'm.sh': { modes: 'snapshot' }, 'gone.txt': { modes: 'keep' } },
      skipUnresolvable: true,
    })

    expect(restoreRequest('S1', ['m.sh'], plan, given, false)).toEqual({
      from: 'S1',
      paths: ['m.sh'],
      dry_run: false,
      confirm: [],
      resolutions: { 'm.sh': { modes: 'snapshot' } },
      skip_unresolvable: false,
    })
  })

  it('skips only when the plan showed what would be skipped', () => {
    const plan = [restored('a.txt'), blocked('b.txt', { reason: 'type_conflict' })]

    expect(restoreRequest('S1', ['.'], plan, answers({ skipUnresolvable: true }), false).skip_unresolvable).toBe(true)
  })

  it('asks for a dry run with no answers before there is a plan', () => {
    expect(restoreRequest('S1', ['a.txt'], [], answers(), true)).toEqual({
      from: 'S1',
      paths: ['a.txt'],
      dry_run: true,
      confirm: [],
      resolutions: {},
      skip_unresolvable: false,
    })
  })

  it('sends a confirmation a question asked for, and a trimmed new name', () => {
    const plan = [
      blocked('a/x.txt', { reason: 'parent_missing', confirm: 'create_parents' }),
      blocked('a:b', { reason: 'name_invalid', resolution: 'rename_to' }),
    ]
    const given = answers({ confirm: ['create_parents'], resolutions: { 'a:b': { rename_to: ' a-b ' } } })

    expect(restoreRequest('S1', ['a'], plan, given, false)).toMatchObject({
      confirm: ['create_parents'],
      resolutions: { 'a:b': { rename_to: 'a-b' } },
    })
  })
})

describe('outcomeCounts', () => {
  it('counts what happened to every file, by outcome', () => {
    expect(
      outcomeCounts([
        restored('a'),
        restored('b'),
        { path: 'c', outcome: 'created' },
        { path: 'd', outcome: 'failed', error: 'busy' },
      ]),
    ).toEqual({ restored: 2, created: 1, failed: 1 })
  })
})

describe('unrecoverableReasons', () => {
  it("names each reason the Safety Snapshot cannot hold some file's content, once", () => {
    const plan = [
      blocked('big.iso', { reason: 'overwrite_unrecoverable', confirm: 'overwrite_unrecoverable', why: 'too_large' }),
      blocked('huge.bin', { reason: 'overwrite_unrecoverable', confirm: 'overwrite_unrecoverable', why: 'too_large' }),
      blocked('secret.key', { reason: 'overwrite_unrecoverable', confirm: 'overwrite_unrecoverable', why: 'excluded' }),
    ]

    expect(unrecoverableReasons(plan)).toEqual(['too_large', 'excluded'])
  })

  it('names none for other issues, nor for one that gives no reason', () => {
    const plan = [
      blocked('a/x.txt', { reason: 'parent_missing', confirm: 'create_parents' }),
      blocked('y.txt', { reason: 'overwrite_unrecoverable', confirm: 'overwrite_unrecoverable' }),
    ]

    expect(unrecoverableReasons(plan)).toEqual([])
  })
})
