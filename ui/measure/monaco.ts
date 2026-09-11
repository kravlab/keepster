/* PROTOTYPE (ticket 09) — measurement entry: what a Monaco-only bundle weighs. */
import { mount } from '../src/editors/monaco'
import { LANGUAGES } from '../src/editors/languages'

;(globalThis as Record<string, unknown>).__keepster_monaco = { mount, LANGUAGES }
