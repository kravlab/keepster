# 40: Publishing the `/v1` contract — a generated, committed OpenAPI document

**Blocked by:** 39 (Read a version back — the derived index, entries, content and the timeline)

**Status:** done

**What to build:** a third-party client author can fetch a machine-readable description of the API from a running
daemon, or read it from the repository, and be sure it matches the code. Landing this early means every endpoint
added afterwards is documented by construction rather than retrofitted.

The document is generated from the API crate's own types behind a default-off feature, the route and the documented
path are one literal so a renamed handler fails to compile and an unrouted handler is dead code, and the committed
document is compared byte for byte by a test. The error-code set is **open**: a code is a string with a generated
description, a client must treat an unknown code as its HTTP status, and the CLI maps codes to exit codes through an
exhaustive match with no wildcard (ADR 0005).

- [x] The daemon serves the document under the version prefix it describes, generated at runtime and before
      authentication, and its content depends on the build and never on the instance
- [x] The document is committed at the repository root and a test fails on any byte of drift
- [x] Every response and error shape the API already has is described, including the Problem Details shape with its
      `code` extension
- [x] A handler that is not routed, or a route whose documented path disagrees with the handler, fails the build
- [x] The compatibility check compares against the last released tag rather than the branch base, and is reported as
      vacuous — not passing — until a release tag exists
- [x] No HTML viewer is embedded
- [x] The merge gate runs the drift test and the compatibility check

## Comments

Implemented with `utoipa` 5.5.0 and `utoipa-axum` 0.2.0, the versions ticket 14 chose. 6.0.0 and 0.3.0 are out,
but both need Rust 1.88 against the workspace's 1.87, and the move buys nothing here: 6.0 does not bring the OAS 3.2
`itemSchema` that SSE waits for.

- `keepster-api` gains the default-off `openapi` feature and `ErrorCode`, declared once in a table that generates
  the enum, its wire spelling, its meaning and the document's list. `Problem.code` stays a `String`, so an unknown
  code still parses. The daemon builds every problem from an `ErrorCode`, so no code is a free-standing literal.
- The router is an `OpenApiRouter` with one `routes!` per path. Both drift holes were checked by hand. A misspelt
  handler fails with ``cannot find `__path_…` ``. A handler left out of the router fails under
  `#![deny(dead_code)]`, together with its parameter struct.
- Path parameters are extracted into the same `IntoParams` structs that document them (`RootAddress`,
  `SnapshotAddress`), so the documented and parsed names cannot differ. Each operation has an explicit
  `operation_id`: the derived ones collided (`list` twice) and would have renamed a generated client's methods
  whenever a handler was renamed.
- `make openapi` rewrites `openapi.json` through the drift test (`KEEPSTER_WRITE_OPENAPI=1`); `.gitattributes`
  pins its line endings so a Windows checkout does not fail the byte comparison.
- `make check-api` (`scripts/check-api.sh`) compares with the last `v*` tag reachable from HEAD using
  `oasdiff breaking --fail-on WARN`. With no tag it prints VACUOUS, plus a GitHub warning annotation and a step
  summary in CI, and exits 0. Checked against a simulated `v0.1.0` tag:
  - an unchanged document passes;
  - a removed response field fails;
  - an added field passes;
  - an added error code passes, as ADR 0005 expects.
- The merge gate gains a `contract` job, which runs the drift test by name and then `check-api` with full history
  and tags. The CLI dependency check now also forbids `utoipa` and `axum`.
- The review found that axum's own extractor refusals were plain text while the document promised problems.
  These are a malformed or wrongly typed JSON body (`400`, `415`, `422`) and a `purge` that is not a boolean.
  They now answer as problems with two new codes, `body_invalid` and `purge_invalid`. Both are additive under
  ADR 0005, and an HTTP test covers each.
- "Before authentication" holds today because nothing authenticates. Ticket 60, which adds the loopback
  listener's authentication, must exempt `GET /v1/openapi.json` alongside `GET /v1/version`.
- Not done, because outside this ticket's boxes:
  - the RFC 8631 `Link: rel="service-desc"` header from ticket 14;
  - `keepster help exit-codes`;
  - the `type`/`instance` members of `Problem`.

  The DTOs' rustdoc becomes the schema descriptions, so some still mention ticket numbers. They are harmless to a
  client, but worth a pass before the first release.
