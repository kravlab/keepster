import { describe, expect, it } from 'vitest'
import { acknowledge, chooseErasing, noConsent, purges, ready, wording } from './unwatch'

describe('the consent unwatching a Root needs', () => {
  it('lets a plain unwatch go with nothing asked, and sends no purge', () => {
    const consent = noConsent()

    expect(ready(consent)).toBe(true)
    expect(purges(consent)).toBe(false)
    expect(wording(consent)).toBe('go')
  })

  it('holds an erasing back until its irreversibility is acknowledged', () => {
    const consent = chooseErasing(noConsent(), true)

    expect(ready(consent)).toBe(false)
    expect(purges(consent)).toBe(false)
  })

  it('lets an erasing go, and purge, once both the choice and the acknowledgement are given', () => {
    const consent = acknowledge(chooseErasing(noConsent(), true), true)

    expect(ready(consent)).toBe(true)
    expect(purges(consent)).toBe(true)
  })

  it('words the button by the choice, before it is acknowledged too', () => {
    const chosen = chooseErasing(noConsent(), true)

    expect(wording(chosen)).toBe('goErasing')
    expect(wording(acknowledge(chosen, true))).toBe('goErasing')
    expect(wording(chooseErasing(chosen, false))).toBe('go')
  })

  it('takes the acknowledgement back when the choice is withdrawn, so choosing again asks it afresh', () => {
    const acknowledged = acknowledge(chooseErasing(noConsent(), true), true)
    const withdrawn = chooseErasing(acknowledged, false)
    const again = chooseErasing(withdrawn, true)

    expect(withdrawn).toEqual(noConsent())
    expect(ready(again)).toBe(false)
    expect(purges(again)).toBe(false)
  })

  it('takes an acknowledgement for nothing as none: only an erasing has one to give', () => {
    const consent = acknowledge(noConsent(), true)

    expect(consent).toEqual(noConsent())
    expect(chooseErasing(consent, true)).toEqual({ erase: true, acknowledged: false })
  })

  it('withdraws the acknowledgement alone and keeps the erasing chosen', () => {
    const consent = acknowledge(acknowledge(chooseErasing(noConsent(), true), true), false)

    expect(consent).toEqual({ erase: true, acknowledged: false })
    expect(ready(consent)).toBe(false)
  })
})
