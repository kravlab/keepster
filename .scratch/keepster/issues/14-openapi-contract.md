# 14 — Publishing the /v1 contract: source of truth and drift control

Type: grilling
Status: open
Blocked by: 08

## Question

[07 — Daemon HTTP API and CLI surface](07-api-and-cli-surface.md) made `/v1` a public contract: additive-only,
breaking changes only through `/v2`, and explicitly serving third-party UIs that cannot shell out to the CLI. It
committed to documenting the API but did not decide how. [08 — Workspace structure and module
boundaries](08-workspace-structure.md) has now put the DTOs in their own crate, `keepster-api`, which knows
nothing of the domain — so the question has a concrete subject and can be settled.

Settle:

- **Source of truth**: is the OpenAPI document generated from `keepster-api`'s types (via `utoipa` or similar),
  hand-written and used to check the types, or generated from the axum handlers themselves? Each puts the
  authority in a different place, and only one of them can be authoritative.
- **Drift control**: what fails, and in which check, when a handler and the document disagree. A document nobody
  verifies is worse than none, because it is believed.
- **Where it lives and how it is served**: in the repository, embedded in the binary and served from the daemon,
  or both — and whether the daemon exposes it at a `/v1` path so that a third-party client can fetch it from a
  running instance.
- **What "additive-only" means mechanically**: whether a check compares the generated document against the last
  released one and fails on a removal or a type change, and where that baseline is kept.
- **The error contract**: ticket 07 fixed RFC 9457 problems with a stable `code` plus a CLI exit-code table.
  Decide whether the code list is generated from the same source as the schema, and whether the exit-code table
  is derived from it rather than maintained by hand.
- **The SSE stream**: OpenAPI describes it poorly. Decide whether the event shapes and the `Last-Event-ID` replay
  semantics are documented in the same artifact or alongside it.
