.PHONY: issues issues-all openapi check-api ui check-ui check-ui-size bench bench-calibrate bench-ceiling

# Issue tracker (.scratch/), see docs/agents/issue-tracker.md
issues:
	@./scripts/issues.sh

issues-all:
	@./scripts/issues.sh --all

# The `/v1` contract (ticket 40). The committed openapi.json is generated, never edited: this rewrites it
# from the code, and the drift test in `cargo test` fails on any byte that differs.
openapi:
	@KEEPSTER_WRITE_OPENAPI=1 cargo test --quiet -p keepster-daemon --test daemon openapi::the_committed_document_is_the_generated_one

# Additive-only, measured against what shipped: the last release tag, not the branch base.
check-api:
	@./scripts/check-api.sh

# The web UI (ticket 64). Node and pnpm are needed here and nowhere else: `cargo build` embeds the
# committed `ui/dist` and never runs node (ticket 08). Rebuild and commit `ui/dist` after changing `ui/`.
ui:
	@cd ui && pnpm install --frozen-lockfile && pnpm build

# Strict type checking, formatting, linting and the unit tests of the SPA's plain modules.
check-ui:
	@cd ui && pnpm install --frozen-lockfile && pnpm check && pnpm format && pnpm lint && pnpm test

# The bundle's weight against its budget (ticket 28), measured on a fresh build.
check-ui-size: ui
	@cd ui && pnpm measure

# The operation tripwire (ticket 30), on Linux and never on tmpfs: the fixture goes beneath
# KEEPSTER_BENCH_DIR, or target/tmp. Each run prints its report and writes it to target/bench/. A breach
# and a refusal both fail the target; the run's last line says which.
#
# The small fixture, held to the committed thresholds in crates/keepster-testkit/bench/thresholds.toml.
bench:
	@cargo bench --locked -p keepster-testkit --bench tripwire -- gate

# Rewrites the thresholds as twice each gated line's median, on the reference machine only; commit the
# result with why it moved.
bench-calibrate:
	@cargo bench --locked -p keepster-testkit --bench tripwire -- calibrate

# The ceiling fixture, ten gibibytes, compared with the outcome budget's p95; gates nothing.
bench-ceiling:
	@cargo bench --locked -p keepster-testkit --bench tripwire -- ceiling
