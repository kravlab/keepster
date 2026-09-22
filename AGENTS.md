# AGENTS.md

## Agent skills

When working with Rust code, always use the `rust-expert` skill in addition to any workflow-specific skill.

These domain skills supplement workflow skills; they do not replace them.

### Issue tracker

Issues are tracked as Markdown files under `.scratch/`. See `docs/agents/issue-tracker.md`.

Run `make issues` to list the open tickets (`make issues-all` includes the closed ones). Every ticket carries a
`Status:` line that MUST be kept true — in particular, set `Status: done` in the same commit as the code that closes
the ticket. See the status lifecycle in `docs/agents/issue-tracker.md`.


### Triage labels

The five canonical triage roles are used verbatim as label strings. See `docs/agents/triage-labels.md`.

### Domain docs

Single-context: `CONTEXT.md` and `docs/adr/` at the repo root. See `docs/agents/domain.md`.

### Workflow handoffs

`/grill-with-docs` must stop after the interview and documentation updates.
Do not call `/to-spec`, `/to-tickets`, `/implement`, or modify source code automatically.

After completing `/grill-with-docs`, `/to-spec`, `/to-tickets`, or `/implement`,
end with `Next command: <command>` and one sentence explaining why that command
fits the current artifacts, unresolved decisions, scope, and completion state.
Recommend exactly one applicable command, or `Next command: none` when the
workflow is complete. Treat the usual
`/grill-with-docs` → `/to-spec` → `/to-tickets` → `/implement` sequence as
conditional: skip any unnecessary phase, and recommend a different relevant
skill when it must resolve a prerequisite first. Recommend only; never invoke
the next command automatically.

## Web / Svelte

When working with Svelte code, always use the `svelte-code-writer` skill
in addition to any workflow-specific skill.

When designing, redesigning, or visually polishing the web UI, also use
the `frontend-design` skill.

Use `frontend-design` for visual hierarchy, typography, spacing, color,
responsive behavior, interaction design, accessibility, and overall
visual quality.

These domain skills supplement workflow skills such as OpenSpec propose,
apply, verify, and code-review; they do not replace them.
