.PHONY: issues issues-all openapi check-api

# Issue tracker (.scratch/), see docs/agents/issue-tracker.md
issues:
	@./scripts/issues.sh

issues-all:
	@./scripts/issues.sh --all

# The `/v1` contract (ticket 40). The committed openapi.json is generated, never edited: this rewrites it
# from the code, and the drift test in `cargo test` fails on any byte that differs.
openapi:
	@KEEPSTER_WRITE_OPENAPI=1 cargo test --quiet -p keepster-daemon --test openapi the_committed_document_is_the_generated_one

# Additive-only, measured against what shipped: the last release tag, not the branch base.
check-api:
	@./scripts/check-api.sh
