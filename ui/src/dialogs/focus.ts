/**
 * Keyboard focus in a modal dialog, which the dialogs share.
 */
import { tick } from 'svelte'

/**
 * Backs out with `cancel`, then hands the keyboard to the element `target` gives once the DOM has caught
 * up: the controls that had it are gone, and the element is read only then, since it may only just have
 * been drawn again.
 */
export async function cancelThenFocus(cancel: () => void, target: () => HTMLElement | undefined): Promise<void> {
  cancel()
  await tick()
  target()?.focus()
}
