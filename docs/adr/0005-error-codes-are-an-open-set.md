# Error codes are an open set, typed as a string rather than a schema enum

Ticket 07 made `code` the stable part of an RFC 9457 problem — the field clients branch on — and fixed an initial
list of sixteen values. Ticket 14 generates the `/v1` document from the Rust types, so the obvious rendering of the
`ErrorCode` enum is a closed JSON Schema `enum`, which every code generator and IDE would then turn into an
exhaustive match for the client. We deliberately emit `type: string` instead, with the known values listed in a
description generated from the same enum, and state in the contract that the set may grow inside `/v1` and that an
unknown code must be treated as its HTTP status.

A reader who finds `type: string` where a perfectly good enum exists will read it as a generation shortcut and
close it. Closing it is a breaking change for every client that has been tolerating unknown codes as instructed,
which is why this is written down.

## Considered Options

- **A closed schema `enum`, derived from `ErrorCode`.** The best artifact for consumers: generated clients get a
  real type, and an unhandled variant is a compile error on their side. Rejected because it promises a frozen set.
  The list is not frozen and never was: ticket 11 added `git_missing` and ticket 10 added the Withheld reasons
  after ticket 07 wrote the list down, and every future ticket that names a new refusal adds another. Mechanically
  it also collides with ticket 14's own compatibility gate — `oasdiff` classifies an added enum value in a
  *response* as potentially breaking, which is the `WARN` level the gate fails on — so each new code would either
  fail the release or force a permanent rule suppression that hollows the gate out.
- **`type: string` with no list at all.** Cheapest, and the gate never complains. Rejected because the document
  then cannot answer the one question `code` exists to answer, "what may I branch on", and the answer would move
  into prose that drifts.
- **A closed enum plus a `/v2` whenever a code is added.** Honest under the compatibility policy and absurd in
  practice: a new refusal reason is the most ordinary additive change this API has, and it would cost a major
  version each time.

## Consequences

- **Clients must degrade, and are told so in the contract.** An unknown `code` is handled as its HTTP status — the
  coarse layer ticket 07 kept precisely for this. A client that matches exhaustively on the documented list is
  written against a promise this API does not make.
- **The description is generated, not maintained.** `#[schema(schema_with = …)]` walks the `ErrorCode` variants
  into the field description, so the document's list and the Rust enum cannot disagree, even though the schema type
  is open. There is exactly one source for the set, as the ticket required.
- **Adding a code stays additive end to end.** It changes a description, not a constraint, so the `oasdiff`
  baseline comparison passes without suppressions, and the compatibility gate keeps meaning what it says.
- **Exhaustiveness moves to our side of the wire.** The guarantee a closed enum would have given a client is kept
  internally instead: the CLI maps `ErrorCode` to exit codes with an exhaustive `match` and no `_` arm, so a new
  code cannot silently acquire a default. This is the reason ticket 07's "`1` everything else" had to be rewritten.
- **A closed set is still reachable at `/v2`**, and would then be a deliberate act with a deprecation window
  rather than a generation default.
