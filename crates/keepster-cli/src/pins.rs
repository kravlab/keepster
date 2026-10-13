//! `keepster pin`, `keepster unpin` and `keepster pins` (ticket 53): a Snapshot that survives Retention
//! unconditionally — of a watched Root, or of one no longer watched, reached by its full identity.

use std::io;
use std::path::Path;
use std::time::SystemTime;

use keepster_api::Pins;

use crate::client::DaemonClient;
use crate::command::{PinArgs, PinsArgs, UnpinArgs};
use crate::exit;
use crate::history::{selected_in, write_log};
use crate::resolve::{Target, held};
use crate::roots::{answered, body_of};

/// Pin a Snapshot, `latest` unless another is named.
pub async fn pin(args: &PinArgs, control_path: &Path, now: SystemTime) -> u8 {
    let client = DaemonClient::new(control_path);
    let typed = args.snapshot.as_deref().unwrap_or("latest");

    let Target { id: root, named } = match held(&client, args.root.as_deref()).await {
        Ok(found) => found,
        Err(code) => return code,
    };
    let snapshot = match selected_in(&client, &root, &named, typed, now).await {
        Ok(snapshot) => snapshot,
        Err(code) => return code,
    };

    match body_of(
        client
            .put(&format!("/v1/roots/{root}/pins/{snapshot}"))
            .await,
    ) {
        Ok(_) => {
            println!("pinned {snapshot}");
            exit::SUCCESS
        }
        Err(code) => code,
    }
}

/// Unpin a Snapshot, which is Retention's again.
pub async fn unpin(args: &UnpinArgs, control_path: &Path, now: SystemTime) -> u8 {
    let client = DaemonClient::new(control_path);

    let Target { id: root, named } = match held(&client, args.root.as_deref()).await {
        Ok(found) => found,
        Err(code) => return code,
    };
    let snapshot = match selected_in(&client, &root, &named, &args.snapshot, now).await {
        Ok(snapshot) => snapshot,
        Err(code) => return code,
    };

    match body_of(
        client
            .delete(&format!("/v1/roots/{root}/pins/{snapshot}"))
            .await,
    ) {
        Ok(_) => {
            println!("unpinned {snapshot}");
            exit::SUCCESS
        }
        Err(code) => code,
    }
}

/// List the pinned Snapshots, newest first, as `log` lists Snapshots.
pub async fn pins(args: &PinsArgs, control_path: &Path) -> u8 {
    let client = DaemonClient::new(control_path);

    let root = match held(&client, args.root.as_deref()).await {
        Ok(found) => found.id,
        Err(code) => return code,
    };
    // Kept as the daemon sent them, so `--json` passes every field through, as `log --json` does.
    let listed: serde_json::Value =
        match answered(client.get(&format!("/v1/roots/{root}/pins")).await) {
            Ok(listed) => listed,
            Err(code) => return code,
        };
    if let Err(error) = serde_json::from_value::<Pins>(listed.clone()) {
        eprintln!("keepster: the daemon's answer could not be read: {error}");
        return exit::OTHER;
    }
    let items = listed["snapshots"].as_array().cloned().unwrap_or_default();
    crate::history::warn_unreadable(&listed["unreadable"]);

    write_log(&items, args.json, &mut io::stdout().lock())
}
