# 08 — Workspace structure and module boundaries

Type: grilling
Status: open
Blocked by: 01, 02

## Question

What crates exist, and what is each forbidden from knowing?

Settle:

- the split: domain core, git-backed store implementation, watcher, snapshot policy (triggers, debounce, rate
  limit), daemon, CLI, embedded UI assets;
- `SnapshotStore` as the single owner of git plumbing — its interface, and the guarantee that no other crate names
  a git concept. This is what keeps the storage model swappable;
- where policy lives: the core decides *when* a snapshot is warranted, the daemon supplies events and clock;
- what the core must never depend on: HTTP, the filesystem watcher, any DI framework, any UI;
- how the pieces are wired at startup, given the core exposes factory functions only;
- testability: the fakes required (in-memory store, virtual clock, temp-dir fixtures) and which behaviours are
  tested against a real git store instead.

Consult the `codebase-design` skill for this one.
