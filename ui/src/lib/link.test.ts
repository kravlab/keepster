import { describe, expect, it } from 'vitest'
import { ApiError } from './api'
import type { RefusalWording } from './failures'
import { sendRootPath } from './link'
import type { Root } from './wire'

const root: Root = { id: 'r1', path: '/home/u/src', state: 'watching' }
const known: Root = { id: '0190a3c4', path: '/home/u/work', state: 'watching' }
const reason = (e: unknown) => (e instanceof Error ? e.message : String(e))
/** The daemon's explanation, the live list knowing only `known`. */
const wording: RefusalWording = { failure: reason, rootById: (id) => (id === known.id ? known : undefined) }

describe('sending a path for a Root', () => {
  it('hands the daemon’s answer on as it is', async () => {
    const outcome = await sendRootPath('/home/u/src', () => Promise.resolve({ kind: 'moved' as const, root }), wording)

    expect(outcome).toEqual({ kind: 'moved', root })
  })

  it('asks the link question for a path through a link', async () => {
    const outcome = await sendRootPath(
      '/home/u/proj',
      () => Promise.resolve({ kind: 'throughLink' as const, path: '/home/u/proj', realPath: '/mnt/work/proj' }),
      wording,
    )

    expect(outcome).toEqual({ kind: 'askingLink', path: '/home/u/proj', realPath: '/mnt/work/proj' })
  })

  it('is refused with the reason, naming an overlapped Root it knows by its path', async () => {
    const overlap = new ApiError(409, '/home/u/work/app overlaps the Root 0190a3c4', 'root_overlaps', {
      root: '0190a3c4',
    })

    const outcome = await sendRootPath('/home/u/work/app', () => Promise.reject(overlap), wording)

    expect(outcome).toEqual({
      kind: 'failed',
      path: '/home/u/work/app',
      reason: '/home/u/work/app overlaps the Root at /home/u/work',
    })
  })

  it('is refused with the failure’s own words otherwise', async () => {
    const outcome = await sendRootPath(
      '/home/u/notes.txt',
      () => Promise.reject(new ApiError(400, '/home/u/notes.txt is not a directory', 'path_invalid')),
      wording,
    )

    expect(outcome).toEqual({
      kind: 'failed',
      path: '/home/u/notes.txt',
      reason: '/home/u/notes.txt is not a directory',
    })
  })
})
