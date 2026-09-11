/* PROTOTYPE (ticket 09) — measurement entry: what a CodeMirror-only bundle weighs.
   Built on its own so nothing from Monaco or Svelte can be attributed to it. */
import { mount } from '../src/editors/cm6'
import { LANGUAGES } from '../src/editors/languages'

// Keep both alive so the bundler cannot tree-shake the thing being measured.
;(globalThis as Record<string, unknown>).__keepster_cm6 = { mount, LANGUAGES }
