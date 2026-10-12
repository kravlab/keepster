//! Turning the daemon's answers into output. Human rendering is the CLI's own; the JSON form is the
//! daemon's payload, untouched.

use std::io::{self, Write};
use std::path::Path;

use keepster_api::{GitStatus, ListenerState, ListenerStatus, Reported, Root, RootState, Status};

/// Write the daemon's payload exactly as it arrived.
///
/// Verbatim, byte for byte: `status --json` is a passthrough of `GET /v1/status`, so a field added to
/// the API reaches scripts without the CLI being taught about it, and two schemas for one meaning
/// never exist (ticket 07). Parsing and re-serialising would silently drop anything this build does
/// not know, which is the drift the passthrough exists to prevent.
pub fn write_json(body: &[u8], out: &mut impl Write) -> io::Result<()> {
    out.write_all(body)
}

/// Render the status rows for a person.
///
/// Every row is named even when its value is not known yet, because ticket 05 forbids the tool to
/// look healthier than it is: a missing row reads as "fine", an `unknown` one does not.
///
/// `shell_git` is the `git` this command's own `PATH` finds, offered as a `git.path` line when the
/// daemon's search, which never reads a `PATH`, came up short.
pub fn write_human(
    status: &Status,
    shell_git: Option<&Path>,
    out: &mut impl Write,
) -> io::Result<()> {
    let daemon = &status.daemon;

    writeln!(
        out,
        "keepster {} ({})",
        daemon.version.build_version, daemon.version.build_hash
    )?;
    writeln!(out, "  api          {}", daemon.version.api_version)?;
    match &daemon.version.store_format {
        Some(format) => writeln!(
            out,
            "  store format container {}, content {}, sidecar {}, index {}",
            format.container, format.content, format.sidecar, format.index
        )?,
        None => writeln!(out, "  store format unknown")?,
    }
    writeln!(out, "  pid          {}", daemon.pid)?;
    writeln!(out, "  uptime       {}s", daemon.uptime_seconds)?;
    writeln!(
        out,
        "  autostart    {}",
        autostart(daemon.autostart, daemon.autostart_reason.as_deref())
    )?;
    writeln!(
        out,
        "  linger       {}",
        linger(daemon.linger, daemon.linger_reason.as_deref())
    )?;
    writeln!(out, "  maintenance  {}", reported(daemon.maintenance))?;
    write_git(&daemon.git, shell_git, out)?;
    if let Some(listener) = &daemon.listener {
        write_listener(listener, out)?;
    }

    write_roots(&status.roots, out)
}

/// Whether the daemon comes back at login, and what to do when it does not.
pub fn autostart(row: Reported, reason: Option<&str>) -> String {
    match (row, reason) {
        (Reported::Unavailable, Some("not_installed")) => {
            "not installed — the daemon will not come back after a reboot; run `keepster service \
             install`"
                .to_owned()
        }
        (Reported::Unavailable, Some("disabled")) => {
            "installed but disabled — the daemon will not come back after a reboot; run `keepster \
             service install`"
                .to_owned()
        }
        (row, _) => reported(row).to_owned(),
    }
}

/// Whether coverage survives logout. The gap a platform imposes is said plainly, never hidden
/// (ticket 05).
pub fn linger(row: Reported, reason: Option<&str>) -> String {
    match (row, reason) {
        (Reported::Unavailable, Some("logout_gap")) => {
            "unavailable — coverage ends at logout".to_owned()
        }
        (Reported::Unavailable, Some("disabled")) => {
            "off — coverage ends at logout; `loginctl enable-linger` keeps it running".to_owned()
        }
        (row, _) => reported(row).to_owned(),
    }
}

/// Whether the daemon has a network surface. Off is a choice, said as one rather than as a fault.
fn write_listener(listener: &ListenerStatus, out: &mut impl Write) -> io::Result<()> {
    match (listener.state, &listener.address) {
        (ListenerState::Listening, Some(address)) => {
            writeln!(out, "  web ui       listening on http://{address}")
        }
        (ListenerState::Off, _) => writeln!(out, "  web ui       off (ui.listener = false)"),
        (ListenerState::Failed, _) | (ListenerState::Listening, None) => writeln!(
            out,
            "  web ui       not listening: {}",
            listener
                .detail
                .as_deref()
                .unwrap_or("the loopback listener could not bind")
        ),
    }
}

/// Render Watched Roots, one per line, for `status`, `watch` and `roots` alike.
///
/// A Root is named the way a person recognises it — the directory's own name plus a short prefix of the
/// identity — because the full ULID is what URLs and scripts use, not what a human reads.
pub fn write_roots(roots: &[Root], out: &mut impl Write) -> io::Result<()> {
    if roots.is_empty() {
        writeln!(out, "  no watched roots registered")?;

        return Ok(());
    }

    for root in roots {
        let name = Path::new(&root.path).file_name().map_or_else(
            || root.path.clone(),
            |name| name.to_string_lossy().into_owned(),
        );
        let short_id: String = root.id.chars().take(SHORT_ID_LENGTH).collect();

        write!(out, "  {name} ({short_id})  {}", root_state(root.state))?;
        match &root.state_code {
            Some(code) => writeln!(out, " [{code}]  {}", root.path)?,
            None => writeln!(out, "  {}", root.path)?,
        }

        // Ticket 07: a Root whose directory is gone is shown with its last known path and the exact
        // command that repairs it, because a move is only ever explicit — the daemon cannot tell a move
        // from a deletion and must not guess.
        if let Some(records) = root.unrecognized_records {
            writeln!(
                out,
                "      {records} metadata record(s) from a newer Keepster are not understood by this \
                 build and are not carried forward"
            )?;
        }
        // Ticket 20: the pending change set is a count and an age, never a listing — a listing of a debounce
        // queue is stale as it prints.
        if root.pending_paths > 0 {
            let plural = if root.pending_paths == 1 { "" } else { "s" };
            match root.oldest_pending_seconds {
                Some(age) => writeln!(
                    out,
                    "      {} change{plural} pending, the oldest {age}s ago",
                    root.pending_paths
                )?,
                None => writeln!(out, "      {} change{plural} pending", root.pending_paths)?,
            }
        }
        // Ticket 56: what each volume state means for the history, so that `unavailable` is never read
        // as "gone".
        let hint = match root.state_code.as_deref() {
            Some("volume_mismatch") => Some(
                "its disk is not mounted there, so nothing is captured; its history reads as ever"
                    .to_owned(),
            ),
            Some("capture_held") => Some(format!(
                "every file is gone and nothing proves it is the disk it was; to record that, run: \
                 keepster snapshot -r {}",
                root.id
            )),
            Some("store_unreachable") => Some(
                "its history store cannot be written, so nothing is captured, and nowhere else instead"
                    .to_owned(),
            ),
            Some("watch_blind") => Some(
                "changes made on the server are never announced here; it is reconciled periodically"
                    .to_owned(),
            ),
            _ => None,
        };
        if let Some(hint) = hint {
            writeln!(out, "      {hint}")?;
        }
        if root.state_code.as_deref() == Some("root_missing") {
            writeln!(
                out,
                "      moved? run: keepster move -r {} <new-path>",
                root.id
            )?;
        }
    }

    Ok(())
}

/// How much of an identity a human is shown. Enough to tell Roots apart in a listing; the full id
/// remains the only form a URL or a script uses.
const SHORT_ID_LENGTH: usize = 8;

fn reported(value: Reported) -> &'static str {
    match value {
        Reported::Unknown => "unknown",
        Reported::Available => "available",
        Reported::Unavailable => "unavailable",
    }
}

/// The `git` maintenance runs with — or, without one, why and how to install one. Keepster prints the
/// command and never runs it (ticket 11).
/// Render what the daemon says of its `git` — the one block `status` and `watch` both print (ticket 11).
pub fn write_git(
    git: &GitStatus,
    shell_git: Option<&Path>,
    out: &mut impl Write,
) -> io::Result<()> {
    match (&git.path, &git.version) {
        (Some(path), Some(version)) => writeln!(out, "  git          {version} at {path}")?,
        (Some(path), None) => writeln!(out, "  git          {path}")?,
        _ => {}
    }
    let Some(reason) = &git.reason else {
        return Ok(());
    };
    let why = match reason.as_str() {
        "missing" => "no git was found".to_owned(),
        "macos_stub" => {
            "only the macOS stub is installed, without the Command Line Tools".to_owned()
        }
        "too_old" => format!("it is older than {}", git.minimum),
        "not_executable" => "it could not be run".to_owned(),
        // The daemon does not look again until `git.path` changes or it restarts: what git said
        // stays checkable meanwhile.
        "failed" => {
            writeln!(
                out,
                "               a Maintenance Pass with it failed; nothing Retention removes is reclaimed until the daemon restarts or git.path changes"
            )?;
            if let Some(detail) = &git.detail {
                writeln!(out, "               git said: {detail}")?;
            }
            return Ok(());
        }
        other => other.to_owned(),
    };
    writeln!(
        out,
        "               {why}; nothing Retention removes is reclaimed until git {} or newer is installed",
        git.minimum
    )?;
    if let Some(detail) = &git.detail {
        writeln!(out, "               git said: {detail}")?;
    }
    // The daemon never reads a `PATH`; this command's own is what a person's shell runs, so a `git`
    // there is offered — never written — as the key that makes it the daemon's.
    if let Some(shell_git) =
        shell_git.filter(|found| git.path.as_deref() != Some(&*found.to_string_lossy()))
    {
        writeln!(
            out,
            "               or use the git your shell runs, in the global configuration:"
        )?;
        writeln!(out, "                 [git]")?;
        writeln!(out, "                 path = {}", toml_string(shell_git))?;
    }
    if let Some(install) = &git.install {
        if let Some(command) = &install.command {
            writeln!(out, "               install it: {command}")?;
        }
        if let Some(note) = &install.note {
            writeln!(out, "               {note}")?;
        }
        writeln!(out, "               {}", install.link)?;
    }

    Ok(())
}

fn root_state(state: RootState) -> &'static str {
    match state {
        RootState::Watching => "watching",
        RootState::Degraded => "degraded",
        RootState::Unavailable => "unavailable",
    }
}

/// `path` as a TOML string: a literal one where it can be, so a Windows path reads as it is written.
fn toml_string(path: &Path) -> String {
    let text = path.to_string_lossy();
    if text.contains('\'') {
        format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        format!("'{text}'")
    }
}
