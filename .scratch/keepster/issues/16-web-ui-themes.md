# 16 — Web UI themes and appearance settings

Type: grilling
Status: resolved
Blocked by: 09

## Question

Which themes does the v1 web UI ship, and how does a user configure its appearance?

- which themes ship, and whether the UI follows `prefers-color-scheme` by default;
- whether a theme also restyles the diff: ticket 09 found the `@codemirror/merge` defaults barely visible on a dark
  theme (an 8 % changed-line background, a 2px underline for changed text), so diff contrast belongs to the theme;
- whether users can add their own themes or only pick among the bundled ones;
- where the appearance preference lives: a daemon configuration key (ticket 04), which the API exposes read-only
  (ticket 07), or browser-local state, weighed against ticket 08's rule that the UI owns no persistence contract;
- whether per-view toggles such as line wrapping (ticket 09) are remembered, and where.
- where the syntax-highlighting line-length threshold lives: ticket 09 made it configurable with a default of
  20,000 characters, and a file with any longer line opens without a language, because CodeMirror parses the whole
  document and cannot skip a single line.

## Answer

**Two bundled themes; the preference belongs to the daemon, the first paint belongs to the browser.**

### Themes

Light and dark ship with v1. The default is `system`: the UI follows `prefers-color-scheme` live, so a fresh
browser looks native without anything being configured. A three-way control (System / Light / Dark) lets the user
pin one, because the OS scheme is not always the reading condition — a diff read at night on a light-mode desktop
has no other way out.

No user-supplied themes in v1. Accepting them would turn the CSS custom properties into a published, versioned
contract — renaming a variable would break someone's theme — and nothing yet suggests anyone wants it.

### The theme owns the diff, and the editor

A theme defines the added, removed and changed colours together with the syntax palette, and the editor theme is
ours, written against the same CSS variables as the surrounding chrome. `@codemirror/merge`'s defaults (an 8 %
changed-line background, a 2px underline for changed text, `dist/index.js:1113-1130`) are never used, and neither is
`oneDark`, which the prototype hardcoded (`ui/src/editors/cm6.ts:15,26`). Otherwise the editor and the rest of the
screen are two different design systems, and the contrast of the one element the tool exists to show is whatever a
dependency's default happened to be.

### Where the preference lives

**The daemon holds it, in its own UI-preferences store**: a file in the daemon's state directory, replaced
atomically. Not `.keepster.toml`, which is an ordinary file in the watched tree — it enters Snapshots and travels to
every clone (ticket 04), the same leak that ruled out a config-stored token (ticket 07). Not the global config
either: `GET /v1/roots/{r}/config` is read-only in v1 and writes were deferred over TOML round-tripping, so a
config-only home would leave the in-UI control unable to make anything stick.

Access is a public pair, `GET /v1/preferences` and `PUT /v1/preferences`, additive to `/v1` under ticket 14. It is
not a privileged endpoint for the embedded SPA: any client may use the same contract, as ticket 07 requires.

Browser-local storage alone was rejected as the home: it is cleared with site data, absent in a private window, and
keyed to the origin `127.0.0.1:<port>`, so a changed port loses the setting.

### Keys

| key | default | meaning |
| --- | --- | --- |
| `theme` | `system` | `system` \| `light` \| `dark` |
| `syntax_line_limit` | `20000` | a file with any longer line opens without a language (ticket 09) |
| `wrap_lines` | `false` | the default state of the line-wrapping toggle |

All three are daemon-wide, editable from the UI, and there is no settings screen: the wrapping toggle stays in the
diff pane's own bar where ticket 15 put it, and flipping it sets the default for the next file as well. The
threshold is daemon-wide rather than per-Root: a Root full of minified sources does not get its own value in v1.

### First paint

The SPA keeps a copy of the resolved values in `localStorage` **as a paint cache only** — the daemon is always the
source of truth. Without it the first frame would be drawn from `prefers-color-scheme` and then corrected by the
`GET`, which is a visible flash for anyone who pinned the opposite theme. Losing the cache therefore costs exactly
one flash, never a lost setting.

`index.html` is not templated with the value: that would make `keepster-ui` read preferences, and ticket 08 keeps it
a dependency-free static-asset server. A failed `PUT` is reported in the UI; it never silently shows a theme the
daemon did not accept.

### Consequences

- `/v1` gains `GET`/`PUT /v1/preferences`, so `openapi.json` and ticket 14's byte-for-byte test move with it.
- The daemon gains one more piece of state to write atomically, alongside the Registry.
- Ticket 09's "the threshold is configurable" is now concrete, and ticket 15's per-file wrapping toggle gains a
  remembered default without gaining a settings screen.
