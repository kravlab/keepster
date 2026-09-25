//! Turning the daemon's answers into output. Human rendering is the CLI's own; the JSON form is the
//! daemon's payload, untouched.

use std::io::{self, Write};
use std::path::Path;

use keepster_api::{Reported, Root, RootState, Status};

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
pub fn write_human(status: &Status, out: &mut impl Write) -> io::Result<()> {
    let daemon = &status.daemon;

    writeln!(
        out,
        "keepster {} ({})",
        daemon.version.build_version, daemon.version.build_hash
    )?;
    writeln!(out, "  api          {}", daemon.version.api_version)?;
    writeln!(out, "  pid          {}", daemon.pid)?;
    writeln!(out, "  uptime       {}s", daemon.uptime_seconds)?;
    writeln!(out, "  autostart    {}", reported(daemon.autostart))?;
    writeln!(out, "  linger       {}", reported(daemon.linger))?;
    writeln!(out, "  maintenance  {}", reported(daemon.maintenance))?;

    write_roots(&status.roots, out)
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
    }
}

fn root_state(state: RootState) -> &'static str {
    match state {
        RootState::Watching => "watching",
        RootState::Degraded => "degraded",
        RootState::Unavailable => "unavailable",
    }
}
