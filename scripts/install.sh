#!/bin/sh

# Install Keepster from a GitHub release on Linux or macOS (ticket 85):
#
#   curl -LsSf https://raw.githubusercontent.com/kravlab/keepster/main/scripts/install.sh | sh
#
# It downloads this platform's archive, checks it against the release's SHA256SUMS, puts `keepster`
# and `keepster-tray` side by side in one lasting folder and runs `keepster install` from there,
# which registers the daemon and the tray at that path (ticket 27: an archive plus `keepster
# install` is the one install mechanism). Its own arguments go to `keepster install`
# (`sh -s -- --no-tray`), and its exit code is that command's.
#
# The script is served from the repository rather than from a release, so it carries no version:
# with none given it installs the latest release, whose version it reads from the archive names in
# that release's SHA256SUMS. Every release already published is installable this way.
#
# Settings, all optional:
#   KEEPSTER_VERSION       the version to install, e.g. 0.1.0; by default, the latest release
#   KEEPSTER_INSTALL_DIR   where the binaries go; by default ~/.local/bin
#   KEEPSTER_RELEASES_URL  the releases' root, laid out as GitHub's: `download/v<version>/<file>`
#                          and `latest/download/<file>`; by default the GitHub releases
#
# PATH is left alone: nothing changes behind the user's back (ticket 27).
#
# Everything runs from `main`, called on the last line: a shell reading the script from a pipe runs
# nothing before the whole of it has arrived, so a download cut short runs nothing at all, and no
# command can read the rest of the script as its own input.

set -eu

# Say what went wrong and stop.
die() {
    echo "keepster installer: $*" >&2
    exit 1
}

# Fetch the URL `$1` into the file `$2` of the temporary folder `$tmp`, with curl or, failing that,
# wget.
download() {
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL -o "$tmp/$2" "$1" || die "could not download $1"
    elif command -v wget >/dev/null 2>&1; then
        wget -q -O "$tmp/$2" "$1" || die "could not download $1"
    else
        die "neither curl nor wget is installed, so the release cannot be downloaded"
    fi
}

# Print the version of the `$1` target's archive, `keepster-<version>-$1.tar.xz`, as the
# SHA256SUMS file `$2` lists it; print nothing when it lists none. A name may be marked `*`, as
# hashed in binary mode.
version_in_sums() {
    awk -v suffix="-$1.tar.xz" '{
        file = $2
        sub(/^\*/, "", file)
        if (substr(file, 1, 9) == "keepster-" && length(file) > 9 + length(suffix) &&
            substr(file, length(file) - length(suffix) + 1) == suffix) {
            print substr(file, 10, length(file) - 9 - length(suffix))
            exit
        }
    }' "$2"
}

main() {
    releases=${KEEPSTER_RELEASES_URL:-https://github.com/kravlab/keepster/releases}

    # The release's targets (ticket 27). Linux builds are static musl, so they run on any
    # distribution; there is no Intel macOS build.
    platform="$(uname -s) $(uname -m)"
    case $platform in
        "Linux x86_64" | "Linux amd64") target=x86_64-unknown-linux-musl ;;
        "Linux aarch64" | "Linux arm64") target=aarch64-unknown-linux-musl ;;
        "Darwin arm64") target=aarch64-apple-darwin ;;
        *) die "no release build for $platform; there are builds for Linux x86_64 and aarch64, and macOS arm64" ;;
    esac

    # What a run leaves behind is removed however it ends: the temporary folder, and a binary copied
    # beside its final name but not yet renamed over it. The EXIT trap covers a normal end and `die`;
    # dash runs no EXIT trap for a signal, so an interruption or a termination is turned into an exit
    # first.
    dest=
    tmp=$(mktemp -d)
    trap 'rm -rf "$tmp"; [ -z "$dest" ] || rm -f "$dest/.keepster.new" "$dest/.keepster-tray.new"' EXIT
    trap 'exit 130' INT
    trap 'exit 143' TERM

    # A version given names its release. With none, the latest release's SHA256SUMS names the
    # version; the archive is then fetched from that version's own release, so a release published
    # in between cannot mix into this run.
    version=${KEEPSTER_VERSION:-}
    if [ -n "$version" ]; then
        download "$releases/download/v$version/keepster-$version-$target.tar.xz" archive.tar.xz
        download "$releases/download/v$version/SHA256SUMS" SHA256SUMS
    else
        download "$releases/latest/download/SHA256SUMS" SHA256SUMS
        version=$(version_in_sums "$target" "$tmp/SHA256SUMS")
        [ -n "$version" ] || die "the latest release has no build for $target"
        download "$releases/download/v$version/keepster-$version-$target.tar.xz" archive.tar.xz
    fi
    name="keepster-$version-$target"

    # The archive's SHA-256 as SHA256SUMS lists it, then as computed: Linux has `sha256sum`, macOS
    # `shasum`.
    expected=$(awk -v file="$name.tar.xz" '$2 == file || $2 == "*" file { print $1 }' "$tmp/SHA256SUMS")
    [ -n "$expected" ] || die "SHA256SUMS lists no $name.tar.xz"
    if command -v sha256sum >/dev/null 2>&1; then
        actual=$(sha256sum "$tmp/archive.tar.xz")
    else
        actual=$(shasum -a 256 "$tmp/archive.tar.xz")
    fi
    actual=${actual%% *}
    [ "$actual" = "$expected" ] || die "$name.tar.xz does not match its checksum in SHA256SUMS"

    # GNU tar hands xz archives to an `xz` program, which a minimal Linux may lack; macOS's tar
    # needs none.
    tar -xJf "$tmp/archive.tar.xz" -C "$tmp" ||
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
