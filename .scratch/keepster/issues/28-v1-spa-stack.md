# 28 — The v1 SPA stack

Type: grilling
Status: needs-triage

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
