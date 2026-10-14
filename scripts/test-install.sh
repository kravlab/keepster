#!/bin/bash

# Tests of scripts/install.sh (ticket 85), run against a release faked in a temporary folder.
#
# The fake release has the real one's layout -- `keepster-<version>-<target>.tar.xz` holding a
# `keepster-<version>-<target>/` folder, beside `SHA256SUMS` -- but its `keepster` is a stub that
# prints a version and records the arguments `install` was given. So what is asserted is what the
# installer does with a release, never what a real service manager does; the release workflow runs
# the installer against the real archives. `uname` is stubbed too, so the platform is the test's.
#
# Usage:
#   scripts/test-install.sh                       # the installer under `sh`
#   INSTALLER_SHELL=bash scripts/test-install.sh  # under bash, as macOS's `sh` is

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
INSTALLER="$REPO_ROOT/scripts/install.sh"
VERSION=0.9.0

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

failures=0

# Report a failed expectation of the current test; the test goes on, and the run fails at the end.
fail() {
    echo "  FAIL: $*" >&2
    failures=$((failures + 1))
}

# A fresh case folder: `rel` the fake release served over file://, `stubs` put first on PATH, `tmp`
# the installer's TMPDIR, `dest` the install folder (not created), `log` where the stub records what
# `install` was given.
new_case() {
    CASE="$WORK/$1"
    mkdir -p "$CASE/rel" "$CASE/stubs" "$CASE/tmp"
    DEST="$CASE/dest"
    LOG="$CASE/install-args"
    stub_uname Linux x86_64
}

# Make `uname -s` answer `$1` and `uname -m` answer `$2`.
stub_uname() {
    cat >"$CASE/stubs/uname" <<EOF
#!/bin/sh
case "\$1" in
    -s) echo '$1' ;;
    -m) echo '$2' ;;
    *) exit 1 ;;
esac
EOF
    chmod +x "$CASE/stubs/uname"
}

# Print `$1`'s SHA256SUMS line: Linux has `sha256sum`, macOS `shasum`.
checksum() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1"
    else
        shasum -a 256 "$1"
    fi
}

# Put an archive for `$1` (a target) into the fake release, with its line in SHA256SUMS. Its stub
# `keepster`, given `install`, records its arguments in `$LOG` and the path it was run by in
# `$LOG.path`, reads whatever its standard input holds, as a command that swallows a piped script
# would, and exits with KEEPSTER_STUB_EXIT (0 by default).
release_for() {
    local name="keepster-$VERSION-$1"
    local dir="$CASE/build/$name"
    mkdir -p "$dir"
    cat >"$dir/keepster" <<EOF
#!/bin/sh
case "\$1" in
    --version) echo 'keepster $VERSION' ;;
    install)
        shift
        printf '%s\n' "\$@" >'$LOG'
        printf '%s\n' "\$0" >'$LOG.path'
        cat >/dev/null
        exit "\${KEEPSTER_STUB_EXIT:-0}"
        ;;
esac
EOF
    printf '#!/bin/sh\n' >"$dir/keepster-tray"
    printf 'MIT\n' >"$dir/LICENSE"
    chmod +x "$dir/keepster" "$dir/keepster-tray"
    tar -cJf "$CASE/rel/$name.tar.xz" -C "$CASE/build" "$name"
    (cd "$CASE/rel" && checksum "$name.tar.xz" >>SHA256SUMS)
}

# Run the installer as a person would, through `sh` from the case folder, with the case's release,
# stubs and folder: EXTRA_PATH, if set, goes on PATH after the stubs, INSTALL_DIR (by default `$DEST`)
# is the install folder it is given, and with PIPED set the script is read from standard input, as
# `curl ... | sh` reads it. INSTALLER_SHELL, set for the whole run, names another shell than `sh`:
# `bash`, which macOS runs as `sh`, reads a piped script a byte at a time where dash reads it whole.
# Its output lands in `$CASE/out` and `$CASE/err`, and its exit code in STATUS.
run_installer() {
    local input=/dev/null
    STATUS=0
    set -- "$INSTALLER" "$@"
    if [ -n "${PIPED:-}" ]; then
        input=$INSTALLER
        set -- -s -- "${@:2}"
    fi
    (cd "$CASE" && env PATH="$CASE/stubs:${EXTRA_PATH:+$EXTRA_PATH:}$PATH" \
        KEEPSTER_VERSION="${KEEPSTER_VERSION-$VERSION}" \
        KEEPSTER_BASE_URL="file://$CASE/rel" \
        KEEPSTER_INSTALL_DIR="${INSTALL_DIR:-$DEST}" \
        TMPDIR="$CASE/tmp" \
        "${INSTALLER_SHELL:-sh}" "$@" <"$input" >"$CASE/out" 2>"$CASE/err") || STATUS=$?
}

test_installs_both_binaries_side_by_side_and_registers_them() {
    new_case installs
    release_for x86_64-unknown-linux-musl

    run_installer

    [ "$STATUS" -eq 0 ] || fail "exit $STATUS: $(cat "$CASE/err")"
    [ -x "$DEST/keepster" ] || fail "no keepster in $DEST"
    [ -x "$DEST/keepster-tray" ] || fail "no keepster-tray in $DEST"
    [ "$("$DEST/keepster" --version)" = "keepster $VERSION" ] || fail "not the released keepster"
    [ -f "$LOG" ] || fail "keepster install was not run"
    [ "$(cat "$LOG.path" 2>/dev/null)" = "$DEST/keepster" ] ||
        fail "keepster install ran as $(cat "$LOG.path" 2>/dev/null), not from $DEST"
}

test_read_from_standard_input_it_runs_to_the_end_whatever_a_command_reads() {
    new_case piped
    release_for x86_64-unknown-linux-musl

    PIPED=1 KEEPSTER_STUB_EXIT=7 run_installer --no-tray

    [ "$STATUS" -eq 7 ] || fail "exit $STATUS, not keepster install's 7: $(cat "$CASE/err")"
    [ "$(cat "$LOG" 2>/dev/null)" = "--no-tray" ] || fail "keepster install got: $(cat "$LOG" 2>/dev/null)"
    grep -q "PATH" "$CASE/out" || fail "the end of the script never ran: $(cat "$CASE/out")"
}

test_a_relative_install_folder_is_registered_by_its_full_path() {
    new_case relative
    release_for x86_64-unknown-linux-musl

    INSTALL_DIR=dest run_installer

    [ "$STATUS" -eq 0 ] || fail "exit $STATUS: $(cat "$CASE/err")"
    [ "$(cat "$LOG.path" 2>/dev/null)" = "$DEST/keepster" ] ||
        fail "keepster install ran as $(cat "$LOG.path" 2>/dev/null), not as $DEST/keepster"
}

test_stopped_by_a_signal_it_leaves_no_temporary_files() {
    new_case signalled
    release_for x86_64-unknown-linux-musl
    # Stopped while it unpacks: `tar` is the installer's own child, so its parent is the installer.
    cat >"$CASE/stubs/tar" <<'EOF'
#!/bin/sh
kill -TERM "$PPID"
EOF
    chmod +x "$CASE/stubs/tar"

    run_installer

    [ "$STATUS" -ne 0 ] || fail "it succeeded"
    [ -z "$(ls -A "$CASE/tmp")" ] || fail "it left $(ls -A "$CASE/tmp")"
    [ ! -e "$DEST" ] || fail "$DEST was created"
}

test_passes_its_arguments_to_keepster_install_and_exits_with_its_code() {
    new_case passes
    release_for x86_64-unknown-linux-musl

    KEEPSTER_STUB_EXIT=7 run_installer --no-tray --no-web-ui "two words"

    [ "$STATUS" -eq 7 ] || fail "exit $STATUS, not keepster install's 7"
    [ "$(cat "$LOG" 2>/dev/null)" = "$(printf -- '--no-tray\n--no-web-ui\ntwo words')" ] ||
        fail "keepster install got: $(cat "$LOG" 2>/dev/null)"
}

# Expect the run to have failed, said `$1` on stderr, and left no install folder behind.
expect_refused() {
    [ "$STATUS" -ne 0 ] || fail "it succeeded"
    grep -q -- "$1" "$CASE/err" || fail "stderr does not say '$1': $(cat "$CASE/err")"
    [ ! -e "$DEST" ] || fail "$DEST was created"
    [ ! -f "$LOG" ] || fail "keepster install was run"
}

test_refuses_an_archive_whose_checksum_does_not_match() {
    new_case mismatch
    release_for x86_64-unknown-linux-musl
    printf 'tampered' >>"$CASE/rel/keepster-$VERSION-x86_64-unknown-linux-musl.tar.xz"

    run_installer

    expect_refused "checksum"
}

test_refuses_an_archive_that_sha256sums_does_not_list() {
    new_case unlisted
    release_for x86_64-unknown-linux-musl
    : >"$CASE/rel/SHA256SUMS"

    run_installer

    expect_refused "SHA256SUMS"
}

test_an_upgrade_replaces_the_binaries_without_writing_into_the_old_ones() {
    new_case upgrade
    release_for x86_64-unknown-linux-musl
    mkdir -p "$DEST"
    printf '#!/bin/sh\necho old\n' >"$DEST/keepster"
    printf '#!/bin/sh\necho old tray\n' >"$DEST/keepster-tray"
    chmod +x "$DEST/keepster" "$DEST/keepster-tray"
    # A hard link stands for a running process: it holds the old file, and a binary written into in
    # place would change under it, where one renamed over it leaves it whole.
    ln "$DEST/keepster" "$CASE/running-keepster"
    ln "$DEST/keepster-tray" "$CASE/running-tray"

    run_installer

    [ "$STATUS" -eq 0 ] || fail "exit $STATUS: $(cat "$CASE/err")"
    [ "$("$DEST/keepster" --version)" = "keepster $VERSION" ] || fail "keepster was not replaced"
    [ "$(cat "$CASE/running-keepster")" = "$(printf '#!/bin/sh\necho old')" ] ||
        fail "the running keepster was written into"
    [ "$(cat "$CASE/running-tray")" = "$(printf '#!/bin/sh\necho old tray')" ] ||
        fail "the running tray was written into"
    [ "$(ls "$DEST")" = "$(printf 'keepster\nkeepster-tray')" ] ||
        fail "$DEST holds more than the binaries: $(ls "$DEST")"
}

test_a_folder_off_path_is_named_with_the_full_path_to_keepster() {
    new_case off-path
    release_for x86_64-unknown-linux-musl

    run_installer

    [ "$STATUS" -eq 0 ] || fail "exit $STATUS: $(cat "$CASE/err")"
    grep -q "$DEST/keepster" "$CASE/out" || fail "stdout does not name $DEST/keepster: $(cat "$CASE/out")"
    grep -q "PATH" "$CASE/out" || fail "stdout does not mention PATH: $(cat "$CASE/out")"
}

test_a_folder_on_path_needs_no_word_about_it() {
    new_case on-path
    release_for x86_64-unknown-linux-musl

    EXTRA_PATH="$DEST" run_installer

    [ "$STATUS" -eq 0 ] || fail "exit $STATUS: $(cat "$CASE/err")"
    ! grep -q "PATH" "$CASE/out" || fail "stdout mentions PATH: $(cat "$CASE/out")"
}

test_leaves_no_temporary_files_whether_it_succeeds_or_fails() {
    new_case tidy
    release_for x86_64-unknown-linux-musl

    run_installer
    [ "$STATUS" -eq 0 ] || fail "exit $STATUS: $(cat "$CASE/err")"
    [ -z "$(ls -A "$CASE/tmp")" ] || fail "a success left $(ls -A "$CASE/tmp")"

    : >"$CASE/rel/SHA256SUMS"
    run_installer
    [ "$STATUS" -ne 0 ] || fail "a release with no checksums succeeded"
    [ -z "$(ls -A "$CASE/tmp")" ] || fail "a failure left $(ls -A "$CASE/tmp")"
}

test_an_archive_tar_cannot_unpack_says_xz_is_needed() {
    # Only GNU tar runs an `xz` from PATH; macOS's tar unpacks xz itself, and needs no such word.
    tar --version 2>/dev/null | grep -q 'GNU tar' || return 0
    new_case no-xz
    release_for x86_64-unknown-linux-musl
    printf '#!/bin/sh\nexit 127\n' >"$CASE/stubs/xz"
    chmod +x "$CASE/stubs/xz"

    run_installer

    expect_refused "xz"
}

test_picks_the_archive_built_for_this_platform() {
    local platform os arch target
    for platform in "Linux amd64 x86_64-unknown-linux-musl" \
        "Linux aarch64 aarch64-unknown-linux-musl" "Linux arm64 aarch64-unknown-linux-musl" \
        "Darwin arm64 aarch64-apple-darwin"; do
        read -r os arch target <<<"$platform"
        new_case "picks-$os-$arch"
        stub_uname "$os" "$arch"
        release_for "$target"

        run_installer

        [ "$STATUS" -eq 0 ] || fail "$os $arch: exit $STATUS: $(cat "$CASE/err")"
        [ -x "$DEST/keepster" ] || fail "$os $arch: nothing installed from the $target archive"
    done
}

test_refuses_a_platform_with_no_release_build_and_names_it() {
    new_case intel-mac
    stub_uname Darwin x86_64
    release_for x86_64-unknown-linux-musl

    run_installer

    expect_refused "Darwin x86_64"
}

test_with_no_version_published_into_it_names_the_setting_that_gives_one() {
    new_case unversioned
    release_for x86_64-unknown-linux-musl

    KEEPSTER_VERSION='' run_installer

    expect_refused "KEEPSTER_VERSION"
}

test_a_version_with_no_release_names_what_could_not_be_downloaded() {
    new_case unreleased
    release_for x86_64-unknown-linux-musl

    KEEPSTER_VERSION=0.0.1 run_installer

    expect_refused "keepster-0.0.1-x86_64-unknown-linux-musl.tar.xz"
}

for test in $(declare -F | awk '{print $3}' | grep '^test_'); do
    echo "$test"
    "$test"
done

if [ "$failures" -ne 0 ]; then
    echo "$failures expectation(s) failed" >&2
    exit 1
fi
echo "all installer tests passed"
