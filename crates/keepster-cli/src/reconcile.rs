//! `keepster reconcile`: compare a Root against its latest Snapshot now, and capture what differs.

use std::io::{self, Write};
use std::path::Path;

use keepster_api::Reconciled;

use crate::client::DaemonClient;
use crate::command::ReconcileArgs;
use crate::exit;
use crate::roots::body_of;

/// Run one Reconciliation of the Root and say what it found.
pub async fn reconcile(args: &ReconcileArgs, control_path: &Path) -> u8 {
    let client = DaemonClient::new(control_path);

    let root = match crate::resolve::resolve(&client, args.root.as_deref()).await {
        Ok(root) => root,
        Err(code) => return code,
    };
    let request: &[u8] = if args.verify {
        br#"{"verify":"hash"}"#
    } else {
        b"{}"
    };
    let body = match body_of(
        client
            .post(&format!("/v1/roots/{}/reconcile", root.id), request)
            .await,
    ) {
        Ok(body) => body,
        Err(code) => return code,
    };

    let mut out = io::stdout().lock();
    let written = if args.json {
        // Verbatim, as `status --json` is: one schema, the daemon's.
        out.write_all(&body)
    } else {
        match serde_json::from_slice::<Reconciled>(&body) {
            Ok(reconciled) => write_reconciled(&reconciled, &root.path, &mut out),
            Err(error) => {
                eprintln!("keepster: the daemon's answer could not be read: {error}");
                return exit::OTHER;
            }
        }
    };

    match written.and_then(|()| out.flush()) {
        Ok(()) => exit::SUCCESS,
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => exit::SUCCESS,
        Err(error) => {
            eprintln!("keepster: could not write the answer: {error}");
            exit::OTHER
        }
    }
}

fn write_reconciled(reconciled: &Reconciled, root: &str, out: &mut impl Write) -> io::Result<()> {
    writeln!(
        out,
        "compared {} files of {root}, read {}",
        reconciled.observed, reconciled.hashed
    )?;
    match &reconciled.snapshot {
        Some(taken) => writeln!(out, "snapshot {} holds what had changed", taken.id)?,
        None => writeln!(out, "nothing had changed since the latest snapshot")?,
    }
    if reconciled.coarse_mtime {
        writeln!(
            out,
            "note: every mtime fell on a whole second, so changes within one second can be missed; \
             `keepster reconcile --verify` reads everything"
        )?;
    }

    Ok(())
}
