//! `keepster ui`: open the web UI signed in, without the token ever appearing in a URL — and
//! `keepster ui token rotate`, which ends every browser session (ticket 60).

use std::io::{self, Write};
use std::path::Path;

use keepster_api::UiBootstrap;

use crate::client::DaemonClient;
use crate::command::{TokenCommand, UiArgs, UiCommand};
use crate::exit;
use crate::roots::{answered, body_of};

/// Open a browser session, or rotate the token.
///
/// With the listener off both fail alike, with the daemon's own `listener_unavailable` and its exit
/// code: there is no session to open and no token to rotate, and the command does not start a listener
/// for the occasion (ticket 24).
pub async fn ui(args: &UiArgs, control_path: &Path) -> u8 {
    let client = DaemonClient::new(control_path);

    match &args.command {
        Some(UiCommand::Token(TokenCommand::Rotate)) => {
            match body_of(client.post("/v1/ui/token/rotate", b"{}").await) {
                Ok(_) => {
                    eprintln!("keepster: the token was replaced; every browser session has ended");
                    exit::SUCCESS
                }
                Err(code) => code,
            }
        }
        None => {
            let bootstrap =
                match answered::<UiBootstrap>(client.post("/v1/ui/bootstrap", b"{}").await) {
                    Ok(bootstrap) => bootstrap,
                    Err(code) => return code,
                };
            if !args.no_browser {
                match open(&bootstrap.url) {
                    Ok(()) => return exit::SUCCESS,
                    Err(error) => {
                        eprintln!(
                            "keepster: no browser could be opened ({error}); open this address:"
                        );
                    }
                }
            }

            let mut out = io::stdout().lock();
            match writeln!(out, "{}", bootstrap.url).and_then(|()| out.flush()) {
                Ok(()) => exit::SUCCESS,
                Err(error) => {
                    eprintln!("keepster: could not write the address: {error}");
                    exit::OTHER
                }
            }
        }
    }
}

/// Hand `url` to the platform's own opener, which starts the user's default browser.
///
/// The URL is passed as one argument, never through a shell: it is the daemon's, and holds nothing but
/// the loopback address and a hex code, but no command line is built from text here.
fn open(url: &str) -> io::Result<()> {
    let mut command = if cfg!(target_os = "macos") {
        std::process::Command::new("open")
    } else if cfg!(windows) {
        let mut command = std::process::Command::new("rundll32");
        command.arg("url.dll,FileProtocolHandler");
        command
    } else {
        std::process::Command::new("xdg-open")
    };
    let status = command
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()?;

    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!("the opener exited with {status}")))
    }
}
