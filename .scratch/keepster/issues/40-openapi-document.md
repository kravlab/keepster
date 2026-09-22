# 40: Publishing the `/v1` contract — a generated, committed OpenAPI document

**Blocked by:** 39 (Read a version back — the derived index, entries, content and the timeline)

**Status:** ready-for-agent

**What to build:** a third-party client author can fetch a machine-readable description of the API from a running
daemon, or read it from the repository, and be sure it matches the code. Landing this early means every endpoint
added afterwards is documented by construction rather than retrofitted.

The document is generated from the API crate's own types behind a default-off feature, the route and the documented
path are one literal so a renamed handler fails to compile and an unrouted handler is dead code, and the committed
document is compared byte for byte by a test. The error-code set is **open**: a code is a string with a generated
description, a client must treat an unknown code as its HTTP status, and the CLI maps codes to exit codes through an
exhaustive match with no wildcard (ADR 0005).

- [ ] The daemon serves the document under the version prefix it describes, generated at runtime and before
      authentication, and its content depends on the build and never on the instance
- [ ] The document is committed at the repository root and a test fails on any byte of drift
- [ ] Every response and error shape the API already has is described, including the Problem Details shape with its
      `code` extension
- [ ] A handler that is not routed, or a route whose documented path disagrees with the handler, fails the build
- [ ] The compatibility check compares against the last released tag rather than the branch base, and is reported as
      vacuous — not passing — until a release tag exists
- [ ] No HTML viewer is embedded
- [ ] The merge gate runs the drift test and the compatibility check
