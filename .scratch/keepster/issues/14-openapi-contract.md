# 14 — Publishing the /v1 contract: source of truth and drift control

Type: grilling
Status: resolved
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

## Answer

### The types are the source of truth, and `keepster-api` gains one optional dependency

The document is generated from `keepster-api`'s DTOs and from `#[utoipa::path]` annotations on the handlers
(`utoipa` 5.5.0, `utoipa-axum` 0.2.0). A hand-written document was rejected because nothing in Rust would check it
against the handlers — the ticket's own "a document nobody verifies is worse than none" — and spec-first codegen
was rejected outright: `openapi-generator`'s `rust-axum` is BETA, needs a JVM in CI, and owns the router and the
trait shape, which is an architecture commitment, not a documentation tactic.

This **amends ticket 08's "`api` depends on serde alone"**, and the amendment is forced rather than chosen. Schema
generation requires a trait (`utoipa::ToSchema`, or `schemars::JsonSchema` under `aide`) implemented **in the crate
that owns the types**; the orphan rule leaves no way to keep `keepster-api` serde-only and still generate schemas.
The alternative — mirroring every DTO in the daemon crate — reintroduces exactly the drift this ticket exists to
remove. So `keepster-api` carries `utoipa` as an **optional, default-off `openapi` feature**
(`#[cfg_attr(feature = "openapi", derive(ToSchema))]`), enabled by the daemon and by nothing else. The crate still
knows no domain, and that was the substance of 08's rule.

`aide` is the only other live axum option and hits the identical orphan rule through `schemars`; it was not
preferred because its stable line is 0.15 with a `schemars` 0.9 → 1.0 migration sitting in alpha since 2025-11,
and because it does not bind the route string to the documented path the way `routes!` does.

### Drift control: one literal, one test, one lint

- **The route and the documented path cannot diverge**, because they are the same string. `routes!(handler)`
  expands to `__path_<handler>::path()` and feeds that same value to `Router::route`. Renaming a handler removes
  `__path_<oldname>` and **fails to compile**.
- **The one hole is a handler never passed to `routes!`** — it disappears from the router and from the document
  simultaneously and silently. It is closed from the other side: such a handler is dead code, so `#![deny(dead_code)]`
  in the daemon crate turns the omission into a build failure.
- **`openapi.json` is committed at the repository root** (OpenAPI **3.1.0**, the only version released utoipa
  emits). `make openapi` regenerates it; a plain `#[test]` builds the document from the router and compares byte for
  byte, failing with "run `make openapi`". Committing it is what makes a contract change visible in review, and it
  is the baseline the compatibility gate needs. JSON because `to_pretty_json()` needs no extra feature and OAS
  recommends the name `openapi.json` for an entry document.
- **`info.version` is `"1"`** — the contract is versioned by URL prefix (ticket 07), so that is the only number
  here with a meaning. The build version is already served by `GET /v1/version`; putting it in `info.version` would
  churn the committed file on every bump and train people to regenerate without reading the diff.

### "Additive-only" is measured against what shipped

`oasdiff` **v1.31.0** (2026-09-05), a static ~6 MiB Go binary that runs fully offline and reads revisions straight
out of git:

```
oasdiff breaking --fail-on WARN <last release tag>:openapi.json HEAD:openapi.json
```

The baseline is the **last released tag**, not the PR base branch: the promise in ticket 07 was made to clients,
and a client has only ever seen what was released, so renaming a field added last week and never shipped is
legitimate. Comparing against `main` would forbid that, since oasdiff cannot tell released from unreleased. A
separate committed baseline file was rejected as a second copy of the document that will be forgotten and turn the
gate falsely green. The gate is vacuous until the first release tag exists, and that is honest: the promise starts
at the first release.

`--fail-on WARN` rather than `ERR`, because WARN is oasdiff's "potentially breaking" level and ticket 07 made this
contract public and third-party-facing; individual rules judged non-breaking are silenced explicitly rather than by
lowering the threshold wholesale.

There is **no pure-Rust equivalent** (`schemadoc-diff` has one version, last published 2023-07-13). `oasdiff` is
therefore a **contributor prerequisite**, unlike ticket 11's external `git`: its absence breaks a check, never the
product.

### Serving: `GET /v1/openapi.json`, generated at runtime, before authentication

- **Under the prefix it describes**, so `/v2/openapi.json` later costs nothing. There is no standard to follow:
  IANA's well-known registry has no `openapi` entry, the OAS standardises only the *file name*, and the
  standards-blessed discovery mechanism is a link relation — so API responses carry
  `Link: </v1/openapi.json>; rel="service-desc"` (RFC 8631).
- **Generated at runtime** from the same `OpenApi` value `split_for_parts()` returned with the router, not
  `include_str!` of the committed file: a binary built without running the drift test would otherwise serve a stale
  document with no way for a client to tell. The Q2 test already guarantees the two are equal.
- **Before authentication on the TCP listener**, alongside `GET /v1/version` (the `Host`/`Origin` checks still
  apply). This rests on an **invariant, not on a permission**: *the document depends on the build and never on the
  instance.* Two daemons of the same build serve byte-identical documents. Concretely this forbids ever populating a
  schema `enum` from user configuration — the set of Roots, the Segment Periods in play, `ui.allowed_origins` —
  because such a document would stop being a contract and start leaking data.
- **No embedded HTML viewer.** `utoipa-swagger-ui` downloads the Swagger UI archive with system `curl` in its build
  script unless the `vendored` feature is on (and that vendoring crate has not been released since 2024-10-16);
  `utoipa-redoc` avoids the build-time download but its standard page pulls the bundle from a CDN, which would make
  a page served on `127.0.0.1` depend on the internet. Either cost buys convenience that an external viewer pointed
  at a stable URL already provides. Adding a page later is additive.

### The error contract: one enum, an open set

**`Problem` is ours**, a seven-field struct in `keepster-api` (`type`, `title`, `status`, `detail`, `instance`,
`code`, `details`), with `IntoResponse` and the `application/problem+json` content type living in the daemon.
`problem_details` 0.10.0 is the only crate with both axum and utoipa integration and would have been the default
choice under "prefer established libraries", but there is no complexity here to buy — the structure is seven fields
and its whole substance (`code`, `details`) is ours — while the crate costs control over the `code` schema (below)
on a foreign generic `ProblemDetails<Ext>`, and its author documents a deviation from RFC 9457 **on
deserialisation**: values of the wrong type fail to parse instead of being ignored, and the CLI is the consumer.

**`code` is one `ErrorCode` enum in `keepster-api`** — no hand-written list anywhere — but it is rendered in the
document as a **`string` whose description is generated from the variants** (`#[schema(schema_with = …)]`), not as a
closed JSON Schema `enum`. A closed enum in a *response* is a promise that no other code will ever appear, which we
cannot make and have already broken twice (ticket 11 added `git_missing`, ticket 10 added Withheld reasons), and
oasdiff classifies an added response enum value as potentially breaking, so every new code would fight the gate
above. The contract therefore states explicitly: **the `code` set may grow inside `/v1`, and a client must treat an
unknown code as its HTTP status.** Recorded as
[ADR 0005](../../../docs/adr/0005-error-codes-are-an-open-set.md), because a reader who sees `type: string` will
otherwise read it as an omission and "fix" it.

**CLI exit codes are an exhaustive `match` on that enum, with no `_` arm.** This amends ticket 07's "`1` everything
else": `1` survives as a value assigned to the variants that deserve it, but not as a wildcard, because a wildcard
silently assigns `1` to every future code and destroys precisely the ability ticket 07 wanted — a script telling
"no such Snapshot" from "the daemon is dead". The mapping lives in the CLI, not on the enum: a process exit code is
CLI policy, and `keepster-api` is where the daemon and third-party clients look. The human-readable table is
printed from the same `match` (`keepster help exit-codes`), so the documentation cannot drift from the behaviour
either.

### SSE: typed today, bound to the stream later

OpenAPI gained real streaming support in **3.2.0** (2025-09-19) — `itemSchema` applies to each item of a
`text/event-stream`, with a dedicated section on SSE — but released utoipa emits **3.1.0 only**; `Version32` sits
unreleased on `master`, in a branch that also carries a breaking YAML-backend change. Pinning a git revision was
rejected: an unreleased dependency on the critical path of a public contract.

The distinction that matters is that 3.1 does not prevent **typing the events**, only **binding the types to the
stream**. Event payloads are ordinary serde DTOs, so from day one they are generated into `components/schemas` as a
tagged `oneOf` discriminated by the event name (`snapshot.created`, `root.state_changed`, …, `resync` included),
drift-tested and oasdiff-checked like everything else. `Last-Event-ID` is declared as a header parameter. What stays
prose in the endpoint description is the ring buffer's bound and the replay/`resync` semantics. When utoipa releases
`Version32`, adding `itemSchema: $ref: '#/components/schemas/Event'` is a **purely additive** change that the
compatibility gate accepts.

A separate AsyncAPI document (3.1.0, 2026-01-31) was rejected: it would be hand-maintained and drift, and Rust
tooling barely exists — the official generator has had no Rust template since the request was filed in 2019.

### Where the checks run, given there is no CI

This ticket adds **no CI files**. The repository has no `.github/` and not one `Cargo.toml`; a contract ticket
choosing the whole repository's pipeline would be overruled by the first build ticket. The gates are therefore
plain targets that work today and inside whatever pipeline appears later:

- `make openapi` — regenerate the committed document;
- the drift check is a `#[test]`, so `cargo test` already enforces it with no infrastructure at all;
- `make check-api` — the oasdiff gate; **until CI exists this is a manual release gate**, recorded on the map so
  that it does not quietly decay into "checked by memory";
- `make check-deps` — `cargo tree -p keepster-cli -e normal` must contain none of `utoipa`, `git2`, `rusqlite`,
  `axum`.

That last target is the price of the `openapi` feature, and it pays a debt of ticket 08's. Cargo unifies features
across one build-graph invocation, so `cargo build --workspace` compiles `keepster-api` with `openapi` on for every
member, the CLI included; per-package release builds (`cargo build -p keepster-cli --release`) are unaffected.
Ticket 08 stated "the CLI binary does not link `git2` or `rusqlite` at all" as a build fact, but nothing was ever
going to ask Cargo for the graph. One command makes both that claim and this feature's blast radius observable
instead of asserted.

### Consequences for other tickets

- **Ticket 08**: `keepster-api`'s dependency rule reads "serde, plus an optional default-off `openapi` feature";
  the daemon crate gains `#![deny(dead_code)]` as a contract mechanism, not a style choice.
- **Ticket 07**: "`1` everything else" is replaced by an exhaustive mapping; the error-code set is explicitly open,
  and clients are told so.
- Nothing here reaches `CONTEXT.md`: no new domain term appeared, and the glossary stays free of implementation.
