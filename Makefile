.PHONY: issues issues-all

# Issue tracker (.scratch/), see docs/agents/issue-tracker.md
issues:
	@./scripts/issues.sh

issues-all:
	@./scripts/issues.sh --all
