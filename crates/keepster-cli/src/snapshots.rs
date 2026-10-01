//! `keepster snapshot`: capturing a Root's contents on demand.

use std::path::Path;

use keepster_api::Snapshot;

use crate::client::DaemonClient;
use crate::command::SnapshotArgs;
use crate::exit;
use crate::roots::answered;

/// Capture the Root's current contents now.
///
/// Ticket 43 makes this happen by itself; until then it is how a person or a script asks for a Snapshot,
/// and how the write path is exercised end to end.
pub async fn snapshot(args: &SnapshotArgs, control_path: &Path) -> u8 {
    let client = DaemonClient::new(control_path);

    let root = match crate::resolve::resolve(&client, args.root.as_deref()).await {
        Ok(root) => root,
        Err(code) => return code,
    };

    let answer = client
        .post(&format!("/v1/roots/{}/snapshots", root.id), b"{}")
        .await;

    match answered::<Snapshot>(answer) {
        Ok(taken) => {
            println!(
                "snapshot {} of {} in segment {}",
                taken.id, root.path, taken.segment
            );

            exit::SUCCESS
        }
        Err(code) => code,
    }
}
