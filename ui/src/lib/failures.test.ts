import { describe, expect, it, vi } from 'vitest'
import { ApiError, SessionEnded, UnexpectedAnswer } from './api'
import { failureText, pathRefusalText } from './failures'
import { HunkMismatch } from './mismatch'
import { strings } from './strings'
import type { Root } from './wire'

describe('failureText', () => {
  it('hands a dead session to the shell and still names it', () => {
    const ended = vi.fn()

    expect(failureText(new SessionEnded('gone'), ended)).toBe(strings.sessionEnded.before)
    expect(ended).toHaveBeenCalledOnce()
  })

  it("shows the daemon's own explanation of a refusal", () => {
    expect(failureText(new ApiError(404, 'no Snapshot s', 'snapshot_not_found'), vi.fn())).toBe('no Snapshot s')
  })

  it('names the status of a refusal the daemon did not explain', () => {
    expect(failureText(new ApiError(502), vi.fn())).toBe(strings.httpStatus(502))
  })

  it('says so when the daemon answered something the UI cannot read', () => {
    expect(failureText(new UnexpectedAnswer('no file'), vi.fn())).toBe(strings.unexpectedAnswer)
  })

  it('says so when the diff does not fit the versions it was drawn over', () => {
    expect(failureText(new HunkMismatch('old line 3'), vi.fn())).toBe(strings.hunkMismatch)
  })

  it('passes on what the browser said about anything else', () => {
    expect(failureText(new TypeError('Failed to fetch'), vi.fn())).toBe('Failed to fetch')
    expect(failureText('odd', vi.fn())).toBe('odd')
  })

  it('leaves the session alone for any other failure', () => {
    const ended = vi.fn()
    failureText(new ApiError(500), ended)

    expect(ended).not.toHaveBeenCalled()
  })
})

describe('pathRefusalText', () => {
  const overlaps = new ApiError(409, '/home/u/src/app overlaps the Root 0190a3c4', 'root_overlaps', {
    path: '/home/u/src/app',
    root: '0190a3c4',
  })
  const known: Root = { id: '0190a3c4', path: '/home/u/src', state: 'watching' }
  const detail = (e: unknown) => (e instanceof Error ? e.message : String(e))

  it('names an overlapped Root by its path when it is known', () => {
    expect(
      pathRefusalText('/home/u/src/app', overlaps, {
        failure: detail,
        rootById: (id) => (id === known.id ? known : undefined),
      }),
    ).toBe('/home/u/src/app overlaps the Root at /home/u/src')
  })

  it("keeps the daemon's reason for an overlapped Root it does not know, and for any other refusal", () => {
    const invalid = new ApiError(400, '/home/u/notes.txt is not a directory', 'path_invalid')

    expect(pathRefusalText('/home/u/src/app', overlaps, { failure: detail, rootById: () => undefined })).toBe(
      '/home/u/src/app overlaps the Root 0190a3c4',
    )
    expect(pathRefusalText('/home/u/notes.txt', invalid, { failure: detail, rootById: () => known })).toBe(
      '/home/u/notes.txt is not a directory',
    )
  })
})
