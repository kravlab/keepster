# 28 — The v1 SPA stack

Type: grilling
Status: resolved

## Question

The ticket 09 prototype runs on **Vite, Svelte 5 and TypeScript**, chosen to get a prototype moving and never
confirmed for v1. The spec must name the stack, because ticket 08 makes the build reproducible and ticket 27 has
to package whatever it produces.

What is already fixed and not up for reconsideration here: the SPA is **embedded in the binary** behind ticket 08's
default-on `embed-ui` feature; the diff editor is **CodeMirror 6** fed by daemon-computed hunks (ticket 09), which
is a library choice, not a framework one; the UI is a client of `/v1` like any other (ticket 07); themes and
preferences come from the daemon (ticket 16); and `cargo build` must never require node (ticket 08).

Settle:

- **Confirm or replace Svelte 5**, with the reason stated. The prototype is evidence, not a commitment, and the
  honest question is whether anything about v1 — bundle size before first paint, the SSE client, CodeMirror 6
  integration, long-lived timeline views — argues differently.
- **The build toolchain and its pinning**: Vite version, node version floor, lockfile policy, and whether the
  built `ui/dist` is committed so that a `cargo build` from a source tarball needs no node at all, or whether
  distributors are expected to run `make ui`.
- **TypeScript strictness and lint/format tooling**, so the repo has one answer rather than per-file drift.
- **Testing the SPA**, and how that relates to the still-fog testing strategy: does the UI get component tests, an
  end-to-end harness against a real daemon, or neither in v1.
- **Bundle budget.** Ticket 09 measured CodeMirror 6 at 96.7 KiB gzip against Monaco's 671 KiB before first paint
  and chose on that basis, which only means something if the whole bundle has a number to be judged against.
- **Accessibility and browser floor**: which browsers v1 supports, given the daemon serves the SPA to whatever the
  user has, and the `prefers-color-scheme` behaviour ticket 16 relies on.

## Answer

**Svelte 5 is confirmed, with Vite, TypeScript and pnpm as the prototype already uses them, and the prototype's
code becomes v1's rather than being rewritten.** The stack carries a measured bundle budget with a gate, a narrow
set of quality gates, and an externally-defined browser floor.

### Svelte 5, confirmed rather than assumed

The prototype on `prototype/web-ui` runs Svelte 5.57, Vite 8.2, TypeScript 7.0 and svelte-check 4.7 under a
committed pnpm lockfile, and uses runes outside components (`src/prototype-timeline/selection.svelte.ts`). It is
not a sketch: it is the working code behind ticket 15's three view modes, so replacing the framework means
rewriting the artifact a settled decision rests on.

Three things argue for keeping it beyond that sunk cost:

- **No SSR is needed at all.** Ticket 16 fixed that `index.html` stays static and the daemon serves prebuilt
  assets, which removes the usual reason to reach for a meta-framework.
- **Runtime weight is noise** beside CodeMirror's 96.7 KiB gzip.
- **Fine-grained reactivity fits the shape of the screen.** The timeline is long-lived and fed by ticket 07's SSE
  stream; runes update what changed without re-rendering the Snapshot list.

The "bigger ecosystem" argument for React is weaker here than usual: ticket 07 made `/v1` a public contract
precisely so that a third party writes *their own* UI rather than extending ours, so our framework choice is not a
platform decision for anyone else.

### The bundle budget, and what holds it

Ticket 09's measurement is committed at `ui/measure/REPORT.md` and splits exactly where it matters — **eager**, the
static import closure the browser must have before the editor paints, against **total**, everything lazily
reachable:

| | eager gzip | total gzip |
|---|---|---|
| CodeMirror 6, MergeView, 13 lazy languages | 96.7 KiB | 250.0 KiB |

**Budget: eager ≤ 150 KiB gzip, total ≤ 400 KiB gzip.** The eager headroom is about 50 KiB for the Svelte runtime
and the application shell — tight, deliberately.

The split is the real defence. The thirteen grammars are lazy chunks today, and a single stray static import moves
one into the eager closure: `lang-javascript` is 32.6 KiB gzip, `lang-rust` 25.2 KiB. That regression barely moves
the total while destroying time-to-first-paint, which is why one number would not have done.

**`make check-ui-size` fails over budget**, built on `ui/measure/run.mjs`, which already performs this measurement
— the tool exists because ticket 09 needed it. Running it manually until a CI pipeline exists is the same honest
position ticket 14 took with `make check-api`, and it is recorded here for the same reason: an unenforced budget
decays silently.

Boundary with the neighbouring ticket: daemon-side numbers — snapshot latency, idle cost, store growth — belong to
ticket 30. Bundle weight lives here and is not repeated there.

### Quality gates

The risk in this SPA is not in its markup. It is in **ticket 07's SSE handling** — reconnection, `Last-Event-ID`,
and honouring `resync` rather than pretending; in **feeding daemon-computed hunks** to CodeMirror through
`diffConfig.override` (ticket 09, the property that made CM6 the only candidate); and in **ticket 10's Restore
preflight**, with per-reason confirmations and per-path resolutions. All three are logic, and all three are
testable without a browser when they live in plain `.ts` modules.

- **Blocking**: `svelte-check` in strict mode, Prettier with the Svelte plugin, a minimal ESLint config.
- **Unit tests under Vitest on the non-visual logic**, which is kept in `.ts` modules beside the components —
  matching the project's own rule that logic belongs where it can be called as a function.
- **No component tests in v1.** They would mostly assert markup that is still going to move.
- **No end-to-end suite in v1.** It is the most valuable and the most expensive, and it belongs to the testing
  strategy still in fog — which now has a named first candidate: **SSE reconnection after a dropped connection**,
  because it is the failure the UI cannot detect for itself.

### The audience decides the browser floor

The SPA is served over loopback to the person who installed the daemon. It is a developer's own browser, not the
open web — so maintaining a version table would be work with no reader.

**The floor is Baseline "Widely available"**, an external, dated definition that Vite can target directly and that
nobody here has to keep current. Accessibility commits to **keyboard operability and a visible focus indicator**,
and claims no WCAG conformance level. Ticket 15 made the Snapshot rail the primary navigation, so arrow-key
movement through it is a requirement rather than a nicety. A conformance claim was rejected because asserting a
level nobody verifies is worse than stating plainly what was done.

### Toolchain

- **The pnpm lockfile is committed** and is what pins Vite, Svelte and TypeScript — not the ranges in
  `package.json`. Ticket 27 already requires a committed lockfile for AUR and nixpkgs, and this is the same file.
- **The minimum node version is declared once**, in `engines` plus `.nvmrc`. It matters because `make ui` needs
  node and `cargo build` must not (ticket 08), and a distributor has to be able to see which is which.
- **The build target is ES2022**, as in ticket 09's measurement. Changing it invalidates the budget above, so the
  two move together or not at all.
- **`ui/dist` is committed and embedded**, settled in ticket 27: committing makes a source build work without
  node, embedding makes the served UI incapable of disagreeing with the API that serves it. `rust-embed` runs with
  `deterministic-timestamps` so the embedded assets do not carry mtimes into the binary.
