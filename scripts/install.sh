#!/bin/sh

# Install Keepster from a GitHub release on Linux or macOS (ticket 85):
#
#   curl -LsSf https://github.com/kravlab/keepster/releases/latest/download/install.sh | sh
#
# It downloads this platform's archive, checks it against the release's SHA256SUMS, puts `keepster`
# and `keepster-tray` side by side in one lasting folder and runs `keepster install` from there,
# which registers the daemon and the tray at that path (ticket 27: an archive plus `keepster
# install` is the one install mechanism). Its own arguments go to `keepster install`
# (`sh -s -- --no-tray`), and its exit code is that command's.
#
# Settings, all optional:
#   KEEPSTER_VERSION      the version to install, e.g. 0.1.0; by default, the release this script
#                         was published in
#   KEEPSTER_INSTALL_DIR  where the binaries go; by default ~/.local/bin
#   KEEPSTER_BASE_URL     where the release's files are; by default its GitHub release
#
# PATH is left alone: nothing changes behind the user's back (ticket 27).
#
# Everything runs from `main`, called on the last line: a shell reading the script from a pipe runs
# nothing before the whole of it has arrived, so a download cut short runs nothing at all, and no
# command can read the rest of the script as its own input.

set -eu

# Written by the release workflow when it publishes this script; empty in the repository.
release_version=''

# Say what went wrong and stop.
die() {
    echo "keepster installer: $*" >&2
    exit 1
}

# Fetch `$1` from the release at `$base` into the temporary folder `$tmp`, with curl or, failing
# that, wget.
download() {
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL -o "$tmp/$1" "$base/$1" || die "could not download $base/$1"
    elif command -v wget >/dev/null 2>&1; then
        wget -q -O "$tmp/$1" "$base/$1" || die "could not download $base/$1"
    else
        die "neither curl nor wget is installed, so the release cannot be downloaded"
    fi
}

main() {
    version=${KEEPSTER_VERSION:-$release_version}
    [ -n "$version" ] || die "this copy names no release; set KEEPSTER_VERSION, e.g. KEEPSTER_VERSION=0.1.0"
    base=${KEEPSTER_BASE_URL:-https://github.com/kravlab/keepster/releases/download/v$version}

    # The release's targets (ticket 27). Linux builds are static musl, so they run on any
    # distribution; there is no Intel macOS build.
    platform="$(uname -s) $(uname -m)"
    case $platform in
        "Linux x86_64" | "Linux amd64") target=x86_64-unknown-linux-musl ;;
        "Linux aarch64" | "Linux arm64") target=aarch64-unknown-linux-musl ;;
        "Darwin arm64") target=aarch64-apple-darwin ;;
        *) die "no release build for $platform; there are builds for Linux x86_64 and aarch64, and macOS arm64" ;;
    esac
    name="keepster-$version-$target"

    # What a run leaves behind is removed however it ends: the temporary folder, and a binary copied
    # beside its final name but not yet renamed over it. The EXIT trap covers a normal end and `die`;
    # dash runs no EXIT trap for a signal, so an interruption or a termination is turned into an exit
    # first.
    dest=
    tmp=$(mktemp -d)
    trap 'rm -rf "$tmp"; [ -z "$dest" ] || rm -f "$dest/.keepster.new" "$dest/.keepster-tray.new"' EXIT
    trap 'exit 130' INT
    trap 'exit 143' TERM

    download "$name.tar.xz"
    download SHA256SUMS

    # The archive's SHA-256 as SHA256SUMS lists it, then as computed: Linux has `sha256sum`, macOS
    # `shasum`.
    expected=$(awk -v file="$name.tar.xz" '$2 == file || $2 == "*" file { print $1 }' "$tmp/SHA256SUMS")
    [ -n "$expected" ] || die "SHA256SUMS lists no $name.tar.xz"
    if command -v sha256sum >/dev/null 2>&1; then
        actual=$(sha256sum "$tmp/$name.tar.xz")
    else
        actual=$(shasum -a 256 "$tmp/$name.tar.xz")
    fi
    actual=${actual%% *}
    [ "$actual" = "$expected" ] || die "$name.tar.xz does not match its checksum in SHA256SUMS"

    # GNU tar hands xz archives to an `xz` program, which a minimal Linux may lack; macOS's tar
    # needs none.
    tar -xJf "$tmp/$name.tar.xz" -C "$tmp" ||
        die "could not unpack $name.tar.xz; on Linux, tar needs xz installed (the xz-utils or xz package)"

    # Made absolute, because `keepster install` registers the path it is run by, and macOS reports
    # that path as given. Logical rather than resolved, so a link the user chose stays theirs.
    mkdir -p "${KEEPSTER_INSTALL_DIR:-$HOME/.local/bin}"
    dest=$(cd "${KEEPSTER_INSTALL_DIR:-$HOME/.local/bin}" && pwd)

    # Each binary is copied beside its final name and renamed over it, so an upgrade never writes
    # into a file a running daemon or tray is executing; `keepster install` then restarts the daemon
    # from the new one.
    for bin in keepster keepster-tray; do
        cp "$tmp/$name/$bin" "$dest/.$bin.new"
        chmod 755 "$dest/.$bin.new"
        mv -f "$dest/.$bin.new" "$dest/$bin"
    done

    code=0
    "$dest/keepster" install "$@" || code=$?

    case ":$PATH:" in
        *":$dest:"*) ;;
        *) echo "keepster is at $dest/keepster; $dest is not on PATH, so add it there to run plain \`keepster\`" ;;
    esac
    exit "$code"
}

main "$@"
