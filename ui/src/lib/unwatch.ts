/**
 * The consent unwatching a Root needs (#31), as a plain module the unwatch dialog draws, as
 * `restore.ts` is for Restore. Unwatching keeps the history and pins its last state; erasing it as well is
 * a choice of its own, and an irreversible one, so it is not taken on the choice alone: a second,
 * separate acknowledgement is required, as `keepster unwatch --purge` requires `--yes`.
 */

/** What the person has chosen and acknowledged in the dialog so far. */
export interface Consent {
  /** Whether the history is to be erased as well. */
  erase: boolean
  /** Whether the person has acknowledged that erasing is irreversible; only an erasing has one to give. */
  acknowledged: boolean
}

/** Nothing chosen: the plain unwatch, which needs no acknowledgement. */
export function noConsent(): Consent {
  return { erase: false, acknowledged: false }
}

/** Chooses erasing the history as well, or withdraws the choice and the acknowledgement with it. */
export function chooseErasing(consent: Consent, erase: boolean): Consent {
  return { erase, acknowledged: erase && consent.acknowledged }
}

/** Gives or withdraws the acknowledgement; without erasing chosen there is nothing to acknowledge. */
export function acknowledge(consent: Consent, acknowledged: boolean): Consent {
  return { ...consent, acknowledged: consent.erase && acknowledged }
}

/** Whether the request may go: every acknowledgement the choice needs is given. */
export function ready(consent: Consent): boolean {
  return !consent.erase || consent.acknowledged
}

/** Whether the request carries `purge=true`: only when erasing was chosen and acknowledged. */
export function purges(consent: Consent): boolean {
  return consent.erase && consent.acknowledged
}

/** Which of the button's two wordings, a key of `strings.unwatch`, follows the choice. */
export function wording(consent: Consent): 'go' | 'goErasing' {
  return consent.erase ? 'goErasing' : 'go'
}
