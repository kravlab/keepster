# The core is a pure decision kernel and the store is concrete, so no port traits exist

Keepster's core holds the logic worth protecting — the Retention ladder, the layered include/exclude matcher, the
trigger machine — and the usual way to protect it is hexagonal: the core declares `trait SnapshotStore`, `trait
Clock`, `trait FileSystem`, and adapters obey. We do the opposite. `keepster-core` is pure functions over domain
types with no traits and no I/O; `keepster-app` holds use-cases that call a **concrete** `SnapshotStore`; time is a
parameter. The workspace contains no port trait for the store, the clock or the filesystem, and no fake or mock of
any of them; the two stand-ins it has are named below, with why. A reader expecting hexagonal architecture would
otherwise read this as an oversight, which is why it is written down.

## Considered Options

- **Ports in the core.** `trait SnapshotStore` / `Clock` / `FileSystem` declared by the core, one git2 adapter,
  fakes in tests. Lets app-level logic be tested with no git on the machine.
- **A pure kernel with nothing above it.** Core returns decisions as data and the daemon interprets them directly,
  with no use-case layer. Smallest surface, but HTTP handlers and CLI-local commands each re-derive the
  orchestration, and the ordering invariants below end up duplicated across them.
- **A trait kept solely for a future `gitoxide` swap.** Ticket 01 rejected `gitoxide` "for now", so a second
  adapter is imaginable.

## Consequences

- **A fake `SnapshotStore` would prove nothing about the risks that actually exist here.** They are all claims
  about real git and a real filesystem: that `git gc --prune=now` reclaims space once a Segment ref is gone
  (ticket 06), that a live reflog makes ref deletion free nothing and `core.filemode` must be `false` at store
  creation (ADR 0001), that the index rebuilds from git objects into the same rows (ticket 03), that a pin
  survives `gc` (ADR 0002). An in-memory double satisfying the trait is green while saying nothing about any of
  them. The trait would buy speed and cost truth.
- **Applying `codebase-design`'s rule: one adapter is a hypothetical seam, two make a real one.** There will be
  exactly one store adapter. A trait shaped around git2's index/tree/ref model would not survive a `gitoxide`
  swap in any case, so it would not be the seam it appears to be.
- **The invariants stay unbreakable from outside.** Because `SnapshotStore` is concrete and owns both git and the
  derived SQLite index, ticket 03's ordering rule — the commit and ref update always precede the index write —
  cannot be violated by a caller. Under a trait, that ordering would have to be re-established by every
  implementation and honoured by every use-case.
- **Testing splits cleanly by nature rather than by layer.** Pure things are tested as values with no disk:
  `retention::plan`, `rules::verdict`, and `decide(state, event, now) -> (Vec<Action>, PolicyState)` folded over a
  sequence of `(event, time)` pairs — which matters because ticket 02 established that filesystem events are lost,
  duplicated and reordered. Everything else is tested against a real git store in a temp dir.
- **The cost is real and one-directional**: `keepster-app` tests need `git` present and write to temp dirs, so
  they are slower than core tests and will not run in an environment without git. Retrofitting a trait later means
  touching every use-case and every test that constructs a store, which is why this is an ADR rather than a note.
- **Nothing needs dependency injection.** With no port trait for the store, the clock or the filesystem there is
  nothing to resolve, so wiring is a sequence of constructor calls in the daemon and a DI container would have no
  work to do.
- **"Swappable" is redefined.** The storage model is *locally replaceable* — no crate outside `keepster-store`
  names a git concept, so a rewrite is confined to one crate — rather than *trait-substitutable*. The weaker claim
  is the true one and must not be restated as the stronger one. One crate outside it names git on purpose:
  `keepster-testkit`, whose performance tripwire (ticket 69) times reading a Snapshot's tree into a git index,
  which no store operation exposes on its own, and names each fixture profile by a git blob id. It is a
  dev-only crate, so the product stays confined; a store rewrite rewrites those lines too.
- **Two stand-ins exist, and neither is a store. The first is which volume a path is on** (ticket 56,
  ADR 0011). The daemon asks it through `trait Volumes`, and the harness answers with `FakeVolumes`. The reasoning
  above runs the other way here: what must be proved is the daemon's behaviour when a disk is unplugged or
  remounted elsewhere, which no test can do to the machine it runs on, while the platform's own answers are tested
  for real, in `keepster-service`, on every platform CI runs. The trait is the daemon's, never the core's, and has
  one production implementation.
- **The second is which files are cloud placeholders** (ticket 75). The store asks it through `trait CloudFiles`,
  the watcher through the function type `IsPlaceholder`, and their tests answer with stand-ins (`FakeCloud`). The
  reason is the same: what must be proved is that a file whose data is only in the cloud is never read, and no CI
  machine has a sync client to free a file's space. Both production answers are one pure function of a `stat`
  already taken, `keepster_core::is_cloud_placeholder`, whose bits are tested as values. The trait and the type
  belong to the store and the watcher, never to the core.
