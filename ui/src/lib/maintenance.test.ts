import { describe, expect, it } from 'vitest'
import { maintenanceNotice } from './maintenance'
import type { GitStatus } from './wire'

const install = { command: 'sudo apt install git', note: 'Or from the vendor.', link: 'https://example.com/git' }

describe('the notice that store maintenance is unavailable', () => {
  it('says which git the daemon found, at which version, where it knows both', () => {
    const notice = maintenanceNotice({ minimum: '2.30', path: '/usr/bin/git', version: '2.20.1', reason: 'too_old' })

    expect(notice.title).toBe('Store maintenance is unavailable.')
    expect(notice.git).toBe('git 2.20.1 at /usr/bin/git')
  })

  it('says where the git is without a version it did not get', () => {
    expect(maintenanceNotice({ minimum: '2.30', path: '/opt/git', reason: 'not_executable' }).git).toBe('git /opt/git')
  })

  it('has no git line where none was found', () => {
    expect(maintenanceNotice({ minimum: '2.30', reason: 'missing' }).git).toBeNull()
  })

  it.each([
    ['missing', 'No git was found'],
    ['macos_stub', 'Only the macOS stub is installed, without the Command Line Tools'],
    ['not_executable', 'It could not be run'],
    ['too_old', 'It is older than 2.30'],
    ['something_new', 'something_new'],
  ])('says why for %s, and what it costs', (reason, why) => {
    expect(maintenanceNotice({ minimum: '2.30', reason }).why).toBe(
      `${why}; nothing Retention removes is reclaimed until git 2.30 or newer is installed.`,
    )
  })

  it('says what a failed Maintenance Pass costs, and what git said', () => {
    const notice = maintenanceNotice({
      minimum: '2.30',
      path: '/usr/bin/git',
      reason: 'failed',
      detail: 'fatal: bad',
      install,
    })

    expect(notice.why).toBe(
      'A Maintenance Pass with it failed; nothing Retention removes is reclaimed until the daemon restarts or git.path changes.',
    )
    expect(notice.gitSaid).toBe('git said: fatal: bad')
  })

  it('offers the way to install one where a newer git is what is missing', () => {
    const notice = maintenanceNotice({ minimum: '2.30', reason: 'missing', install })

    expect(notice.install).toEqual(install)
    expect(notice.gitSaid).toBeNull()
  })

  it.each<[string, GitStatus]>([
    ['after a failed Maintenance Pass', { minimum: '2.30', reason: 'failed', install }],
    ['without a reason', { minimum: '2.30', install }],
    ['when the daemon offers none', { minimum: '2.30', reason: 'missing' }],
  ])('offers no way to install one %s', (_, git) => {
    expect(maintenanceNotice(git).install).toBeNull()
  })

  it('says nothing of why without a reason', () => {
    expect(maintenanceNotice({ minimum: '2.30', detail: 'unused' })).toMatchObject({ why: null, gitSaid: null })
  })

  it('leaves out an install command or note the daemon did not give', () => {
    expect(
      maintenanceNotice({ minimum: '2.30', reason: 'missing', install: { link: 'https://example.com/git' } }).install,
    ).toEqual({
      command: null,
      note: null,
      link: 'https://example.com/git',
    })
  })
})
