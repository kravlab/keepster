# 70: The report layer — the platform matrix, watcher conformance and coverage

**Blocked by:** 44 (Reconciliation — the guarantee, and what counts as evidence of change), 48 (Withheld, the metadata
sidecar, special files and paths as bytes), 49 (Symlinks — stored as links, optionally followed)

**Status:** ready-for-agent

**What to build:** the project can state, per platform, what the tests actually ran, what is impossible there and what
was never run — so a skip is never mistaken for coverage — and can see what each operating system's watcher really
delivers.

These are **reports**: they gate nothing, and they cannot lie because every number carries its environment. The
distinction they exist to preserve is that "impossible here" is not "never run here", which a single skip marker
erases.

- [ ] Every (case, platform) pair carries one of three declared outcomes: ran; impossible, compiled out with a
      recorded reason so the decision is made at compile time; unavailable, where a capability probe failed, with its
      reason
- [ ] The table is **generated** from the degenerate catalogue's metadata rather than maintained by hand, and
      published per platform with the two unverified platforms marked as untested until they run
- [ ] An ordinary run stays green when a capability probe fails, since a developer is not obliged to enable a
      platform feature; the release checklist requires the table with no gaps among the cases possible on the
      reference platform
- [ ] A printed runtime skip is forbidden anywhere in the suite
- [ ] A watcher conformance report records, per platform, what the operating system actually delivered for the
      documented event classes, with its environment, gating nothing
- [ ] A coverage report answers one question — which crate has no test at all — and no threshold is enforced
- [ ] The three-platform matrix produces all of these as run artifacts, which is what removes the untested marks
