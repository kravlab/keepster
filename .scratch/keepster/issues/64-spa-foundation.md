# 64: The web UI's foundation — embedded assets, the session, the rail and the diff

**Blocked by:** 46 (Diff — computed by the daemon), 60 (The loopback listener and the browser's way in)

**Status:** ready-for-agent

**What to build:** a user opens the web UI and lands on one History screen showing a Snapshot rail, picks a Snapshot,
picks a file, and reads its diff — served entirely by the daemon's own binary with nothing installed.

The stack is the prototype's: Svelte 5, Vite, TypeScript, pnpm, with the built output committed so a plain checkout
builds without node, and the built SPA embedded in the binary behind a default-on feature. The diff editor is
CodeMirror 6, fed the **daemon's** hunks through its override hook, so the merge view's own diff is never used.

- [ ] The daemon serves the embedded SPA with correct content types, caching and single-page fallback, and the page
      makes no external request
- [ ] The bootstrap flow lands the browser in an authenticated session and the address bar ends up clean
- [ ] The History screen shows the Snapshot rail with sticky day headings, and selecting a Snapshot lists its changed
      paths
- [ ] Selecting a file shows its diff, rendered from hunks the daemon computed, with line wrapping off by default and
      an opt-in control in the diff pane's own bar
- [ ] A file with a line over the configured threshold opens without syntax highlighting instead of freezing
- [ ] The non-visual logic lives in plain modules covered by unit tests: the event-stream reconnection, the hunk feed
      into the editor
- [ ] The bundle stays inside the eager and total size budgets, checked mechanically, with the language grammars
      remaining lazy chunks
- [ ] Quality gates run: strict type checking, formatting and linting for the SPA, in a job that is the only one
      installing node

## Comments
- **From ticket 60.** The loopback router (`loopback_router` in `keepster-daemon`) serves a one-line
  placeholder at `/`, where a bootstrap exchange redirects; the embedded SPA replaces it. Every asset is
  behind the same gate (`Host`, `Origin`, cookie), so the SPA needs no route of its own. The `embed-ui`
  feature must not change the listener's default (ticket 24).
