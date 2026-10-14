# Keepster

Editor-independent file history. A background daemon captures snapshots of the directories you
watch into its own git-backed store, so work overwritten by an agent, a script or a stray command
can be recovered — without relying on the directory's own version control.

## Features

- **Automatic snapshots** on file changes, debounced and rate-limited; `keepster snapshot` takes one on demand.
- **Separate history store**: it lives outside the watched directory and is invisible to its own Git.
- **Include rules** per watched directory, in the global configuration or a shared `.keepster.toml`;
  `.gitignore` is an optional layer beneath them.
- **Undoable restore**: a Safety Snapshot is taken before every restore. Export writes old versions
  into another directory or a zip archive and leaves the files untouched.
- **Search the past**: `find` a file by name (deleted ones included), `grep` text across every version.
- **Retention** thins old snapshots by age; **pins** keep chosen ones forever.
- **`forget`** removes a captured secret from the whole history.
- **Web UI** on `127.0.0.1`, token-protected and switchable off, plus a **tray** that alerts when
  capture stops.
- **Per-user service**: a systemd user unit, a LaunchAgent or a Scheduled Task; no administrator rights.

## Platforms

| OS      | Architecture           |
|---------|------------------------|
| Linux   | x86_64, aarch64 (musl) |
| macOS   | aarch64                |
| Windows | x86_64                 |

## Install

### With the install script

Linux and macOS:

```sh
curl -LsSf https://raw.githubusercontent.com/kravlab/keepster/main/scripts/install.sh | sh
```

Windows (PowerShell):

```powershell
irm https://raw.githubusercontent.com/kravlab/keepster/main/scripts/install.ps1 | iex
```

The script installs the latest release: it downloads the archive for your platform, checks it against
`SHA256SUMS`, puts `keepster` and `keepster-tray` in `~/.local/bin` (Windows:
`%LOCALAPPDATA%\Programs\Keepster`) and runs `keepster install` from there. It leaves `PATH` alone and
says when that directory is not on it. Running it again upgrades in place.

- `KEEPSTER_VERSION` picks another release and `KEEPSTER_INSTALL_DIR` another directory:
  `curl -LsSf … | KEEPSTER_VERSION=0.1.0 sh`, or on Windows `$env:KEEPSTER_VERSION = '0.1.0'` first.
- Arguments go to `keepster install`: `curl -LsSf … | sh -s -- --no-tray`, or on Windows
  `.\install.ps1 --no-tray` for a downloaded copy.

### By hand

1. Download the archive for your platform from [Releases](https://github.com/kravlab/keepster/releases)
   and check it against `SHA256SUMS`.
2. Put `keepster` and `keepster-tray` side by side in a permanent location, for example a directory on
   your `PATH`. The service runs the binary from where it is installed.
3. Register the daemon and the tray:

   ```sh
   keepster install            # --no-tray to skip the tray, --no-web-ui for no web UI
   keepster service status
   ```

`keepster uninstall` removes what `install` created; history is erased only when asked with `--yes`.

## Quick start

```sh
keepster watch ~/code/app          # put a directory under history (--dry-run to preview)
keepster status                    # is work being captured right now?
keepster log                       # snapshots of the current root, newest first
keepster diff --from 2h            # what changed in the last two hours
keepster cat src/main.rs -s ~3     # a file as it was three snapshots ago
keepster restore src/main.rs --from 1h --dry-run
keepster ui                        # open the web UI, signed in
```

Run `keepster help <command>` for every command and option.

## Building from source

Requires Rust 1.87 or newer.

```sh
cargo build --release --locked -p keepster-daemon -p keepster-tray
```

The web UI is embedded from the committed `ui/dist`; Node and pnpm are needed only to change it
(`make ui`).

## License

MIT — see [LICENSE](LICENSE).
