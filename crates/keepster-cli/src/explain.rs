//! `keepster explain`: whether a path is captured, and which rule at which level decided it.

use std::io::{self, Write};
use std::path::Path;

use keepster_api::{
    ConfigLevel, Decision, Explanation, Origin, RuleEffect, RuleReference, encode_path_query,
};

use crate::client::DaemonClient;
use crate::command::ExplainArgs;
use crate::exit;
use crate::history::located;
use crate::roots::body_of;

/// Ask the daemon about one path and print its answer.
pub async fn explain(args: &ExplainArgs, control_path: &Path) -> u8 {
    let client = DaemonClient::new(control_path);

    let (root, relative) = match located(&client, args.root.as_deref(), Some(&args.path)).await {
        Ok(found) => found,
        Err(code) => return code,
    };
    let relative = relative.unwrap_or_default();

    let url = format!(
        "/v1/roots/{}/explain?path={}",
        root.id,
        encode_path_query(&relative)
    );
    let body = match body_of(client.get(&url).await) {
        Ok(body) => body,
        Err(code) => return code,
    };

    let mut out = io::stdout().lock();
    let written = if args.json {
        // Verbatim, as `status --json` is: one schema, the daemon's.
        out.write_all(&body)
    } else {
        match serde_json::from_slice::<Explanation>(&body) {
            Ok(explained) => write_explanation(&explained, &mut out),
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

/// The answer in words: the decision first, then the rule behind it, then anything worth a warning.
fn write_explanation(explained: &Explanation, out: &mut impl Write) -> io::Result<()> {
    let path = if explained.path.is_empty() {
        "the Root itself"
    } else {
        explained.path.as_str()
    };

    match explained.decision {
        Decision::Captured => {
            writeln!(out, "{path}: captured")?;
            if let Some(rule) = &explained.rule {
                writeln!(out, "  re-included by {}", describe(rule))?;
            }
        }
        Decision::Excluded => {
            writeln!(out, "{path}: not captured, excluded")?;
            if let Some(rule) = &explained.rule {
                writeln!(out, "  by {}", describe(rule))?;
            }
        }
        Decision::Pruned => {
            let ancestor = explained.pruned_by.as_deref().unwrap_or("an ancestor");
            writeln!(
                out,
                "{path}: not captured, because `{ancestor}` is excluded and never entered"
            )?;
            if let Some(rule) = &explained.rule {
                writeln!(out, "  `{ancestor}` is excluded by {}", describe(rule))?;
            }
            if let Some(own) = explained
                .own_rule
                .as_ref()
                .filter(|rule| rule.effect == RuleEffect::Include)
            {
                writeln!(
                    out,
                    "  {} would re-include it, but an excluded directory wins",
                    describe(own)
                )?;
            }
        }
        Decision::Secret => {
            writeln!(out, "{path}: not captured, refused as a likely secret")?;
            if let Some(rule) = &explained.rule {
                writeln!(out, "  by {}", describe(rule))?;
            }
            writeln!(
                out,
                "  to capture it anyway, add it to files.secret_opt_in in the global configuration"
            )?;
        }
        Decision::Withheld => match explained.reason.as_deref() {
            Some("too_large") => {
                let size = explained.size.unwrap_or_default();
                writeln!(
                    out,
                    "{path}: withheld, {size} bytes is over the {}-byte cap",
                    explained.max_size
                )?;
                match &explained.rule {
                    Some(rule) => writeln!(out, "  cap set by {}", describe(rule))?,
                    None => writeln!(
                        out,
                        "  cap set by max_size ({})",
                        where_set(&explained.max_size_origin)
                    )?,
                }
            }
            Some("reserved_name") => {
                writeln!(out, "{path}: never captured, its name is reserved")?;
            }
            Some(reason) => writeln!(out, "{path}: withheld ({reason})")?,
            None => writeln!(out, "{path}: withheld")?,
        },
    }

    if explained.on_disk.is_none() {
        writeln!(out, "  (nothing is at this path right now)")?;
    }
    for warning in &explained.warnings {
        writeln!(out, "warning: {} ({})", warning.message, warning.code)?;
    }

    Ok(())
}

/// A rule as a person wrote it: the pattern, its level, and where.
fn describe(rule: &RuleReference) -> String {
    format!("`{}` ({})", rule.pattern, where_set(&rule.origin))
}

/// Where a rule or a value was set: its level, its file and its line.
pub(crate) fn where_set(origin: &Origin) -> String {
    let level = match origin.level {
        ConfigLevel::Default => "default",
        ConfigLevel::IgnoreFile => "ignore-file",
        ConfigLevel::Global => "global",
        ConfigLevel::GlobalPattern => "global-pattern",
        ConfigLevel::GlobalRoot => "global-root",
        ConfigLevel::Root => "root",
    };

    match origin.line {
        Some(line) => format!("{level}, {} line {line}", origin.source),
        None => format!("{level}, {}", origin.source),
    }
}
