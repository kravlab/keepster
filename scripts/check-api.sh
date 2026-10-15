#!/bin/bash

# The `/v1` compatibility check (ticket 40): the committed openapi.json must be an additive change of the
# one the last release shipped.
#
# The baseline is the last release tag reachable from HEAD, never the branch base: the promise was made
# to clients, and a client has only ever seen what was released, so changing something added last week
# and never shipped is legitimate. Until a release tag exists nothing has been promised, and the check
# says so: it reports itself as VACUOUS rather than as a pass, and exits 0 only so that it does not block
# a merge over a promise that has not been made yet.
#
# `oasdiff` is a contributor prerequisite; its absence breaks this check, never the product.
#
# Usage:
#   scripts/check-api.sh

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DOCUMENT="openapi.json"
# The false positives oasdiff reports for an additive change, each one said why there.
IGNORED="scripts/check-api-ignore.txt"
OASDIFF_VERSION="v1.31.0"

# Release tags are `v` followed by a version, as the release pipeline writes them.
RELEASE_TAGS='v[0-9]*'

vacuous() {
    local reason="$1"
    local message="the API compatibility check is VACUOUS, not passing: $reason"

    echo "check-api: $message" >&2
    if [ "${GITHUB_ACTIONS:-}" = "true" ]; then
        echo "::warning title=API compatibility is vacuous::$message"
        if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
            printf '### API compatibility: vacuous\n\n%s\n' "$message" >>"$GITHUB_STEP_SUMMARY"
        fi
    fi
    exit 0
}

cd "$REPO_ROOT"

if ! tag="$(git describe --tags --abbrev=0 --match "$RELEASE_TAGS" HEAD 2>/dev/null)"; then
    vacuous "no release tag ($RELEASE_TAGS) is reachable from HEAD, so no contract has shipped yet"
fi

if ! command -v oasdiff >/dev/null 2>&1; then
    echo "check-api: oasdiff is required; install it with" >&2
    echo "  go install github.com/oasdiff/oasdiff@$OASDIFF_VERSION" >&2
    exit 2
fi

workdir="$(mktemp -d)"
trap 'rm -rf "$workdir"' EXIT

if ! git show "$tag:$DOCUMENT" >"$workdir/released.json" 2>/dev/null; then
    vacuous "release $tag carries no $DOCUMENT, so it published no contract to compare against"
fi

echo "check-api: comparing $DOCUMENT with release $tag" >&2
oasdiff breaking --fail-on WARN --err-ignore "$IGNORED" "$workdir/released.json" "$DOCUMENT"
echo "check-api: $DOCUMENT is compatible with release $tag" >&2
