//! `keepster config`: a Root's effective configuration, and the level that set each key.

use std::io::{self, Write};
use std::path::Path;

use keepster_api::{ConfigKey, ConfigLevel, Origin, RootConfiguration};

use crate::client::DaemonClient;
use crate::command::ConfigArgs;
use crate::exit;
use crate::explain::where_set;
use crate::history::located;
use crate::roots::body_of;

/// Ask the daemon for a Root's effective configuration and print it.
pub async fn config(args: &ConfigArgs, control_path: &Path) -> u8 {
    let client = DaemonClient::new(control_path);

    let (root, _) = match located(&client, args.root.as_deref(), None).await {
        Ok(found) => found,
        Err(code) => return code,
    };
    let body = match body_of(client.get(&format!("/v1/roots/{}/config", root.id)).await) {
        Ok(body) => body,
        Err(code) => return code,
    };

    let mut out = io::stdout().lock();
    let written = if args.json {
        // Verbatim, as `status --json` is: one schema, the daemon's.
        out.write_all(&body)
    } else {
        match serde_json::from_slice::<RootConfiguration>(&body) {
            Ok(configuration) => write_configuration(&configuration, &mut out),
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

/// One key per line — a list's entries beneath it, each with where it was written — then anything
/// wrong with the files the values came from.
fn write_configuration(configuration: &RootConfiguration, out: &mut impl Write) -> io::Result<()> {
    for key in &configuration.keys {
        write_key(key, out)?;
    }

    for warning in &configuration.warnings {
        writeln!(out, "warning: {} ({})", warning.message, warning.code)?;
    }
    for error in &configuration.errors {
        writeln!(
            out,
            "error: {} — the last valid version of that file is in force until it is fixed",
            error.detail
        )?;
    }

    Ok(())
}

fn write_key(key: &ConfigKey, out: &mut impl Write) -> io::Result<()> {
    let restart = if key.restart_required {
        " — a file sets another value, which takes effect at the next restart"
    } else {
        ""
    };

    match &key.entries {
        Some(entries) if !entries.is_empty() => {
            writeln!(out, "{}:{restart}", key.key)?;
            for entry in entries {
                writeln!(
                    out,
                    "  {}  ({})",
                    shown(&entry.value),
                    where_set(&entry.origin)
                )?;
            }
        }
        _ => writeln!(
            out,
            "{} = {} ({}){restart}",
            key.key,
            shown(&key.value),
            set_by(&key.origin)
        )?,
    }

    Ok(())
}

/// Where a value was set, or just `default` for one nobody set.
fn set_by(origin: &Origin) -> String {
    match origin.level {
        ConfigLevel::Default => "default".to_owned(),
        _ => where_set(origin),
    }
}

/// A value as a person reads it: text without quotes, a size cap after its pattern.
pub(crate) fn shown(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Object(fields) if fields.contains_key("pattern") => {
            let pattern = fields["pattern"].as_str().unwrap_or_default();
            match fields.get("max").and_then(serde_json::Value::as_u64) {
                Some(max) => format!("{pattern} (at most {max} bytes)"),
                None => format!("{pattern} (back to the scalar cap)"),
            }
        }
        other => other.to_string(),
    }
}
