# 16 — Web UI themes and appearance settings

Type: grilling
Status: open
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
