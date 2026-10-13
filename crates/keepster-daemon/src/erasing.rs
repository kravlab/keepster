//! What `keepster uninstall` erases when asked (ticket 68): every Root's history, every Root's
//! registration, or everything Keepster keeps for this user. No install or uninstall path erases
//! anything on its own — a downgrade is a supported path, and on several channels it is
//! remove-then-install — so each part is selected by a flag and consented to by `--yes`.
//!
//! It runs with no daemon: the uninstall has stopped it first and refuses to erase beside one still
//! running, so no store lock is taken here.

use std::path::Path;

use keepster_app::roots::{self, Erasure};
use keepster_cli::{UninstallArgs, exit};
use keepster_store::Registry;

/// What the flags select.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Selection {
    /// Part of what the Registry holds: the History Stores, the registrations, or both.
    Part(Erasure),
    /// Everything: the store base, the configuration and the logs.
    Everything,
}

impl Selection {
    /// What `args` select. Clap keeps `--purge` apart from the other two.
    pub(crate) fn of(args: &UninstallArgs) -> Self {
        if args.purge {
            return Self::Everything;
        }

        Self::Part(Erasure {
            history: args.history,
            roots: args.roots,
        })
    }

    /// Whether nothing is selected, which is the plain uninstall.
    pub(crate) fn is_nothing(self) -> bool {
        self == Self::Part(Erasure::default())
    }

    /// Refuse a selection that erases without `--yes`, saying what it would erase.
    pub(crate) fn consented(self, yes: bool) -> Result<(), String> {
        if yes || self.is_nothing() {
            return Ok(());
        }
        let what = match self {
            Self::Everything => {
                "--purge erases every Root's history, the Roots, the configuration \
                                 and the logs"
            }
            Self::Part(Erasure {
                history: true,
                roots: true,
            }) => "--history --roots erases every Root, its history included",
            Self::Part(Erasure { history: true, .. }) => "--history erases every Root's history",
            Self::Part(_) => "--roots stops watching every Root",
        };

        Err(keepster_cli::consent::refusal(what))
    }
}

/// Erase what `selection` selects, saying what was erased.
pub(crate) fn erase(selection: Selection) -> u8 {
    let Some(store_base) = keepster_service::default_store_base() else {
        eprintln!("keepster: this platform has no per-user data directory, so nothing was erased");
        return exit::OTHER;
    };
    match selection {
        Selection::Part(erasure) => part(&store_base, erasure),
        Selection::Everything => everything(&store_base),
    }
}

/// Erase from every Root under `store_base` what `erasure` selects.
fn part(store_base: &Path, erasure: Erasure) -> u8 {
    if !store_base.exists() {
        println!("no Root was ever watched here; nothing was erased");
        return exit::SUCCESS;
    }
    match roots::erase(&Registry::at(store_base), erasure) {
        Ok(erased) => {
            if erasure.history {
                println!("erased the history of {} Root(s)", erased.histories);
            }
            if erasure.roots && !erasure.history {
                println!(
                    "stopped watching {} Root(s); their history is kept, decaying to its last state, \
                     which is pinned",
                    erased.unwatched
                );
            }
            exit::SUCCESS
        }
        Err(error) => {
            eprintln!("keepster: {error}; what was erased before it stays erased");
            exit::OTHER
        }
    }
}

/// Erase the store base, the configuration's directory and the log directory — each one Keepster's own.
fn everything(store_base: &Path) -> u8 {
    let config_dir = keepster_service::default_config_path()
        .and_then(|path| path.parent().map(Path::to_path_buf));
    let mut code = exit::SUCCESS;
    for dir in [
        Some(store_base.to_path_buf()),
        config_dir,
        keepster_service::default_log_dir(),
    ]
    .into_iter()
    .flatten()
    {
        match std::fs::remove_dir_all(&dir) {
            Ok(()) => println!("erased {}", dir.display()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                eprintln!("keepster: {} was not erased: {error}", dir.display());
                code = exit::OTHER;
            }
        }
    }

    code
}

#[cfg(test)]
mod tests {
    use keepster_app::roots::Erasure;

    use super::Selection;

    fn refusal(history: bool, roots: bool) -> String {
        Selection::Part(Erasure { history, roots })
            .consented(false)
            .expect_err("no consent was given")
    }

    #[test]
    fn a_refusal_names_what_each_selection_would_erase() {
        assert!(refusal(true, false).starts_with("--history erases every Root's history;"));
        assert!(refusal(false, true).starts_with("--roots stops watching every Root;"));
        // Both together leave nothing of any Root, which the refusal must not understate.
        assert!(
            refusal(true, true)
                .starts_with("--history --roots erases every Root, its history included;")
        );
        assert!(
            Selection::Everything
                .consented(false)
                .expect_err("no consent was given")
                .starts_with("--purge erases every Root's history")
        );
    }

    #[test]
    fn consent_or_an_empty_selection_is_not_refused() {
        assert_eq!(Selection::Part(Erasure::default()).consented(false), Ok(()));
        assert_eq!(Selection::Everything.consented(true), Ok(()));
    }
}
