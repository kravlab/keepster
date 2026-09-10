# Issue tracker: Local Markdown

Issues and specs for this repo live as markdown files in `.scratch/`.

## Conventions

- One feature per directory: `.scratch/<feature-slug>/`
- The spec is `.scratch/<feature-slug>/spec.md`
- Implementation issues are one file per ticket at `.scratch/<feature-slug>/issues/<NN>-<slug>.md`, numbered from `01`, never a single combined tickets file
- Triage state is recorded as a `Status:` line within the first 15 lines of each issue file (see `triage-labels.md` for
  the label strings)
- Comments and conversation history append to the bottom of the file under a `## Comments` heading

## Ticket status lifecycle

The `Status:` line is the only record of ticket state, so it must be kept true:

- when creating a ticket, set `Status: needs-triage`, or `Status: ready-for-agent` once it is fully specified;
- when finishing the work a ticket describes, set `Status: done` in the same commit as the code change, so that a
  merged branch can never leave a ticket looking open;
- when abandoning a ticket, set `Status: wontfix` and say why under `## Comments`.

Wayfinder decision tickets use their own terminal label, `resolved` (see "Wayfinding operations" below). The three
terminal labels are therefore `done`, `wontfix` and `resolved`; every other value, and a missing line, counts as
open work.

Never close a ticket by deleting it, and never report work as complete while its ticket still shows a non-terminal
status.

Run `make issues` to list the tickets that are still open, or `make issues-all` to include the closed ones.

## When a skill says "publish to the issue tracker"

Create a new file under `.scratch/<feature-slug>/` (creating the directory if needed).

## When a skill says "fetch the relevant ticket"

Read the file at the referenced path. The user will normally pass the path or the issue number directly.

## Wayfinding operations

Used by `/wayfinder`. The **map** is a file with one **child** file per ticket.

- **Map**: `.scratch/<effort>/map.md` (the Notes / Decisions-so-far / Fog body).
- **Child ticket**: `.scratch/<effort>/issues/NN-<slug>.md`, numbered from `01`, with the question in the body. A `Type:` line records the ticket type (`research`/`prototype`/`grilling`/`task`); a `Status:` line records `claimed`/`resolved`.
- **Blocking**: a `Blocked by: NN, NN` line near the top. A ticket is unblocked when every file it lists is `resolved`.
- **Frontier**: scan `.scratch/<effort>/issues/` for files that are open, unblocked, and unclaimed; first by number wins.
- **Claim**: set `Status: claimed` and save before any work.
- **Resolve**: append the answer under an `## Answer` heading, set `Status: resolved`, then append a context pointer (gist + link) to the map's Decisions-so-far in `map.md`.
