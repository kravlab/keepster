//! `keepster diagnose`: one local zip for a bug report, its inventory printed first (ticket 63,
//! ADR 0013).
//!
//! Part of the binary rather than of `keepster-cli`: when no daemon answers, the bundle is collected
//! from the disk, which the thin client cannot read, and the archive is written with the domain core's
//! zip writer, as Export's is. It never starts a daemon — the command exists for the moment one will not run, and starting
//! it would hide exactly that.
//!
//! **Nothing is sent anywhere.** The daemon is asked over its control channel, and what it answers is
//! written into the one local file named here. There is no other destination, and there must not be.

use std::fs::OpenOptions;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use keepster_api::Diagnostics;
use keepster_cli::{ClientError, DaemonClient, DiagnoseArgs, exit};
use keepster_daemon::bundle::Bundle;
use keepster_daemon::diagnostics::{Places, on_disk};

/// Collect the bundle, print its inventory, and write it unless this is a dry run.
///
/// The exit code is the collection's: a bundle collected from the disk because no daemon answers is
/// written all the same, and exits as an unreachable daemon does, so a script can tell it is not whole.
pub(crate) async fn diagnose(args: &DiagnoseArgs, control_path: &Path, now: SystemTime) -> u8 {
    let (bundle, code) = collect(control_path).await;
    print!("{}", bundle.inventory());

    if args.dry_run {
        return code;
    }

    let path = args.output.clone().unwrap_or_else(|| default_name(now));
    match write_new(&path, |out| bundle.write_zip(out, now)) {
        Ok(()) => {
            println!(
                "\nThe bundle is {}. Nothing was sent anywhere.",
                path.display()
            );
            code
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            eprintln!(
                "keepster: {} already exists, and is never overwritten; nothing was written",
                path.display()
            );
            exit::STATE_CONFLICT
        }
        Err(error) => {
            eprintln!(
                "keepster: the bundle could not be written to {}: {error}",
                path.display()
            );
            exit::OTHER
        }
    }
}

/// The bundle and the exit code it calls for: the daemon's answer, or the disk when there is none.
async fn collect(control_path: &Path) -> (Bundle, u8) {
    let degraded = |why: String, code: u8| {
        let found = on_disk(&places(), &keepster_daemon::PlatformVolumes);
        (
            Bundle::from_disk(keepster_daemon::version_info(), found, &why),
            code,
        )
    };

    match DaemonClient::new(control_path).get("/v1/diagnostics").await {
        Ok(response) if response.status.is_success() => {
            match serde_json::from_slice::<Diagnostics>(&response.body) {
                Ok(found) => (
                    Bundle::from_daemon(keepster_daemon::version_info(), found),
                    exit::SUCCESS,
                ),
                Err(error) => degraded(
                    format!("the daemon's answer could not be read ({error})"),
                    exit::OTHER,
                ),
            }
        }
        Ok(response) => degraded(
            format!(
                "the daemon answered {}: {}",
                response.status,
                String::from_utf8_lossy(&response.body)
            ),
            exit::OTHER,
        ),
        Err(error @ ClientError::Unreachable { .. }) => {
            degraded(error.to_string(), exit::DAEMON_UNREACHABLE)
        }
        Err(error) => degraded(error.to_string(), exit::OTHER),
    }
}

/// Where the daemon keeps what the disk holds, as this platform resolves it for this user.
fn places() -> Places {
    Places {
        store_base: keepster_service::default_store_base(),
        config_path: keepster_service::default_config_path(),
        home: std::env::home_dir(),
        log_dir: keepster_service::default_log_dir(),
    }
}

/// Create the file at `path` and have `write` fill it, refusing a file that exists.
///
/// A file this created and could not finish is removed: a half-written bundle is no bundle, and left in
/// place it would be the file the next run refuses to overwrite.
fn write_new(path: &Path, write: impl FnOnce(&mut dyn Write) -> io::Result<()>) -> io::Result<()> {
    let file = OpenOptions::new().write(true).create_new(true).open(path)?;
    let mut out = BufWriter::new(file);
    let written = write(&mut out).and_then(|()| {
        out.into_inner()
            .map_err(io::IntoInnerError::into_error)?
            .sync_all()
    });
    if written.is_err() {
        // The error that stopped the write is the one to report; a failed removal adds nothing to it.
        let _ = std::fs::remove_file(path);
    }

    written
}

/// `keepster-diagnose-<moment>.zip` in the current directory, the moment in UTC and safe in a file
/// name on every platform.
fn default_name(now: SystemTime) -> PathBuf {
    let moment = jiff::Timestamp::try_from(now)
        .map(|moment| moment.strftime("%Y%m%dT%H%M%SZ").to_string())
        .unwrap_or_else(|_| "unknown".to_owned());

    PathBuf::from(format!("keepster-diagnose-{moment}.zip"))
}

#[cfg(test)]
mod tests {
    //! Writing the bundle's file: a new one or none, never a half-written one left in the way.

    use std::io;

    use super::write_new;

    #[test]
    fn a_bundle_that_fails_part_way_leaves_no_file_behind() {
        let directory = tempfile::tempdir().expect("a directory");
        let path = directory.path().join("bundle.zip");

        let written = write_new(&path, |out| {
            out.write_all(b"PK\x03\x04")?;
            Err(io::Error::other("the disk said no"))
        });

        assert_eq!(
            written.expect_err("it fails").to_string(),
            "the disk said no"
        );
        assert!(
            !path.exists(),
            "a half-written bundle would refuse the next run"
        );
    }

    #[test]
    fn a_file_already_there_is_refused_and_kept() {
        let directory = tempfile::tempdir().expect("a directory");
        let path = directory.path().join("bundle.zip");
        std::fs::write(&path, b"someone's file").expect("a file");

        let written = write_new(&path, |out| out.write_all(b"PK"));

        assert_eq!(
            written.expect_err("it is refused").kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(
            std::fs::read(&path).expect("still there"),
            b"someone's file"
        );
    }
}
