.PHONY: issues issues-all openapi check-api ui check-ui check-ui-size bench bench-calibrate bench-ceiling platform-table conformance coverage

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

# The report layer (ticket 70): each report gates nothing and is written to target/reports, which CI's
# reports workflow also uploads per platform. KEEPSTER_REPORT_DIR must be absolute: tests run in their own
# crate's directory.
REPORTS := $(CURDIR)/target/reports

# This platform's column of the degenerate catalogue's table, written by the catalogue's own test, then
# the table of every column in target/reports; a platform with no column there reads `untested`.
platform-table:
	@KEEPSTER_REPORT_DIR=$(REPORTS) cargo test --locked -q -p keepster-testkit --test degenerate every_case_makes_exactly_the_entries_it_declares
	@cargo run --locked -q -p keepster-testkit --bin keepster-report -- table $(REPORTS)

# What this platform's watcher delivers for each documented class of change, into target/reports. It gates
# nothing.
conformance:
	@cargo run --locked -q -p keepster-testkit --bin keepster-report -- conformance $(REPORTS)

# Which crate no test reaches, into target/reports; no threshold. Needs the llvm-tools-preview component
# and cargo-llvm-cov 0.9.1 (`cargo install cargo-llvm-cov --version 0.9.1 --locked`). The report is made
# from whatever the tests reached even when some failed, and the target then fails as they did.
coverage:
	@mkdir -p $(REPORTS)
	@cargo llvm-cov clean --workspace
	@status=0; \
	cargo llvm-cov --workspace --all-features --locked --no-fail-fast --no-report || status=$$?; \
	cargo llvm-cov report --json --summary-only --output-path $(REPORTS)/coverage.json && \
	cargo run --locked -q -p keepster-testkit --bin keepster-report -- coverage $(REPORTS)/coverage.json $(REPORTS) && \
	exit $$status
