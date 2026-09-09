#!/bin/bash

# List the tickets of the local Markdown issue tracker under `.scratch/`.
#
# Ticket state lives in a `Status:` line near the top of every issue file (see
# docs/agents/issue-tracker.md). `done` and `wontfix` are the terminal labels;
# anything else -- including a missing line -- counts as open work.
#
# Usage:
#   scripts/issues.sh          # open tickets only
#   scripts/issues.sh --all    # every ticket, including the closed ones

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SCRATCH_DIR="$REPO_ROOT/.scratch"

show_all=false
case "${1:-}" in
    --all) show_all=true ;;
    "") ;;
    *)
        echo "Unknown argument: $1" >&2
        echo "Usage: $(basename "$0") [--all]" >&2
        exit 2
        ;;
esac

if [ ! -d "$SCRATCH_DIR" ]; then
    echo "No issue tracker: $SCRATCH_DIR does not exist."
    exit 0
fi

# The header is scanned rather than the whole file so that a `Status:` word
# quoted inside the ticket body cannot be mistaken for the ticket's own state.
# Both `Status: x` and `**Status:** x` are accepted because both spellings are
# already in use.
read_status() {
    sed -n '1,15p' "$1" \
        | sed -n 's/^[[:space:]]*[*_]*Status:[*_]*[[:space:]]*//p' \
        | head -1 \
        | tr -d '\r' \
        | tr '[:upper:]' '[:lower:]' \
        | sed 's/[[:space:]]*$//'
}

total=0
shown=0
current_feature=""

# Sorted so that the output order is stable and groups by feature directory.
while IFS= read -r file; do
    total=$((total + 1))
    status="$(read_status "$file")"

    if [ "$show_all" = false ]; then
        case "$status" in
            done | wontfix) continue ;;
        esac
    fi

    feature="$(basename "$(dirname "$(dirname "$file")")")"
    if [ "$feature" != "$current_feature" ]; then
        [ -n "$current_feature" ] && echo ""
        echo "$feature"
        current_feature="$feature"
    fi

    printf '  %-18s %s\n' "${status:-<no status>}" "$(basename "$file")"
    shown=$((shown + 1))
done < <(find "$SCRATCH_DIR" -mindepth 3 -maxdepth 3 -path '*/issues/*.md' -type f | sort)

echo ""
if [ "$show_all" = true ]; then
    echo "$shown ticket(s)."
else
    echo "$shown open of $total ticket(s). Run with --all to see the closed ones."
fi
