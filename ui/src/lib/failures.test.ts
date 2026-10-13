import { describe, expect, it, vi } from 'vitest'
import { ApiError, SessionEnded, UnexpectedAnswer } from './api'
import { failureText } from './failures'
import { HunkMismatch } from './mismatch'
import { strings } from './strings'

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
