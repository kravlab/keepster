//! Turning the daemon's answers into output. Human rendering is the CLI's own; the JSON form is the
//! daemon's payload, untouched.

use std::io::{self, Write};

use keepster_api::{Reported, RootState, Status};

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

    if status.roots.is_empty() {
        writeln!(out, "  no watched roots registered")?;
    } else {
        for root in &status.roots {
            writeln!(out, "  {}  {}", root.id, root_state(root.state))?;
        }
    }

    Ok(())
}

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
