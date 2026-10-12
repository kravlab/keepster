//! `keepster log-level`: the daemon's log level, raised for a while or back to its default (ticket 62).

use std::io::{self, Write};
use std::path::Path;

use keepster_api::{LogLevelRequest, LogStatus, Status};

use crate::client::DaemonClient;
use crate::command::LogLevelArgs;
use crate::exit;
use crate::render::log_level as shown;
use crate::roots::answered;

/// Show the level, raise it for a while, or end a raise — and say where things stand after.
pub async fn log_level(args: &LogLevelArgs, control_path: &Path) -> u8 {
    let client = DaemonClient::new(control_path);

    let log = if args.reset {
        answered::<LogStatus>(client.delete("/v1/log/level").await)
    } else if let Some(level) = &args.level {
        let request = LogLevelRequest {
            level: level.clone(),
            duration: args.duration.clone(),
        };
        let body = serde_json::to_vec(&request).expect("the request serialises");
        answered::<LogStatus>(client.put_json("/v1/log/level", &body).await)
    } else {
        answered::<Status>(client.get("/v1/status").await).and_then(|status| {
            status.daemon.log.ok_or_else(|| {
                eprintln!("keepster: this daemon does not report its log level");
                exit::OTHER
            })
        })
    };
    let log = match log {
        Ok(log) => log,
        Err(code) => return code,
    };

    let mut out = io::stdout().lock();
    match writeln!(out, "log level {}", shown(&log)).and_then(|()| out.flush()) {
        Ok(()) => exit::SUCCESS,
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => exit::SUCCESS,
        Err(error) => {
            eprintln!("keepster: could not write the answer: {error}");
            exit::OTHER
        }
    }
}
